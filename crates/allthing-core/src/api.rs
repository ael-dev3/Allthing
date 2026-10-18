// SPDX-License-Identifier: AGPL-3.0-or-later
//! The JSON contract between the Worker (`/api/v1/*`) and the browser app,
//! and the cache policy behind each endpoint (see docs/BUDGET.md).
//!
//! Each page needs exactly one API call: the Worker aggregates everything a
//! page shows into one payload, and the browser sorts and pages it locally,
//! so switching sort or page costs no request at all.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{Cast, Channel, ThreadNode, User, UserSummary};
use crate::params::percent_encode;
use crate::provenance::{Backend, Origin};

pub const API_PREFIX: &str = "/api/v1";

/// Where and when a payload's data came from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub backend: Backend,
    /// Unix milliseconds when the Worker fetched the data upstream.
    pub fetched_at: i64,
    /// Served from cache past its freshness because the upstream is failing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
}

/// Per-account and per-cast facts gathered alongside a page's casts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Annotations {
    /// fid -> Neynar user score in millionths, read onchain on Base.
    /// Absent: unknown. Zero: Neynar has no score for the account.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scores: BTreeMap<u64, u32>,
    /// cast hash -> the signer that signed it and the app that requested it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub origins: BTreeMap<String, Origin>,
    /// app fid -> the app's own Farcaster account.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub apps: BTreeMap<u64, UserSummary>,
    /// Casts whose origin is still being resolved in the background.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pending: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Annotations {
    /// Take `fresh` scores for `fids`, keeping the ones already known for
    /// those fids (from Hypersnap or the page's previous copy) wherever no
    /// fresh score came back, as when the onchain read fails.
    pub fn merge_scores(&mut self, fresh: BTreeMap<u64, u32>, fids: &[u64]) {
        self.scores.retain(|fid, _| fids.contains(fid));
        self.scores.extend(fresh);
    }

    /// The app a cast was posted from, if known.
    pub fn app_for(&self, hash: &str) -> Option<&UserSummary> {
        let fid = self.origins.get(hash)?.app_fid?;
        self.apps.get(&fid)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomePayload {
    pub pool: Vec<Cast>,
    #[serde(default)]
    pub channels: Vec<Channel>,
    #[serde(default)]
    pub notes: Annotations,
    pub meta: Meta,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelPayload {
    pub channel: Channel,
    pub pool: Vec<Cast>,
    #[serde(default)]
    pub notes: Annotations,
    pub meta: Meta,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotedCast {
    pub hash: String,
    #[serde(default)]
    pub cast: Option<Cast>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CastPayload {
    pub thread: ThreadNode,
    #[serde(default)]
    pub parent: Option<Cast>,
    #[serde(default)]
    pub quotes: Vec<QuotedCast>,
    #[serde(default)]
    pub channel: Option<Channel>,
    #[serde(default)]
    pub notes: Annotations,
    pub meta: Meta,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserPayload {
    pub user: User,
    pub pool: Vec<Cast>,
    #[serde(default)]
    pub notes: Annotations,
    pub meta: Meta,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelsPayload {
    pub channels: Vec<Channel>,
    pub meta: Meta,
}

/// One search box: casts, people and channels matching a query.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPayload {
    pub query: String,
    pub casts: Vec<Cast>,
    pub users: Vec<User>,
    pub channels: Vec<Channel>,
    #[serde(default)]
    pub notes: Annotations,
    pub meta: Meta,
}

/// Which reactions a who-list shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReactionKind {
    Likes,
    Recasts,
}

impl ReactionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ReactionKind::Likes => "likes",
            ReactionKind::Recasts => "recasts",
        }
    }

    pub fn parse(value: &str) -> Option<ReactionKind> {
        match value {
            "likes" => Some(ReactionKind::Likes),
            "recasts" => Some(ReactionKind::Recasts),
            _ => None,
        }
    }
}

/// Who liked or recasted a cast, newest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionsPayload {
    pub hash: String,
    pub kind: ReactionKind,
    pub users: Vec<UserSummary>,
    /// More reactors exist than are listed.
    #[serde(default)]
    pub more: bool,
    #[serde(default)]
    pub scores: BTreeMap<u64, u32>,
    pub meta: Meta,
}

/// The first direct replies of a cast, for the replies preview.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepliesPayload {
    pub hash: String,
    pub replies: Vec<Cast>,
    #[serde(default)]
    pub notes: Annotations,
    pub meta: Meta,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
    /// Seconds to wait before retrying, for 429 and 503 answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u32>,
}

pub fn home_path() -> String {
    format!("{API_PREFIX}/home")
}

/// The next trending pages, for infinite scroll.
pub fn home_page_path(offset: u32) -> String {
    if offset == 0 { home_path() } else { format!("{API_PREFIX}/home?offset={offset}") }
}

pub fn vote_path(fid: u64) -> String {
    format!("{API_PREFIX}/vote?fid={fid}")
}

pub fn channel_path(key: &str) -> String {
    format!("{API_PREFIX}/channel?id={}", percent_encode(key))
}

pub fn cast_path(hash: &str) -> String {
    format!("{API_PREFIX}/cast?hash={hash}")
}

pub fn user_path(username: &str) -> String {
    format!("{API_PREFIX}/user?name={}", percent_encode(username))
}

pub fn wallets_path(fid: u64) -> String {
    format!("{API_PREFIX}/wallets?fid={fid}")
}

pub fn channels_path(query: Option<&str>) -> String {
    match query.map(str::trim).filter(|q| !q.is_empty()) {
        Some(q) => format!("{API_PREFIX}/channels?q={}", percent_encode(q)),
        None => format!("{API_PREFIX}/channels"),
    }
}

