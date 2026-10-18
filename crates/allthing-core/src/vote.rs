// SPDX-License-Identifier: AGPL-3.0-or-later
//! The October 2026 Farcaster vote: after Neynar sold Farcaster to Network
//! School, farcaster.xyz asked every account to "vote to stay" (accept the
//! new client agreement) or "vote to leave". A stay vote is recorded on Base
//! as an opt-in NFT whose token id is the FID; a leave vote is only recorded
//! by the farcaster.xyz backend. See docs/VOTE.md.

use serde::{Deserialize, Serialize};

use crate::api::Meta;

/// "Network School Farcaster client" (VOTE), an ERC-721 on Base.
pub const VOTE_CONTRACT: &str = "0x4ecfEd276c566AeB49Da1f3A888E8Dbb7263028D";
/// `minted(uint256 fid)`: 1 while the FID holds a stay vote.
pub const MINTED_SELECTOR: &str = "7dc0bf3f";
/// Per-FID revocation count (selector `0xae36b5b8`; the name is not public).
pub const REVOKED_SELECTOR: &str = "ae36b5b8";

/// ABI-encode a call taking one `uint256 fid`.
pub fn encode_fid_call(selector: &str, fid: u64) -> String {
    format!("0x{selector}{fid:064x}")
}

/// What Base says about an FID's vote.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnchainVote {
    /// Holds the opt-in NFT: voted to stay.
    Stay,
    /// Voted to stay, and the opt-in was later revoked (moderated out).
    Revoked,
    /// No stay vote onchain: voted to leave, or did not vote.
    NoVote,
}

impl OnchainVote {
    /// From `minted(fid)` and the revocation count.
    pub fn from_words(minted: u64, revoked: u64) -> OnchainVote {
        match (minted, revoked) {
            (m, _) if m > 0 => OnchainVote::Stay,
            (_, r) if r > 0 => OnchainVote::Revoked,
            _ => OnchainVote::NoVote,
        }
    }
}

/// What the farcaster.xyz app reports (`viewerContext.optInStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppVote {
    OptedIn,
    OptedOut,
    ModeratedOut,
    Pending,
}

impl AppVote {
    pub fn parse(value: &str) -> Option<AppVote> {
        match value {
            "opted_in" => Some(AppVote::OptedIn),
            "opted_out" => Some(AppVote::OptedOut),
            "moderated_out" => Some(AppVote::ModeratedOut),
            "pending" => Some(AppVote::Pending),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VotePayload {
    pub fid: u64,
    /// `None` when Base could not be read.
    pub onchain: Option<OnchainVote>,
    /// `None` when farcaster.xyz could not be read.
    pub app: Option<AppVote>,
    pub meta: Meta,
}

impl VotePayload {
    /// A short label and where it comes from. The onchain record wins; the
    /// app's word is only used for what Base cannot know (leaving).
    pub fn summary(&self) -> (&'static str, &'static str) {
        match (self.onchain, self.app) {
            (Some(OnchainVote::Stay), _) => ("Voted to stay", "Recorded onchain on Base (opt-in NFT for this FID)."),
            (Some(OnchainVote::Revoked), _) => {
                ("Stay vote revoked", "Voted to stay; the opt-in was later revoked onchain.")
            }
            (_, Some(AppVote::ModeratedOut)) => {
                ("Stay vote revoked", "Per farcaster.xyz; its client opt-in was moderated out.")
            }
            (_, Some(AppVote::OptedOut)) => {
                ("Voted to leave", "Per farcaster.xyz; leave votes are not recorded onchain.")
            }
            (Some(OnchainVote::NoVote), Some(AppVote::OptedIn)) => {
                ("Unknown", "Base reports no current stay vote, but farcaster.xyz reports opted in.")
            }
            (None, Some(AppVote::OptedIn)) => {
                ("Voted to stay", "Per farcaster.xyz; the onchain record could not be read.")
            }
            (Some(OnchainVote::NoVote), None) => {
                ("Unknown", "No stay vote onchain; farcaster.xyz could not be read to check for a leave vote.")
            }
            (Some(OnchainVote::NoVote), Some(AppVote::Pending)) => {
                ("Did not vote", "No stay vote onchain, and farcaster.xyz reports no leave vote.")
            }
            (None, _) => ("Unknown", "The vote could not be read just now."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::Backend;

    fn payload(onchain: Option<OnchainVote>, app: Option<AppVote>) -> VotePayload {
        VotePayload { fid: 1, onchain, app, meta: Meta { backend: Backend::Hypersnap, fetched_at: 0, stale: false } }
    }

    #[test]
    fn missing_and_conflicting_sources_do_not_invent_vote_results() {
        assert_eq!(payload(Some(OnchainVote::NoVote), None).summary().0, "Unknown");
        assert_eq!(payload(Some(OnchainVote::NoVote), Some(AppVote::OptedIn)).summary().0, "Unknown");
        let (_, detail) = payload(None, Some(AppVote::ModeratedOut)).summary();
        assert!(detail.contains("farcaster.xyz"));
        assert!(!detail.contains("revoked onchain"));
        assert_eq!(payload(None, Some(AppVote::OptedOut)).summary().0, "Voted to leave");
        assert_eq!(payload(None, Some(AppVote::OptedIn)).summary().0, "Voted to stay");
        assert_eq!(payload(Some(OnchainVote::Stay), Some(AppVote::OptedOut)).summary().0, "Voted to stay");
        assert_eq!(payload(Some(OnchainVote::Revoked), None).summary().0, "Stay vote revoked");
    }

    #[test]
    fn encodes_and_summarises() {
        assert_eq!(
            encode_fid_call(MINTED_SELECTOR, 539_854),
            "0x7dc0bf3f0000000000000000000000000000000000000000000000000000000000083cce"
        );
        assert_eq!(OnchainVote::from_words(1, 0), OnchainVote::Stay);
        assert_eq!(OnchainVote::from_words(0, 1), OnchainVote::Revoked);
        assert_eq!(OnchainVote::from_words(0, 0), OnchainVote::NoVote);
        assert_eq!(AppVote::parse("opted_out"), Some(AppVote::OptedOut));
        assert_eq!(payload(Some(OnchainVote::Stay), Some(AppVote::OptedIn)).summary().0, "Voted to stay");
        assert_eq!(payload(Some(OnchainVote::NoVote), Some(AppVote::OptedOut)).summary().0, "Voted to leave");
        assert_eq!(payload(Some(OnchainVote::NoVote), Some(AppVote::Pending)).summary().0, "Did not vote");
        assert_eq!(payload(Some(OnchainVote::NoVote), None).summary().0, "Unknown");
        assert_eq!(payload(Some(OnchainVote::Revoked), None).summary().0, "Stay vote revoked");
        assert_eq!(payload(None, None).summary().0, "Unknown");
    }
}
