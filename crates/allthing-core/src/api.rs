// SPDX-License-Identifier: AGPL-3.0-or-later
//! The JSON contract between the Worker (`/api/v1/*`) and the browser app,
//! and the cache policy behind each endpoint (see docs/BUDGET.md).
//!
//! Each page needs exactly one API call: the Worker aggregates everything a
//! page shows into one payload, and the browser sorts and pages it locally,
//! so switching sort or page costs no request at all.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::latest::Cursor;
use crate::model::{Cast, Channel, ThreadNode, User, UserSummary};
use crate::params::{hash_from_param, percent_encode};
use crate::profile::UserCard;
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
    /// Account fid -> what its profile card shows beyond its picture and
    /// names (see [`crate::profile`]), for the accounts the page shows. Read
    /// from the user objects that come with the page's casts, at no extra
    /// request, and carried to the next build while the account is shown.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub users: BTreeMap<u64, UserCard>,
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
        self.users.extend(newer.users.clone());
        self.pending = self.pending.saturating_add(newer.pending);
    }

    /// Keep the profile cards of the accounts in `shown`: the `fresh` ones,
    /// read with this build's casts, in place of those carried from the
    /// previous build, which stay only while their account is still shown
    /// (the author of a quote carried forward, say).
    pub fn keep_users(&mut self, fresh: BTreeMap<u64, UserCard>, shown: impl IntoIterator<Item = u64>) {
        let shown: HashSet<u64> = shown.into_iter().collect();
        self.users.extend(fresh);
        self.users.retain(|fid, _| shown.contains(fid));
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
    /// For Past month, Past year and All time: how much of Farcaster
    /// Allthing's index covers. Absent while the index can't rank them, and
    /// the pool is then the past week's trending casts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<IndexCoverage>,
}

/// What a ranking from Allthing's index was ranked from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexCoverage {
    /// Casts indexed in the range.
    pub casts: u64,
    /// The oldest of them, Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    /// When the ranking was computed, Unix milliseconds.
    pub ranked_at: i64,
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
    /// Fid -> what the profile card of a person listed shows beyond their
    /// picture and names (see [`crate::profile`]), from the user objects
    /// that come with the reactions.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cards: BTreeMap<u64, UserCard>,
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

impl RepliesPayload {
    /// The replies preview of a post's own payload, whose thread holds the
    /// cast's direct replies: hovering a feed card prefetches its post, so
    /// its replies preview needs no request of its own. Like
    /// `/api/v1/replies`, it leaves hidden replies out, emptied nodes that
    /// the thread keeps included, and lists the most engaged first.
    pub fn from_post(post: &CastPayload) -> RepliesPayload {
        let replies = post.thread.children.iter().map(|node| node.cast.clone()).collect();
        RepliesPayload {
            hash: post.thread.cast.hash.clone(),
            replies: crate::thread::reply_preview(replies),
            // The replies' links, and the profile cards of their authors.
            notes: Annotations {
                links: post.notes.links.clone(),
                users: post.notes.users.clone(),
                ..Annotations::default()
            },
            meta: post.meta.clone(),
        }
    }
}

/// People a who-liked or who-recasted preview lists.
pub const REACTIONS_PREVIEW: u32 = 10;

/// Feed cards whose previews load together: a card in view brings the run
/// of this many cards it belongs to, in list order (cards 1–6, 7–12, …), so
/// readers of the same feed ask for the same batches and the edge cache
/// answers most of them. Six keeps a batch's parsing within a page build's
/// CPU time even when every list is full (docs/BUDGET.md).
pub const ENGAGEMENT_CHUNK: usize = 6;

/// Lists one batch reads at most: a run of cards, each with likes and
/// recasts. Each list is one upstream read.
pub const ENGAGEMENT_MAX_LISTS: usize = 2 * ENGAGEMENT_CHUNK;

/// What `/api/v1/engagement` answers a malformed request.
pub const ENGAGEMENT_USAGE: &str =
    "Expected ?likes=<cast hashes>&recasts=<cast hashes>, comma-separated, 12 lists in all at most";

