// SPDX-License-Identifier: AGPL-3.0-or-later
//! A signed-in reader's front page: the casts of the accounts they follow,
//! which the app ranks hot or top as it ranks the trending pool
//! (`ranking::sorted_for`, with the reader's favorites and those they see
//! less of). The Worker reads them from Hypersnap's `feed/following`, which
//! joins the reader's follows itself, so a follow list of any size costs one
//! read; it is newest first and mostly replies, which the front page leaves
//! out. It is the front page's **Following** tab (`?feed=following`), and
//! holds only their casts: no trending casts top it up. The other tab, **For
//! you**, is the trending pool with the casts of those they follow lifted
//! (`ranking::FOLLOWED_HOT_WEIGHT`), and the authors of this payload's casts
//! are how the app knows whom (`Followed`).

use serde::{Deserialize, Serialize};

use crate::api::{API_PREFIX, Annotations, CachePolicy, Meta};
use crate::model::Cast;

/// Where the app reads it.
pub const FOLLOWING_PATH: &str = "/api/v1/following";

/// The front page's query naming its tab.
pub const FEED_PARAM: &str = "feed";
/// Its value for the Following tab. Any other, or none, is For you, so the
/// old `?feed=everyone` links open For you, as they opened the trending
/// pool.
pub const FOLLOWING: &str = "following";

/// The front page's two tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Feed {
    /// The trending pool, with the casts of whom the reader follows lifted.
    ForYou,
    /// Only the casts of whom the reader follows.
    Following,
}

impl Feed {
    /// The tab a reader sees for the page's `feed` query: Following only
    /// signed in and asked for; signed out, For you is the only one.
    pub fn of(signed_in: bool, param: Option<&str>) -> Feed {
        if signed_in && param == Some(FOLLOWING) { Feed::Following } else { Feed::ForYou }
    }

    /// The `feed` query that keeps this tab: none for For you.
    pub fn param(self) -> Option<String> {
        match self {
            Feed::ForYou => None,
            Feed::Following => Some(FOLLOWING.to_owned()),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Feed::ForYou => "For you",
            Feed::Following => "Following",
        }
    }
}

/// A first page with fewer top-level casts than this reads Hypersnap's next
/// page too (two at most: each is about 200 KB to parse).
pub const SECOND_PAGE_BELOW: usize = 30;

/// Hypersnap's cursors for this feed are a number; anything else is refused.
const MAX_CURSOR: usize = 24;

/// A reader's own: fresh a minute, as the newest casts of whom they follow
/// change by the minute, and never kept by a browser or a shared cache
/// (`private, no-store`); the app keeps it as long as it's fresh.
pub const FOLLOWING_CACHE: CachePolicy = CachePolicy { fresh: 60, stale: 600, keep: 86_400, browser: 0 };

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowingPayload {
    /// Top-level casts by the accounts the reader follows, newest first, with
    /// mini apps and spam left out.
    pub pool: Vec<Cast>,
    #[serde(default)]
    pub notes: Annotations,
    pub meta: Meta,
    /// The cursor of the next, older page, if there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

/// The accounts a page shows the reader follows: the authors of its casts,
/// which are the ones who cast lately, the ones For you can lift.
pub fn followed_authors(payload: &FollowingPayload) -> std::collections::BTreeSet<u64> {
    payload.pool.iter().map(|c| c.author.fid).collect()
}

/// The path of a page: the first without a cursor.
pub fn following_path(cursor: Option<&str>) -> String {
    match cursor {
        Some(cursor) => format!("{API_PREFIX}/following?cursor={cursor}"),
        None => FOLLOWING_PATH.to_owned(),
    }
}

/// A cursor from `?cursor=`, if it is one Hypersnap could have given.
pub fn cursor_from_param(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= MAX_CURSOR && value.bytes().all(|b| b.is_ascii_digit()))
        .then(|| value.to_owned())
}

/// Whether a first page with `top_level` casts reads the next page too.
pub fn wants_second_page(first: bool, top_level: usize, next: Option<&str>) -> bool {
    first && top_level < SECOND_PAGE_BELOW && next.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::UserSummary;

    fn cast(hash: &str, parent: Option<&str>) -> Cast {
        Cast {
            hash: hash.to_owned(),
            author: UserSummary { fid: 1, username: "a".into(), display_name: String::new(), pfp_url: None },
            text: String::new(),
            mentions: Vec::new(),
            ts: 1_792_000_000,
            parent_hash: parent.map(str::to_owned),
            parent_url: None,
            channel: None,
            embeds: Vec::new(),
            likes: 0,
            recasts: 0,
            replies: 0,
            mini_app: false,
            spam: false,
        }
    }

    #[test]
    fn paths_and_cursors() {
        assert_eq!(following_path(None), "/api/v1/following");
        assert_eq!(following_path(Some("183148482")), "/api/v1/following?cursor=183148482");
        assert_eq!(cursor_from_param(" 183148482 ").as_deref(), Some("183148482"));
        assert_eq!(cursor_from_param(""), None);
        assert_eq!(cursor_from_param("12&fid=3"), None);
        assert_eq!(cursor_from_param(&"9".repeat(25)), None);
    }

    #[test]
    fn a_sparse_page_reads_more() {
        assert!(wants_second_page(true, 22, Some("1")));
        assert!(!wants_second_page(true, 22, None));
        assert!(!wants_second_page(true, 30, Some("1")));
        assert!(!wants_second_page(false, 0, Some("1")));
    }

    #[test]
    fn the_tab_is_for_you_unless_a_signed_in_reader_asks_for_following() {
        assert_eq!(Feed::of(true, Some(FOLLOWING)), Feed::Following);
        assert_eq!(Feed::of(true, None), Feed::ForYou);
        // The old link to everyone's trending casts.
        assert_eq!(Feed::of(true, Some("everyone")), Feed::ForYou);
        assert_eq!(Feed::of(false, Some(FOLLOWING)), Feed::ForYou);
        assert_eq!(Feed::Following.param().as_deref(), Some(FOLLOWING));
        assert_eq!(Feed::ForYou.param(), None);
    }

    #[test]
    fn followed_authors_are_the_pools() {
        let mut b = cast("0xb", None);
        b.author.fid = 7;
        let payload = FollowingPayload {
            pool: vec![cast("0xa", None), b],
            notes: Annotations::default(),
            meta: Meta::default(),
            next: None,
        };
        assert_eq!(followed_authors(&payload), std::collections::BTreeSet::from([1, 7]));
    }

    #[test]
    fn cache_policy_is_private_and_ordered() {
        let p = FOLLOWING_CACHE;
        assert!(p.fresh <= p.stale && p.stale <= p.keep && p.browser == 0);
    }
}
