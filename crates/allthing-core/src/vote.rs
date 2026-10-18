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
/// `mintGeneration(uint256 fid)`: raised by one each time the FID's stay
/// vote is revoked (a new vote must be signed for the new generation).
pub const MINT_GENERATION_SELECTOR: &str = "ae36b5b8";

/// ABI-encode a call taking one `uint256 fid`.
pub fn encode_fid_call(selector: &str, fid: u64) -> String {
    format!("0x{selector}{fid:064x}")
}

/// What Base says about an FID's vote.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnchainVote {
    /// Holds the opt-in token: voted to stay.
    Stay,
    /// Voted to stay, and Network School's moderation later revoked it,
    /// which burns the token and cancels the vote.
    Revoked,
    /// No stay vote onchain: voted to leave (recorded offchain only), or did
    /// not vote.
    NoVote,
}

impl OnchainVote {
    /// From `minted(fid)` and `mintGeneration(fid)`.
    pub fn from_words(minted: u64, generation: u64) -> OnchainVote {
        match (minted, generation) {
            (m, _) if m > 0 => OnchainVote::Stay,
            (_, g) if g > 0 => OnchainVote::Revoked,
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

/// How an account voted, as profiles say it: stay is "yes", leave is "no".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoteResult {
    Yes,
    No,
    DidNotVote,
}

impl VoteResult {
    pub fn label(self) -> &'static str {
        match self {
            VoteResult::Yes => "Voted yes",
            VoteResult::No => "Voted no",
            VoteResult::DidNotVote => "Didn't vote",
        }
    }

    /// The CSS modifier for its pill.
    pub fn class(self) -> &'static str {
        match self {
            VoteResult::Yes => "vote-yes",
            VoteResult::No => "vote-no",
            VoteResult::DidNotVote => "vote-none",
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
    /// The result and where it comes from, or `None` when the sources can't
    /// settle it (profiles then leave the vote out). Base decides wherever it
    /// can: a stay vote is the token, and a revoked one is cancelled. Leave
    /// votes exist only at farcaster.xyz.
    pub fn summary(&self) -> Option<(VoteResult, &'static str)> {
        use AppVote::*;
        use OnchainVote::*;
        use VoteResult::{DidNotVote, No, Yes};
        Some(match (self.onchain, self.app) {
            (Some(Stay), _) => (Yes, "Voted to stay: recorded on Base as this account's opt-in token."),
            (Some(Revoked | NoVote), Some(OptedOut)) => {
                (No, "Voted to leave, per farcaster.xyz: leave votes aren't recorded on Base.")
            }
            (Some(Revoked), _) => {
                (DidNotVote, "A stay vote on Base was revoked by Network School's moderation, which cancels it.")
            }
            (Some(NoVote), Some(Pending | ModeratedOut)) => {
                (DidNotVote, "No vote on Base, and no leave vote at farcaster.xyz.")
            }
            (Some(NoVote), Some(OptedIn)) => {
                (DidNotVote, "No vote is recorded on Base, though farcaster.xyz lists this account as opted in.")
            }
            (None, Some(OptedIn)) => (Yes, "Voted to stay, per farcaster.xyz: Base couldn't be read just now."),
            (None, Some(OptedOut)) => (No, "Voted to leave, per farcaster.xyz: leave votes aren't recorded on Base."),
            (None, Some(Pending | ModeratedOut)) => {
                (DidNotVote, "No vote, per farcaster.xyz: Base couldn't be read just now.")
            }
            // No stay vote, but a leave vote can't be ruled out; or nothing
            // could be read.
            (Some(NoVote), None) | (None, None) => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::Backend;

    fn shown(onchain: Option<OnchainVote>, app: Option<AppVote>) -> Option<&'static str> {
        let payload = VotePayload {
            fid: 1,
            onchain,
            app,
            meta: Meta { backend: Backend::Hypersnap, fetched_at: 0, stale: false },
        };
        payload.summary().map(|(result, _)| result.label())
    }

    #[test]
    fn every_combination_reads_yes_no_or_didnt_vote() {
        use AppVote::*;
        use OnchainVote::*;
        let onchain = [Some(Stay), Some(Revoked), Some(NoVote), None];
        let app = [Some(OptedIn), Some(OptedOut), Some(ModeratedOut), Some(Pending), None];
        for o in onchain {
            for a in app {
                if let Some(label) = shown(o, a) {
                    assert!(["Voted yes", "Voted no", "Didn't vote"].contains(&label), "{o:?} {a:?}: {label}");
                }
            }
        }
    }

    #[test]
    fn base_decides_where_it_can() {
        use AppVote::*;
        use OnchainVote::*;
        // The token is the stay vote, whatever the app says.
        assert_eq!(shown(Some(Stay), Some(OptedOut)), Some("Voted yes"));
        assert_eq!(shown(Some(Stay), None), Some("Voted yes"));
        // Revoking cancels the vote; a later leave vote still counts.
        assert_eq!(shown(Some(Revoked), None), Some("Didn't vote"));
        assert_eq!(shown(Some(Revoked), Some(ModeratedOut)), Some("Didn't vote"));
        assert_eq!(shown(Some(Revoked), Some(OptedOut)), Some("Voted no"));
        // Leave is offchain only.
        assert_eq!(shown(Some(NoVote), Some(OptedOut)), Some("Voted no"));
        assert_eq!(shown(Some(NoVote), Some(Pending)), Some("Didn't vote"));
        // The app says opted in but no token exists: Base's result stands.
        assert_eq!(shown(Some(NoVote), Some(OptedIn)), Some("Didn't vote"));
    }

    #[test]
    fn missing_sources_do_not_invent_a_result() {
        use AppVote::*;
        use OnchainVote::*;
        // No token, and farcaster.xyz unreadable: could be a leave vote.
        assert_eq!(shown(Some(NoVote), None), None);
        assert_eq!(shown(None, None), None);
        // Base unreadable: the app's word, said so in the tooltip.
        assert_eq!(shown(None, Some(OptedIn)), Some("Voted yes"));
        assert_eq!(shown(None, Some(OptedOut)), Some("Voted no"));
        assert_eq!(shown(None, Some(ModeratedOut)), Some("Didn't vote"));
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
        // The known FIDs in docs/VOTE.md.
        assert_eq!(shown(Some(OnchainVote::Stay), Some(AppVote::OptedIn)), Some("Voted yes")); // 539854, 3
        assert_eq!(shown(Some(OnchainVote::NoVote), Some(AppVote::Pending)), Some("Didn't vote")); // 5650
    }
}