/// Which casts' likes and recasts a batch previews (`/api/v1/engagement`).
/// The hashes are normalized, sorted and distinct, so the same lists make
/// the same path and cache key whatever order and spelling they were asked
/// for in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EngagementRequest {
    likes: Vec<String>,
    recasts: Vec<String>,
}

impl EngagementRequest {
    /// For feed cards given as (hash, likes, recasts), in list order. A kind
    /// is asked for only when someone reacted: a zero count opens no
    /// preview. Malformed hashes are skipped, and cards that would take the
    /// batch past [`ENGAGEMENT_MAX_LISTS`] lists are left out.
    pub fn for_casts<'a>(casts: impl IntoIterator<Item = (&'a str, u64, u64)>) -> EngagementRequest {
        let mut request = EngagementRequest::default();
        for (hash, likes, recasts) in casts {
            let Some(hash) = hash_from_param(hash) else { continue };
            if request.len() + usize::from(likes > 0) + usize::from(recasts > 0) > ENGAGEMENT_MAX_LISTS {
                break;
            }
            if likes > 0 {
                request.likes.push(hash.clone());
            }
            if recasts > 0 {
                request.recasts.push(hash);
            }
        }
        request.canonical()
    }

    /// From a query's comma-separated `likes` and `recasts` cast hashes.
    /// `None` when any item is not a cast hash, or when no list or more than
    /// [`ENGAGEMENT_MAX_LISTS`] lists are asked for.
    pub fn parse(likes: Option<&str>, recasts: Option<&str>) -> Option<EngagementRequest> {
        let list = |value: Option<&str>| -> Option<Vec<String>> {
            match value.map(str::trim).filter(|v| !v.is_empty()) {
                None => Some(Vec::new()),
                // Refused before parsing a long list item by item.
                Some(v) if v.split(',').nth(ENGAGEMENT_MAX_LISTS).is_some() => None,
                Some(v) => v.split(',').map(hash_from_param).collect(),
            }
        };
        let request = EngagementRequest { likes: list(likes)?, recasts: list(recasts)? }.canonical();
        (!request.is_empty() && request.len() <= ENGAGEMENT_MAX_LISTS).then_some(request)
    }

    fn canonical(mut self) -> EngagementRequest {
        for list in [&mut self.likes, &mut self.recasts] {
            list.sort_unstable();
            list.dedup();
        }
        self
    }

    /// Casts whose likes are asked for, sorted.
    pub fn likes(&self) -> &[String] {
        &self.likes
    }

    /// Casts whose recasts are asked for, sorted.
    pub fn recasts(&self) -> &[String] {
        &self.recasts
    }

    /// Lists asked for; each is one upstream read.
    pub fn len(&self) -> usize {
        self.likes.len() + self.recasts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every list asked for, likes first.
    pub fn lists(&self) -> impl Iterator<Item = (&str, ReactionKind)> {
        let likes = self.likes.iter().map(|hash| (hash.as_str(), ReactionKind::Likes));
        likes.chain(self.recasts.iter().map(|hash| (hash.as_str(), ReactionKind::Recasts)))
    }

    /// The path that asks for each list alone, as a popover does: where a
    /// client files the batch's previews.
    pub fn preview_paths(&self) -> impl Iterator<Item = String> + '_ {
        self.lists().map(|(hash, kind)| reactions_path(hash, kind, REACTIONS_PREVIEW))
    }

    /// `/api/v1/engagement?likes=0x…,0x…&recasts=0x…`, leaving out a kind
    /// with no casts.
    pub fn path(&self) -> String {
        let mut query = Vec::new();
        if !self.likes.is_empty() {
            query.push(format!("likes={}", self.likes.join(",")));
        }
        if !self.recasts.is_empty() {
            query.push(format!("recasts={}", self.recasts.join(",")));
        }
        format!("{API_PREFIX}/engagement?{}", query.join("&"))
    }
}

/// Who liked and recasted a batch of feed cards (`/api/v1/engagement`),
/// loaded before anyone hovers a count. Each preview is exactly what
/// `/api/v1/reactions?hash=…&type=…&limit=10` answers for that cast and
/// kind. A list that couldn't be read this time is left out; its popover
/// loads it when it opens, as without the batch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngagementPayload {
    /// In the request's order: likes first, then recasts, each by hash.
    #[serde(default)]
    pub previews: Vec<ReactionsPayload>,
    pub meta: Meta,
}