pub fn search_path(query: &str) -> String {
    format!("{API_PREFIX}/search?q={}", percent_encode(query.trim()))
}

/// `limit` is a short preview (10) or the full list (100).
pub fn reactions_path(hash: &str, kind: ReactionKind, limit: u32) -> String {
    format!("{API_PREFIX}/reactions?hash={hash}&type={}&limit={limit}", kind.as_str())
}

pub fn replies_path(hash: &str) -> String {
    format!("{API_PREFIX}/replies?hash={hash}")
}

/// How long each endpoint's edge-cached payload is served, in seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CachePolicy {
    /// Served as is.
    pub fresh: u32,
    /// Served while one background refresh runs (stale-while-revalidate).
    pub stale: u32,
    /// Kept for serving when the upstream fails (stale-if-error).
    pub keep: u32,
    /// `Cache-Control: max-age` for browsers.
    pub browser: u32,
}

pub const HOME_CACHE: CachePolicy = CachePolicy { fresh: 60, stale: 600, keep: 86_400, browser: 30 };
pub const CHANNEL_CACHE: CachePolicy = CachePolicy { fresh: 120, stale: 900, keep: 86_400, browser: 60 };
pub const CAST_CACHE: CachePolicy = CachePolicy { fresh: 120, stale: 1800, keep: 86_400, browser: 60 };
pub const USER_CACHE: CachePolicy = CachePolicy { fresh: 300, stale: 1800, keep: 86_400, browser: 120 };
pub const WALLET_CACHE: CachePolicy = CachePolicy { fresh: 21_600, stale: 86_400, keep: 172_800, browser: 3600 };
pub const CHANNELS_CACHE: CachePolicy = CachePolicy { fresh: 3600, stale: 86_400, keep: 172_800, browser: 600 };
pub const SEARCH_CACHE: CachePolicy = CachePolicy { fresh: 300, stale: 1800, keep: 86_400, browser: 120 };
pub const REACTIONS_CACHE: CachePolicy = CachePolicy { fresh: 300, stale: 3600, keep: 86_400, browser: 300 };
pub const REPLIES_CACHE: CachePolicy = CachePolicy { fresh: 120, stale: 1800, keep: 86_400, browser: 60 };
/// Votes change rarely (a vote can be changed or revoked); hourly is plenty.
pub const VOTE_CACHE: CachePolicy = CachePolicy { fresh: 3600, stale: 86_400, keep: 172_800, browser: 600 };
/// Lookups that found nothing (unknown user, missing cast).
pub const MISS_CACHE: CachePolicy = CachePolicy { fresh: 300, stale: 300, keep: 300, browser: 120 };

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_survive_a_failed_read() {
        let mut notes = Annotations::default();
        notes.scores.extend([(1, 740_000), (2, 500_000), (9, 100_000)]);
        // The onchain read failed: what was known for the page's fids stays.
        notes.merge_scores(BTreeMap::new(), &[1, 2, 3]);
        assert_eq!(notes.scores, BTreeMap::from([(1, 740_000), (2, 500_000)]));
        // A fresh read wins over what was known.
        notes.merge_scores(BTreeMap::from([(1, 750_000), (3, 900_000)]), &[1, 2, 3]);
        assert_eq!(notes.scores, BTreeMap::from([(1, 750_000), (2, 500_000), (3, 900_000)]));
    }

    #[test]
    fn paths_encode_keys() {
        assert_eq!(home_path(), "/api/v1/home");
        assert_eq!(channel_path("eip155:8453/erc20:0xabc"), "/api/v1/channel?id=eip155%3A8453%2Ferc20%3A0xabc");
        assert_eq!(user_path("balajis.eth"), "/api/v1/user?name=balajis.eth");
        assert_eq!(user_path("fid:42"), "/api/v1/user?name=fid%3A42");
        assert_eq!(channels_path(Some(" art ")), "/api/v1/channels?q=art");
        assert_eq!(channels_path(Some("")), "/api/v1/channels");
        assert_eq!(wallets_path(3), "/api/v1/wallets?fid=3");
        assert_eq!(search_path(" art club "), "/api/v1/search?q=art%20club");
        assert_eq!(reactions_path("0xab", ReactionKind::Likes, 10), "/api/v1/reactions?hash=0xab&type=likes&limit=10");
        assert_eq!(replies_path("0xab"), "/api/v1/replies?hash=0xab");
    }

    #[test]
    fn annotations_round_trip_and_resolve_apps() {
        let mut notes = Annotations::default();
        notes.scores.insert(3, 1_000_000);
        notes.origins.insert("0xab".into(), Origin { signer: "0x01".into(), app_fid: Some(9152) });
        notes.apps.insert(
            9152,
            UserSummary { fid: 9152, username: "warpcast".into(), display_name: "Warpcast".into(), pfp_url: None },
        );
        let json = serde_json::to_string(&notes).unwrap();
        let back: Annotations = serde_json::from_str(&json).unwrap();
        assert_eq!(back, notes);
        assert_eq!(back.app_for("0xab").map(|a| a.username.as_str()), Some("warpcast"));
        assert_eq!(back.app_for("0xcd"), None);
        assert_eq!(serde_json::to_string(&Annotations::default()).unwrap(), "{}");
    }

    #[test]
    fn policies_are_ordered() {
        for policy in [HOME_CACHE, CHANNEL_CACHE, CAST_CACHE, USER_CACHE, WALLET_CACHE, CHANNELS_CACHE, MISS_CACHE] {
            assert!(policy.fresh <= policy.stale && policy.stale <= policy.keep);
            assert!(policy.browser <= policy.fresh);
        }
    }
}
