// SPDX-License-Identifier: AGPL-3.0-or-later
//! Bounded, browser-local reading history. Only cast hashes and seen times;
//! never cast contents, authors, a server payload, or an inferred impression.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub const MAX_POSTS: usize = 2_000;
pub const MAX_SCOPES: usize = 4;
pub const KEEP_SECS: i64 = 30 * 86_400;
const MAX_BYTES: usize = 1_000_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Post {
    hash: String,
    at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Scope {
    name: String,
    touched: i64,
    #[serde(default)]
    reset: u64,
    posts: Vec<Post>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct History {
    version: u8,
    scopes: Vec<Scope>,
}

impl Default for History {
    fn default() -> Self {
        Self { version: 1, scopes: Vec::new() }
    }
}

/// The signed-in FID has its own scope; signed out uses another one.
pub fn scope(fid: Option<u64>) -> String {
    fid.filter(|fid| *fid > 0).map_or_else(|| "anonymous".into(), |fid| format!("fid:{fid}"))
}

fn valid_scope(name: &str) -> bool {
    name == "anonymous"
        || name
            .strip_prefix("fid:")
            .and_then(|fid| fid.parse::<u64>().ok())
            .is_some_and(|fid| fid > 0 && name == scope(Some(fid)))
}

fn hash(value: &str) -> Option<String> {
    (value.len() == 42 && value.starts_with("0x") && value[2..].bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

impl History {
    pub fn read(text: &str, now: i64) -> Self {
        let mut history: Self =
            if text.len() <= MAX_BYTES { serde_json::from_str(text).unwrap_or_default() } else { Self::default() };
        if history.version != 1 {
            return Self::default();
        }
        history.prune(now);
        history
    }

    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.scopes.is_empty()
    }

    pub fn hashes(&self, scope: &str) -> BTreeSet<String> {
        self.scopes.iter().filter(|s| s.name == scope).flat_map(|s| s.posts.iter().map(|p| p.hash.clone())).collect()
    }

    pub fn record(&mut self, name: &str, value: &str, now: i64) {
        self.record_at(name, value, now, now);
    }

    /// Merge a pending observation using the current clock for expiry, not
    /// the earlier observation time (another tab may have newer records).
    pub fn record_at(&mut self, name: &str, value: &str, seen_at: i64, now: i64) {
        if seen_at <= now - KEEP_SECS || seen_at > now {
            self.prune(now);
            return;
        }
        let Some(hash) = hash(value).filter(|_| valid_scope(name)) else { return };
        let at = match self.scopes.iter().position(|s| s.name == name) {
            Some(at) => at,
            None => {
                self.scopes.push(Scope { name: name.into(), touched: seen_at, reset: 0, posts: Vec::new() });
                self.scopes.len() - 1
            }
        };
        let scope = &mut self.scopes[at];
        scope.touched = scope.touched.max(seen_at);
        let at = scope.posts.iter().filter(|p| p.hash == hash).map(|p| p.at).max().unwrap_or(seen_at).max(seen_at);
        scope.posts.retain(|p| p.hash != hash);
        scope.posts.push(Post { hash, at });
        self.prune(now);
    }

    pub fn revision(&self, scope: &str) -> u64 {
        self.scopes.iter().find(|s| s.name == scope).map_or(0, |s| s.reset)
    }

    /// Keep a bounded tombstone so another tab cannot replay impressions
    /// pending before this reset. It occupies the same reader-scope cap.
    pub fn reset(&mut self, name: &str, now: i64) {
        if !valid_scope(name) {
            return;
        }
        let reset = self.revision(name).checked_add(1).unwrap_or(1);
        self.scopes.retain(|s| s.name != name);
        self.scopes.push(Scope { name: name.into(), touched: now, reset, posts: Vec::new() });
        self.prune(now);
    }

    fn prune(&mut self, now: i64) {
        let mut merged = BTreeMap::<String, Scope>::new();
        for scope in std::mem::take(&mut self.scopes).into_iter().filter(|s| valid_scope(&s.name)) {
            let into = merged.entry(scope.name.clone()).or_insert_with(|| Scope {
                name: scope.name,
                touched: 0,
                reset: scope.reset,
                posts: Vec::new(),
            });
            into.touched = into.touched.max(scope.touched.min(now));
            if scope.reset > into.reset {
                into.posts.clear();
                into.reset = scope.reset;
            }
            if scope.reset == into.reset {
                into.posts.extend(scope.posts);
            }
        }
        for scope in merged.values_mut() {
            let mut posts = BTreeMap::<String, i64>::new();
            for post in &scope.posts {
                if post.at > now - KEEP_SECS
                    && post.at <= now
                    && let Some(hash) = hash(&post.hash)
                {
                    posts.entry(hash).and_modify(|at| *at = (*at).max(post.at)).or_insert(post.at);
                }
            }
            scope.posts = posts.into_iter().map(|(hash, at)| Post { hash, at }).collect();
            scope.posts.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.hash.cmp(&b.hash)));
            scope.posts.truncate(MAX_POSTS);
        }
        self.scopes = merged
            .into_values()
            .filter(|s| !s.posts.is_empty() || (s.reset > 0 && s.touched > now - KEEP_SECS))
            .collect();
        self.scopes.sort_by(|a, b| b.touched.cmp(&a.touched).then_with(|| a.name.cmp(&b.name)));
        self.scopes.truncate(MAX_SCOPES);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cast(n: usize) -> String {
        format!("0x{n:040x}")
    }

    #[test]
    fn scopes_do_not_share_history_and_reset_only_clears_one() {
        let mut h = History::default();
        for fid in [None, Some(1), Some(2)] {
            h.record(&scope(fid), &cast(fid.unwrap_or(0) as usize), 100);
        }
        assert_eq!(h.hashes(&scope(Some(1))), BTreeSet::from([cast(1)]));
        h.reset(&scope(Some(1)), 101);
        assert!(h.hashes(&scope(Some(1))).is_empty());
        assert_eq!(h.hashes(&scope(None)), BTreeSet::from([cast(0)]));
        assert_eq!(h.hashes(&scope(Some(2))), BTreeSet::from([cast(2)]));
    }

    #[test]
    fn only_newest_two_thousand_posts_and_four_scopes_survive() {
        let mut h = History::default();
        for n in 0..MAX_POSTS + 3 {
            h.record("anonymous", &cast(n), n as i64);
        }
        assert_eq!(h.hashes("anonymous").len(), MAX_POSTS);
        assert!(!h.hashes("anonymous").contains(&cast(2)));
        for fid in 1..=4 {
            h.record(&scope(Some(fid)), &cast(0), 3_000 + fid as i64);
        }
        assert_eq!(h.scopes.len(), MAX_SCOPES);
        assert!(h.hashes("anonymous").is_empty());
    }

    #[test]
    fn expiry_and_future_times_are_checked_when_read() {
        let mut h = History::default();
        h.record("anonymous", &cast(1), 100);
        h.record("anonymous", &cast(2), 101);
        h.record("anonymous", &cast(3), 102);
        let read = History::read(&h.encode(), 100 + KEEP_SECS);
        assert_eq!(read.hashes("anonymous"), BTreeSet::from([cast(2), cast(3)]));
        assert!(History::read(&h.encode(), 99).is_empty());
    }

    #[test]
    fn malformed_unknown_and_oversize_storage_fails_open() {
        for input in ["not json", "{}", r#"{"version":2,"scopes":[]}"#] {
            assert!(History::read(input, 100).is_empty());
        }
        assert!(History::read(&" ".repeat(MAX_BYTES + 1), 100).is_empty());
        let mut h = History::default();
        h.record("fid:00", &cast(1), 100);
        h.record("anonymous", "0xzz", 100);
        assert!(h.is_empty());
    }

    #[test]
    fn re_read_deduplicates_case_and_preserves_latest_seen_time() {
        let upper = format!("0x{}", "AB".repeat(20));
        let mut h = History::default();
        h.record("anonymous", &upper, 100);
        h.record("anonymous", &upper.to_ascii_lowercase(), 200);
        assert_eq!(h.hashes("anonymous").len(), 1);
        assert_eq!(History::read(&h.encode(), 100 + KEEP_SECS).hashes("anonymous").len(), 1);
    }

    #[test]
    fn merging_older_pending_observations_keeps_newer_records_and_duplicates() {
        let mut h = History::default();
        h.record("anonymous", &cast(1), 101);
        h.record("anonymous", &cast(2), 102);
        h.record_at("anonymous", &cast(1), 100, 103);
        h.record_at("anonymous", &cast(3), 99, 103);
        assert_eq!(h.hashes("anonymous"), BTreeSet::from([cast(1), cast(2), cast(3)]));
        assert_eq!(h.scopes[0].posts.iter().find(|p| p.hash == cast(1)).unwrap().at, 101);
        assert_eq!(History::read(&h.encode(), 101 + KEEP_SECS).hashes("anonymous"), BTreeSet::from([cast(2)]));
    }

    #[test]
    fn reset_revisions_survive_reading_and_invalidate_old_pending_observations() {
        let mut h = History::default();
        h.record("anonymous", &cast(1), 100);
        let pending_revision = h.revision("anonymous");
        h.reset("anonymous", 101);
        let reset = History::read(&h.encode(), 102);
        assert_ne!(reset.revision("anonymous"), pending_revision);
        assert!(reset.hashes("anonymous").is_empty());
        assert_eq!(reset.revision("fid:1"), 0);
    }

    #[test]
    fn a_feed_snapshot_stays_fixed_as_reading_continues() {
        let mut h = History::default();
        h.record("anonymous", &cast(1), 100);
        let snapshot = h.hashes("anonymous");
        h.record("anonymous", &cast(2), 101);
        assert!(!snapshot.contains(&cast(2)));
        assert!(h.hashes("anonymous").contains(&cast(2)));
    }
}