impl EngagementPayload {
    /// Each preview with the path that asks for it alone
    /// ([`reactions_path`] with [`REACTIONS_PREVIEW`]).
    pub fn by_path(&self) -> impl Iterator<Item = (String, &ReactionsPayload)> {
        self.previews.iter().map(|preview| (reactions_path(&preview.hash, preview.kind, REACTIONS_PREVIEW), preview))
    }
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
/// Rankings from Allthing's index are recomputed every 5 minutes at most.
pub const INDEXED_HOME_CACHE: CachePolicy = CachePolicy { fresh: 300, stale: 1800, keep: 86_400, browser: 120 };
/// New casts arrive every few seconds: fresh for 30 s, then refreshed in the
/// background for a minute, then rebuilt while the reader waits.
pub const LATEST_CACHE: CachePolicy = CachePolicy { fresh: 30, stale: 60, keep: 86_400, browser: 15 };
/// Active accounts: counted every two minutes.
pub const ACTIVITY_CACHE: CachePolicy = CachePolicy { fresh: 60, stale: 600, keep: 86_400, browser: 30 };
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
/// Wallet activity with an EVM chain still unread: read again in minutes.
pub const WALLET_RETRY_CACHE: CachePolicy = CachePolicy { fresh: 300, stale: 3600, keep: 172_800, browser: 60 };
pub const CHANNELS_CACHE: CachePolicy = CachePolicy { fresh: 3600, stale: 86_400, keep: 172_800, browser: 600 };
pub const SEARCH_CACHE: CachePolicy = CachePolicy { fresh: 300, stale: 1800, keep: 86_400, browser: 120 };
pub const REACTIONS_CACHE: CachePolicy = CachePolicy { fresh: 300, stale: 3600, keep: 86_400, browser: 300 };
/// A batch of like and recast previews: as fresh as one preview, since
/// that is what each of them becomes in the browser.
pub const ENGAGEMENT_CACHE: CachePolicy = REACTIONS_CACHE;
/// A batch missing some of its lists (a read failed, or the build left
/// Hypersnap's bucket to page builds) is rebuilt soon, and browsers don't
/// keep it.
pub const ENGAGEMENT_PARTIAL_CACHE: CachePolicy = CachePolicy { fresh: 15, stale: 60, keep: 86_400, browser: 0 };
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

    fn card(bio: &str, followers: u64) -> UserCard {
        UserCard { bio: bio.into(), followers, ..Default::default() }
    }

    #[test]
    fn merging_pages_keeps_known_facts_and_prefers_newer_values() {
        let mut notes = Annotations {
            origins: BTreeMap::from([("first".into(), Origin { signer: "old".into(), app_fid: None })]),
            links: BTreeMap::from([("https://first.example".into(), false), ("https://both.example".into(), false)]),
            quotes: BTreeMap::from([("0xq1".into(), quoted("0xq1"))]),
            users: BTreeMap::from([(7, card("", 10)), (8, card("gm", 1))]),
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
            users: BTreeMap::from([(7, card("", 12))]),
            pending: 1,
        };
        notes.merge(&newer);
        assert_eq!(notes.origins.len(), 2);
        assert_eq!(notes.users, BTreeMap::from([(7, card("", 12)), (8, card("gm", 1))]));
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
    fn profile_cards_stay_for_the_accounts_still_shown() {
        let mut notes = Annotations {
            users: BTreeMap::from([(1, card("carried", 1)), (2, card("old", 2)), (3, card("gone", 3))]),
            ..Default::default()
        };
        let fresh = BTreeMap::from([(2, card("new", 20)), (4, card("new", 4)), (5, card("not shown", 5))]);
        notes.keep_users(fresh, [1, 2, 4, 6]);
        assert_eq!(notes.users, BTreeMap::from([(1, card("carried", 1)), (2, card("new", 20)), (4, card("new", 4))]));
        notes.keep_users(BTreeMap::new(), []);
        assert!(notes.users.is_empty());
    }

    #[test]
    fn payloads_cached_before_profile_cards_still_parse() {
        // Cached before `notes.users` existed: no cards, nothing else lost.
        let home: HomePayload = serde_json::from_str(
            r#"{"pool":[{"hash":"0xab","author":{"fid":3,"username":"dwr"},"text":"gm","ts":1}],
                "notes":{"origins":{"0xab":{"signer":"0x01","app_fid":9152}},"pending":1},
                "meta":{"backend":"hypersnap","fetched_at":0}}"#,
        )
        .unwrap();
        assert!(home.notes.users.is_empty());
        assert_eq!(home.notes.pending, 1);
        assert_eq!(home.notes.origins["0xab"].app_fid, Some(9152));
        let post: CastPayload = serde_json::from_str(
            r#"{"thread":{"cast":{"hash":"0xab","author":{"fid":3,"username":"dwr"},"text":"gm","ts":1}},
                "meta":{"backend":"hypersnap","fetched_at":0}}"#,
        )
        .unwrap();
        assert!(post.notes.users.is_empty());

