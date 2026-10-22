// SPDX-License-Identifier: AGPL-3.0-or-later
//! A signed-in reader's front page: the casts of the accounts they follow,
//! which the app ranks hot or top as it ranks the trending pool
//! (`ranking::sorted_for`, with the reader's favorites and those they see
//! less of). The Worker reads them from Hypersnap's `feed/following`, which
//! joins the reader's follows itself, so a follow list of any size costs one
//! read; it is newest first and mostly replies, which the front page leaves
//! out. Where they follow few accounts that cast lately, the page is topped
//! up with trending casts, listed after theirs under a label of their own.

use serde::{Deserialize, Serialize};

use crate::api::{API_PREFIX, Annotations, CachePolicy, Meta};
use crate::model::Cast;

/// Where the app reads it.
pub const FOLLOWING_PATH: &str = "/api/v1/following";

/// The query that shows a signed-in reader the trending feed instead.
pub const EVERYONE_PARAM: &str = "feed";
/// Its value.
pub const EVERYONE: &str = "everyone";

/// A first page with fewer top-level casts than this reads Hypersnap's next
/// page too (two at most: each is about 200 KB to parse).
pub const SECOND_PAGE_BELOW: usize = 30;

/// A first page with fewer than this, after both reads, is topped up with
/// trending casts.
pub const TOP_UP_BELOW: usize = 10;

/// How many trending casts top it up at most.
pub const TOP_UP: usize = 40;

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
    /// On a first page with fewer than `TOP_UP_BELOW` casts: trending casts
    /// by others, listed after the reader's own under a label.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub topped_up: Vec<Cast>,
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

/// Whether a first page with `top_level` casts is topped up.
pub fn wants_top_up(first: bool, top_level: usize) -> bool {
    first && top_level < TOP_UP_BELOW
}

/// Trending casts to list after the reader's own: none of theirs again, and
/// no reply, `TOP_UP` at most, in the order given.
pub fn top_up(theirs: &[Cast], trending: Vec<Cast>) -> Vec<Cast> {
    let mut seen: std::collections::HashSet<String> = theirs.iter().map(|c| c.hash.clone()).collect();
    trending.into_iter().filter(|c| !c.is_reply() && seen.insert(c.hash.clone())).take(TOP_UP).collect()
}

/// Whether the front page shows `signed_in` reader the feed of whom they
/// follow, given the page's `feed` query.
pub fn shows_following(signed_in: bool, feed: Option<&str>) -> bool {
    signed_in && feed != Some(EVERYONE)
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
    fn a_sparse_page_reads_more_then_tops_up() {
        assert!(wants_second_page(true, 22, Some("1")));
        assert!(!wants_second_page(true, 22, None));
        assert!(!wants_second_page(true, 30, Some("1")));
        assert!(!wants_second_page(false, 0, Some("1")));
        assert!(wants_top_up(true, 9));
        assert!(!wants_top_up(true, 10));
        assert!(!wants_top_up(false, 0));
    }

    #[test]
    fn top_up_leaves_out_theirs_and_replies() {
        let theirs = vec![cast("0xa", None)];
        let trending = vec![cast("0xa", None), cast("0xb", Some("0xp")), cast("0xc", None), cast("0xc", None)];
        let hashes: Vec<String> = top_up(&theirs, trending).into_iter().map(|c| c.hash).collect();
        assert_eq!(hashes, ["0xc"]);
        let many = (0..100).map(|i| cast(&format!("0x{i}"), None)).collect();
        assert_eq!(top_up(&[], many).len(), TOP_UP);
    }

    #[test]
    fn only_a_signed_in_reader_sees_following() {
        assert!(shows_following(true, None));
        assert!(!shows_following(true, Some(EVERYONE)));
        assert!(!shows_following(false, None));
    }

    #[test]
    fn cache_policy_is_private_and_ordered() {
        let p = FOLLOWING_CACHE;
        assert!(p.fresh <= p.stale && p.stale <= p.keep && p.browser == 0);
    }
}
