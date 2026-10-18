// SPDX-License-Identifier: AGPL-3.0-or-later
//! The JSON contract between the Worker (`/api/v1/*`) and the browser app,
//! and the cache policy behind each endpoint (see docs/BUDGET.md).
//!
//! Each page needs exactly one API call: the Worker aggregates everything a
//! page shows into one payload, and the browser sorts and pages it locally,
//! so switching sort or page costs no request at all.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::latest::Cursor;
use crate::model::{Cast, Channel, ThreadNode, User, UserSummary};
use crate::params::percent_encode;
use crate::provenance::{Backend, Origin};
use crate::ranking::TrendingWindow;

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
    /// cast hash -> the signer that signed it and the app that requested it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub origins: BTreeMap<String, Origin>,
    /// app fid -> the app's own Farcaster account.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub apps: BTreeMap<u64, UserSummary>,
    /// Casts whose origin is still being resolved in the background.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pending: u32,
    /// Link URL (see [`crate::embeds::link_key`]) -> whether its page
    /// declares a mini app or frame. Read from the page's meta tags by the
    /// Worker, and carried to the next build of the same payload.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub links: BTreeMap<String, bool>,
    /// Quoted cast hash -> the quoted cast, for listing cards that quote
    /// one. A hidden quoted cast (spam, a mini app) comes without its
    /// content (see [`crate::quotes::as_quote`]). Carried to the next build
    /// of the same payload while still quoted.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub quotes: BTreeMap<String, Cast>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Annotations {
    /// Add a page's facts without losing facts from earlier pages. A value
    /// present in the newer page replaces the old one; an absent value does
    /// not erase an already known fact.
    pub fn merge(&mut self, newer: &Annotations) {
        self.origins.extend(newer.origins.clone());
        self.apps.extend(newer.apps.clone());
        self.links.extend(newer.links.clone());
        self.quotes.extend(newer.quotes.clone());
        self.pending = self.pending.saturating_add(newer.pending);
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
    pub notes: Annotations,
    pub meta: Meta,
}

/// The newest casts on Farcaster, from Snapchain's event stream: the front
/// page's "new" list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatestPayload {
    /// Newest first.
    pub pool: Vec<Cast>,
    #[serde(default)]
    pub notes: Annotations,
    /// Where the next build continues reading each shard's events.
    #[serde(default)]
    pub cursors: Vec<Cursor>,
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

/// The front page's trending casts over `window`, from `offset` (later pages
/// are for infinite scroll). The default window (a day) has no parameter.
pub fn home_page_path(offset: u32, window: TrendingWindow) -> String {
    let mut query = Vec::new();
    if window != TrendingWindow::Day {
        query.push(format!("window={}", window.as_str()));
    }
    if offset != 0 {
        query.push(format!("offset={offset}"));
    }
    if query.is_empty() { home_path() } else { format!("{API_PREFIX}/home?{}", query.join("&")) }
}