        // Cards round-trip keyed by fid, and an empty map is left out.
        let mut notes = Annotations::default();
        notes.users.insert(
            3,
            UserCard {
                bio: "Interested in technology.".into(),
                followers: 1,
                registered_at: Some(9),
                ..Default::default()
            },
        );
        let json = serde_json::to_string(&notes).unwrap();
        assert_eq!(json, r#"{"users":{"3":{"bio":"Interested in technology.","followers":1,"registered_at":9}}}"#);
        assert_eq!(serde_json::from_str::<Annotations>(&json).unwrap(), notes);
        assert!(!serde_json::to_string(&Annotations::default()).unwrap().contains("users"));
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
            INDEXED_HOME_CACHE,
            LATEST_CACHE,
            CHANNEL_CACHE,
            CAST_CACHE,
            USER_CACHE,
            USER_RETRY_CACHE,
            WALLET_CACHE,
            WALLET_RETRY_CACHE,
            CHANNELS_CACHE,
            SEARCH_CACHE,
            REACTIONS_CACHE,
            ENGAGEMENT_CACHE,
            ENGAGEMENT_PARTIAL_CACHE,
            REPLIES_CACHE,
            VOTE_CACHE,
            MISS_CACHE,
        ] {
            assert!(policy.fresh <= policy.stale && policy.stale <= policy.keep);
            assert!(policy.browser <= policy.fresh);
        }
        // A partial batch is rebuilt well before a complete one.
        const { assert!(ENGAGEMENT_PARTIAL_CACHE.fresh * 10 <= ENGAGEMENT_CACHE.fresh) };
    }

    fn hash(n: u32) -> String {
        format!("0x{n:040x}")
    }

    fn person(fid: u64) -> UserSummary {
        UserSummary { fid, username: format!("user{fid}"), display_name: String::new(), pfp_url: None }
    }

    fn meta() -> Meta {
        Meta { backend: Backend::Hypersnap, fetched_at: 1_792_235_681_000, stale: false }
    }

    /// The query value of `key` in `path`.
    fn param<'a>(path: &'a str, key: &str) -> Option<&'a str> {
        let query = path.split_once('?')?.1;
        query.split('&').find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))
    }

    #[test]
    fn engagement_requests_are_canonical() {
        let (a, b) = (hash(0xa), hash(0xb));
        // Upper case, without 0x, repeated, padded and in any order: one
        // request, one path, one cache key.
        let messy = format!("{}, {} ,{b}", b.to_uppercase().replace("0X", "0x"), a.trim_start_matches("0x"));
        let request = EngagementRequest::parse(Some(&messy), Some(&a.to_uppercase())).unwrap();
        assert_eq!(request.likes(), [a.clone(), b.clone()]);
        assert_eq!(request.recasts(), std::slice::from_ref(&a));
        assert_eq!(request.len(), 3);
        assert_eq!(request.path(), format!("/api/v1/engagement?likes={a},{b}&recasts={a}"));
        let reordered = EngagementRequest::parse(Some(&format!("{a},{b}")), Some(&format!(" {a} "))).unwrap();
        assert_eq!(reordered, request);
        assert_eq!(
            request.lists().collect::<Vec<_>>(),
            [(a.as_str(), ReactionKind::Likes), (b.as_str(), ReactionKind::Likes), (a.as_str(), ReactionKind::Recasts)]
        );
        assert_eq!(
            request.preview_paths().collect::<Vec<_>>(),
            [
                reactions_path(&a, ReactionKind::Likes, 10),
                reactions_path(&b, ReactionKind::Likes, 10),
                reactions_path(&a, ReactionKind::Recasts, 10),
            ]
        );

        // A kind with no casts is left out of the path.
        let recasts = EngagementRequest::parse(None, Some(&b)).unwrap();
        assert_eq!(recasts.path(), format!("/api/v1/engagement?recasts={b}"));
        assert_eq!(EngagementRequest::parse(Some(" "), Some(&b)), Some(recasts));

        // Nothing at all, a malformed hash or an empty item is refused.
        assert_eq!(EngagementRequest::parse(None, None), None);
        assert_eq!(EngagementRequest::parse(Some(""), Some(" ")), None);
        assert_eq!(EngagementRequest::parse(Some("0x123"), None), None);
        assert_eq!(EngagementRequest::parse(Some(&format!("{a},,{b}")), None), None);
        assert_eq!(EngagementRequest::parse(Some(&format!("{a},nope")), None), None);
        assert_eq!(EngagementRequest::parse(Some(&format!("{a},{}", "zz".repeat(20))), None), None);
    }

    #[test]
    fn engagement_requests_are_capped() {
        let chunk = ENGAGEMENT_CHUNK as u32;
        let likes: Vec<String> = (0..chunk).map(hash).collect();
        let recasts: Vec<String> = (100..100 + chunk).map(hash).collect();
        let full = EngagementRequest::parse(Some(&likes.join(",")), Some(&recasts.join(","))).unwrap();
        assert_eq!(full.len(), ENGAGEMENT_MAX_LISTS);
        // One more list, in either kind, is too many.
        let over = format!("{},{}", recasts.join(","), hash(200));
        assert_eq!(EngagementRequest::parse(Some(&likes.join(",")), Some(&over)), None);
        let many: Vec<String> = (0..40).map(hash).collect();
        assert_eq!(EngagementRequest::parse(Some(&many.join(",")), None), None);
        // Repeats count against the cap before they are merged.
        let repeated = vec![hash(1); ENGAGEMENT_MAX_LISTS + 1].join(",");
        assert_eq!(EngagementRequest::parse(Some(&repeated), None), None);
        let most = vec![hash(1); ENGAGEMENT_MAX_LISTS].join(",");
        assert_eq!(EngagementRequest::parse(Some(&most), None).map(|r| r.len()), Some(1));
        // The error message names the cap.
        assert!(ENGAGEMENT_USAGE.contains(&format!(" {ENGAGEMENT_MAX_LISTS} lists ")));
    }

    #[test]
    fn engagement_requests_for_cards_ask_only_for_counts_above_zero() {
        let hashes: Vec<String> = (0..10).map(hash).collect();
        let cards = [(hashes[3].as_str(), 5, 0), (hashes[1].as_str(), 0, 2), (hashes[2].as_str(), 0, 0), ("bad", 9, 9)];
        let request = EngagementRequest::for_casts(cards);
        assert_eq!(request.likes(), [hashes[3].clone()]);
        assert_eq!(request.recasts(), [hashes[1].clone()]);
        assert!(EngagementRequest::for_casts([(hashes[2].as_str(), 0, 0)]).is_empty());

        // A run of cards with both counts fills a batch; later cards are left out.
        let busy: Vec<(&str, u64, u64)> = hashes.iter().map(|h| (h.as_str(), 1, 1)).collect();
        let request = EngagementRequest::for_casts(busy[..ENGAGEMENT_CHUNK].iter().copied());
        assert_eq!(request.len(), ENGAGEMENT_MAX_LISTS);
        assert_eq!(EngagementRequest::for_casts(busy.iter().copied()), request);
        // Its path parses back to the same request.
        let path = request.path();
        assert_eq!(EngagementRequest::parse(param(&path, "likes"), param(&path, "recasts")), Some(request));
    }

    #[test]
    fn engagement_previews_are_single_reaction_previews() {
        let (a, b) = (hash(0xa), hash(0xb));
        let preview = |hash: &str, kind, users: Vec<UserSummary>, more| ReactionsPayload {
            hash: hash.to_owned(),
            kind,
            users,
            more,
            cards: BTreeMap::new(),
            meta: meta(),
        };
        let batch = EngagementPayload {
            previews: vec![
                preview(&a, ReactionKind::Likes, vec![person(3), person(2)], true),
                preview(&b, ReactionKind::Likes, vec![], false),
                preview(&a, ReactionKind::Recasts, vec![person(5)], false),
            ],
            meta: meta(),
        };
        let paths: Vec<String> = batch.by_path().map(|(path, _)| path).collect();
        assert_eq!(
            paths,
            [
                reactions_path(&a, ReactionKind::Likes, 10),
                reactions_path(&b, ReactionKind::Likes, 10),
                reactions_path(&a, ReactionKind::Recasts, 10),
            ]
        );

        // On the wire, each preview is what /api/v1/reactions?…&limit=10
        // answers: filed under its own path, it reads back as that payload.
        let json = serde_json::to_value(&batch).unwrap();
        let single = serde_json::json!({
            "hash": a, "kind": "likes", "more": true,
            "users": [{ "fid": 3, "username": "user3" }, { "fid": 2, "username": "user2" }],
            "meta": { "backend": "hypersnap", "fetched_at": 1_792_235_681_000_i64 },
        });
        assert_eq!(json["previews"][0], single);
        for (path, preview) in batch.by_path() {
            let filed = serde_json::to_string(preview).unwrap();
            let read: ReactionsPayload = serde_json::from_str(&filed).unwrap();
            assert_eq!(&read, preview, "{path}");
        }
        let back: EngagementPayload = serde_json::from_value(json).unwrap();
        assert_eq!(back, batch);

        // A batch with nothing in it still parses.
        let empty: EngagementPayload =
            serde_json::from_str(r#"{"meta":{"backend":"hypersnap","fetched_at":0}}"#).unwrap();
        assert!(empty.previews.is_empty());
    }

    #[test]
    fn a_post_payload_previews_its_replies() {
        let cast = |hash: &str, likes: u64| -> Cast {
            serde_json::from_value(serde_json::json!({
                "hash": hash, "author": { "fid": 1, "username": "a" }, "text": format!("text of {hash}"),
                "ts": 1_792_235_681, "likes": likes,
            }))
            .unwrap()
        };
        let node = |cast: Cast, children: Vec<ThreadNode>| ThreadNode { cast, children };
        let mut emptied = cast("0xemptied", 40);
        emptied.mini_app = true;
        emptied.strip_hidden();
        let notes = Annotations {
            links: BTreeMap::from([("https://game.example/".to_owned(), true)]),
            pending: 3,
            ..Default::default()
        };
        let post = CastPayload {
            thread: node(
                cast("0xroot", 0),
                vec![
                    node(cast("0xquiet", 1), vec![node(cast("0xdeep", 99), vec![])]),
                    node(emptied, vec![node(cast("0xbelow", 0), vec![])]),
                    node(cast("0xliked", 7), vec![]),
                ],
            ),
            parent: None,
            quotes: vec![],
            channel: None,
            notes,
            meta: Meta { backend: Backend::Hypersnap, fetched_at: 5, stale: true },
        };
        let preview = RepliesPayload::from_post(&post);
        assert_eq!(preview.hash, "0xroot");
        // Direct replies only, most engaged first, the emptied mini app left out.
        let hashes: Vec<&str> = preview.replies.iter().map(|c| c.hash.as_str()).collect();
        assert_eq!(hashes, ["0xliked", "0xquiet"]);
        // What /api/v1/replies carries: the link verdicts, and the post's age.
        assert_eq!(preview.notes, Annotations { links: post.notes.links.clone(), ..Default::default() });
        assert_eq!(preview.meta, post.meta);
    }
}