/// The newest casts (the front page's "new").
pub fn latest_path() -> String {
    format!("{API_PREFIX}/latest")
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
/// New casts arrive every few seconds: fresh for 30 s, then refreshed in the
/// background for a minute, then rebuilt while the reader waits.
pub const LATEST_CACHE: CachePolicy = CachePolicy { fresh: 30, stale: 60, keep: 86_400, browser: 15 };
pub const CHANNEL_CACHE: CachePolicy = CachePolicy { fresh: 120, stale: 900, keep: 86_400, browser: 60 };
pub const CAST_CACHE: CachePolicy = CachePolicy { fresh: 120, stale: 1800, keep: 86_400, browser: 60 };
pub const USER_CACHE: CachePolicy = CachePolicy { fresh: 300, stale: 1800, keep: 86_400, browser: 120 };
/// When the account in a cached profile payload got its first username, if
/// the payload is `fid`'s and knew it: a build whose own read failed keeps it.
pub fn first_username_at_in(payload: &[u8], fid: u64) -> Option<i64> {
    #[derive(Deserialize)]
    struct Previous {
        user: PreviousUser,
    }
    #[derive(Deserialize)]
    struct PreviousUser {
        fid: u64,
        #[serde(default)]
        first_username_at: Option<i64>,
    }
    let user = serde_json::from_slice::<Previous>(payload).ok()?.user;
    if user.fid == fid { user.first_username_at } else { None }
}

/// A profile built while a secondary read failed (when the account joined):
/// refreshed soon, so the gap isn't kept for the usual five minutes.
pub const USER_RETRY_CACHE: CachePolicy = CachePolicy { fresh: 30, stale: 300, keep: 86_400, browser: 15 };
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

    fn quoted(hash: &str) -> Cast {
        serde_json::from_value(serde_json::json!({
            "hash": hash, "author": { "fid": 2, "username": "quoted" }, "text": "quoted words", "ts": 1_700_000_000,
        }))
        .unwrap()
    }

    #[test]
    fn merging_pages_keeps_known_facts_and_prefers_newer_values() {
        let mut notes = Annotations {
            origins: BTreeMap::from([("first".into(), Origin { signer: "old".into(), app_fid: None })]),
            links: BTreeMap::from([("https://first.example".into(), false), ("https://both.example".into(), false)]),
            quotes: BTreeMap::from([("0xq1".into(), quoted("0xq1"))]),
            pending: 2,
            ..Default::default()
        };
        let newer = Annotations {
            origins: BTreeMap::from([("later".into(), Origin { signer: "new".into(), app_fid: Some(99) })]),
            apps: BTreeMap::from([(
                99,
                UserSummary { fid: 99, username: "app".into(), display_name: String::new(), pfp_url: None },
            )]),
            links: BTreeMap::from([("https://later.example".into(), true), ("https://both.example".into(), true)]),
            quotes: BTreeMap::from([("0xq2".into(), quoted("0xq2"))]),
            pending: 1,
        };
        notes.merge(&newer);
        assert_eq!(notes.origins.len(), 2);
        assert_eq!(notes.app_for("later").map(|app| app.username.as_str()), Some("app"));
        assert_eq!(
            notes.links,
            BTreeMap::from([
                ("https://both.example".into(), true),
                ("https://first.example".into(), false),
                ("https://later.example".into(), true),
            ])
        );
        assert_eq!(notes.quotes.keys().collect::<Vec<_>>(), ["0xq1", "0xq2"]);
        assert_eq!(notes.pending, 3);
    }

    #[test]
    fn paths_encode_keys() {
        assert_eq!(home_path(), "/api/v1/home");
        assert_eq!(home_page_path(0, TrendingWindow::Day), "/api/v1/home");
        assert_eq!(home_page_path(200, TrendingWindow::Day), "/api/v1/home?offset=200");
        assert_eq!(home_page_path(0, TrendingWindow::Hour), "/api/v1/home?window=1h");
        assert_eq!(home_page_path(100, TrendingWindow::Week), "/api/v1/home?window=7d&offset=100");
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
        notes.origins.insert("0xab".into(), Origin { signer: "0x01".into(), app_fid: Some(9152) });
        notes.apps.insert(
            9152,
            UserSummary { fid: 9152, username: "warpcast".into(), display_name: "Warpcast".into(), pfp_url: None },
        );
        notes.quotes.insert("0xq1".into(), quoted("0xq1"));
        let json = serde_json::to_string(&notes).unwrap();
        assert!(json.contains(r#""quotes":{"0xq1":{"hash":"0xq1""#), "{json}");
        let back: Annotations = serde_json::from_str(&json).unwrap();
        assert_eq!(back, notes);
        assert_eq!(back.app_for("0xab").map(|a| a.username.as_str()), Some("warpcast"));
        assert_eq!(back.app_for("0xcd"), None);
        assert_eq!(serde_json::to_string(&Annotations::default()).unwrap(), "{}");
    }

    #[test]
    fn payloads_cached_with_scores_still_parse() {
        // Payloads cached before scores were removed still carry them.
        let notes: Annotations = serde_json::from_str(r#"{"scores":{"3":1000000},"pending":2}"#).unwrap();
        assert_eq!(notes, Annotations { pending: 2, ..Default::default() });
        let who: ReactionsPayload = serde_json::from_str(
            r#"{"hash":"0xab","kind":"likes","users":[],"scores":{"3":1000000},"meta":{"backend":"hypersnap","fetched_at":0}}"#,
        )
        .unwrap();
        assert_eq!(who.kind, ReactionKind::Likes);
    }

    #[test]
    fn a_profile_keeps_the_first_username_date_its_previous_copy_had() {
        let payload = br#"{"user":{"fid":3,"username":"dwr","first_username_at":1623888000},"pool":[],"meta":{"backend":"hypersnap","fetched_at":0}}"#;
        assert_eq!(first_username_at_in(payload, 3), Some(1_623_888_000));
        assert_eq!(first_username_at_in(payload, 4), None);
        assert_eq!(first_username_at_in(br#"{"user":{"fid":3,"username":"dwr"},"pool":[]}"#, 3), None);
        assert_eq!(first_username_at_in(b"not json", 3), None);
    }

    #[test]
    fn policies_are_ordered() {
        for policy in [
            HOME_CACHE,
            LATEST_CACHE,
            CHANNEL_CACHE,
            CAST_CACHE,
            USER_CACHE,
            USER_RETRY_CACHE,
            WALLET_CACHE,
            CHANNELS_CACHE,
            MISS_CACHE,
        ] {
            assert!(policy.fresh <= policy.stale && policy.stale <= policy.keep);
            assert!(policy.browser <= policy.fresh);
        }
    }
}
