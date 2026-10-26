// SPDX-License-Identifier: AGPL-3.0-or-later
//! Private Library state and durable offline intent. The server owns revisions;
//! a disconnected reader cannot turn an old bookmark/read back on after a reset.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub fid: u64,
    pub generation: u64,
    pub revision: u64,
    pub seen_count: u64,
    pub bookmark_count: u64,
    pub cleared_at: i64,
    pub last_clear_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub fid: u64,
    pub generation: u64,
    pub hash: String,
    pub seen: bool,
    pub seen_at: i64,
    pub seen_revision: u64,
    pub bookmarked: bool,
    pub bookmark_at: i64,
    pub bookmark_revision: u64,
    #[serde(default)]
    pub cursor: String,
}

pub fn canonical_hash(value: &str) -> Option<String> {
    (value.len() == 42 && value.starts_with("0x") && value[2..].bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadIntent {
    Seen { at: i64 },
    Unread,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRead {
    pub generation: u64,
    pub expected_revision: u64,
    pub intent: ReadIntent,
    /// Distinguishes a newer local action from the write being acknowledged.
    pub sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingBookmark {
    pub desired: bool,
    pub expected_revision: u64,
    pub sequence: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedItem {
    pub server: Item,
    /// False until a caller-scoped membership/page response established this row.
    pub known: bool,
    pub read: Option<PendingRead>,
    pub bookmark: Option<PendingBookmark>,
    #[serde(default)]
    pub read_settled: u64,
    #[serde(default)]
    pub bookmark_settled: u64,
}

impl CachedItem {
    fn accepts(&self, row: &Item, account: &Account) -> bool {
        row.fid == account.fid
            && canonical_hash(&row.hash).as_ref() == Some(&row.hash)
            && ((!self.known && self.server.hash.is_empty())
                || (self.server.fid == row.fid && self.server.hash == row.hash))
    }

    pub fn seen(&self, generation: u64) -> bool {
        match self.read.as_ref().filter(|read| read.generation == generation) {
            Some(PendingRead { intent: ReadIntent::Seen { .. }, .. }) => true,
            Some(PendingRead { intent: ReadIntent::Unread, .. }) => false,
            None => self.known && self.server.generation == generation && self.server.seen,
        }
    }

    pub fn bookmarked(&self) -> Option<bool> {
        self.bookmark.as_ref().map(|intent| intent.desired).or_else(|| self.known.then_some(self.server.bookmarked))
    }

    /// Merge only newer authoritative state, independently for read and bookmark
    /// revisions. View caches can temporarily contain overlapping argument sets.
    pub fn observe(&mut self, row: Item, account: &Account) {
        if !self.accepts(&row, account) {
            return;
        }
        if !self.known {
            self.server = row.clone();
            self.known = true;
        }
        if row.bookmark_revision >= self.server.bookmark_revision {
            self.server.bookmarked = row.bookmarked;
            self.server.bookmark_at = row.bookmark_at;
            self.server.bookmark_revision = row.bookmark_revision;
        }
        if row.generation == account.generation
            && row.generation >= self.server.generation
            && (self.server.generation != row.generation || row.seen_revision >= self.server.seen_revision)
        {
            self.server.generation = row.generation;
            self.server.seen = row.seen;
            self.server.seen_at = row.seen_at;
            self.server.seen_revision = row.seen_revision;
        }
        self.reconcile(account);
    }

    /// Apply a fresh row only after this exact sent operation succeeded. A
    /// newer local action may have replaced it while the reducer was in flight.
    /// Rebase that action only across our own single revision, never across an
    /// unrelated server change or a clear. The controller must defer ordinary
    /// observations for an in-flight item until its outcome is known; otherwise
    /// `observe` correctly treats an unexplained revision as a remote conflict.
    pub fn accepted_read(&mut self, sent: &PendingRead, row: Item, account: &Account) {
        if !self.accepts(&row, account) {
            return;
        }
        let own_revision = row.seen_revision == sent.expected_revision
            || sent.expected_revision.checked_add(1) == Some(row.seen_revision);
        let matches = match sent.intent {
            ReadIntent::Seen { at } => row.seen && (row.seen_revision == sent.expected_revision || row.seen_at <= at),
            ReadIntent::Unread => !row.seen,
        };
        if own_revision
            && matches
            && sent.generation == account.generation
            && row.generation == account.generation
            && let Some(pending) = self.read.as_mut()
            && pending.sequence > sent.sequence
            && pending.generation == sent.generation
            && pending.expected_revision == sent.expected_revision
        {
            pending.expected_revision = row.seen_revision;
        }
        self.acknowledged_read(sent.sequence);
        self.observe(row, account);
    }

    /// Bookmark counterpart of `accepted_read`. A successful same-value no-op
    /// is safe too, provided no intervening revision has changed the row.
    pub fn accepted_bookmark(&mut self, sent: &PendingBookmark, row: Item, account: &Account) {
        if !self.accepts(&row, account) {
            return;
        }
        let own_revision = row.bookmark_revision == sent.expected_revision
            || sent.expected_revision.checked_add(1) == Some(row.bookmark_revision);
        if own_revision
            && row.bookmarked == sent.desired
            && let Some(pending) = self.bookmark.as_mut()
            && pending.sequence > sent.sequence
            && pending.expected_revision == sent.expected_revision
        {
            pending.expected_revision = row.bookmark_revision;
        }
        self.acknowledged_bookmark(sent.sequence);
        self.observe(row, account);
    }

    pub fn reconcile(&mut self, account: &Account) {
        let generation = if self.known { account.generation.max(self.server.generation) } else { account.generation };
        if self.read.as_ref().is_some_and(|read| {
            read.generation != generation
                || (self.known
                    && self.server.generation == generation
                    && self.server.seen_revision > read.expected_revision)
                || (account.generation == generation
                    && matches!(read.intent, ReadIntent::Seen { at } if at <= account.cleared_at))
        }) && let Some(read) = self.read.take()
        {
            self.read_settled = self.read_settled.max(read.sequence);
        }
        // A later server operation wins over disconnected intent. An identical
        // value is already complete; a different one is a conflict, never replayed.
        if self.bookmark.as_ref().is_some_and(|intent| {
            self.known
                && (self.server.bookmark_revision > intent.expected_revision
                    || self.server.bookmarked == intent.desired)
        }) && let Some(intent) = self.bookmark.take()
        {
            self.bookmark_settled = self.bookmark_settled.max(intent.sequence);
        }
    }

    pub fn acknowledged_read(&mut self, sequence: u64) {
        self.read_settled = self.read_settled.max(sequence);
        if self.read.as_ref().is_some_and(|read| read.sequence <= sequence) {
            self.read = None;
        }
    }

    pub fn acknowledged_bookmark(&mut self, sequence: u64) {
        self.bookmark_settled = self.bookmark_settled.max(sequence);
        if self.bookmark.as_ref().is_some_and(|intent| intent.sequence <= sequence) {
            self.bookmark = None;
        }
    }

    pub fn pending(&self) -> bool {
        self.read.is_some() || self.bookmark.is_some()
    }

    /// Merge inside an IndexedDB read/write transaction. This also protects
    /// against another tab committing an older snapshot after an acknowledgement.
    pub fn merge_disk(&mut self, incoming: &Self) {
        self.merge_disk_preserving_inflight(incoming, None, None);
    }

    /// Preserve a queued successor while an older operation is still in flight.
    /// The sender must finish with `accepted_read`/`accepted_bookmark` and a fresh
    /// row, or observe the authoritative failure state. This protection never
    /// overrides explicit settlement or a history-generation reset.
    pub fn merge_disk_preserving_inflight(
        &mut self,
        incoming: &Self,
        read_sequence: Option<u64>,
        bookmark_sequence: Option<u64>,
    ) {
        if canonical_hash(&incoming.server.hash).as_ref() != Some(&incoming.server.hash) {
            return;
        }
        if self.server.hash.is_empty() {
            self.server = incoming.server.clone();
        }
        if self.server.fid != incoming.server.fid || self.server.hash != incoming.server.hash {
            return;
        }
        if incoming.known {
            if !self.known || incoming.server.bookmark_revision >= self.server.bookmark_revision {
                self.server.bookmarked = incoming.server.bookmarked;
                self.server.bookmark_at = incoming.server.bookmark_at;
                self.server.bookmark_revision = incoming.server.bookmark_revision;
            }
            if !self.known
                || incoming.server.generation > self.server.generation
                || (incoming.server.generation == self.server.generation
                    && incoming.server.seen_revision >= self.server.seen_revision)
            {
                self.server.generation = incoming.server.generation;
                self.server.seen = incoming.server.seen;
                self.server.seen_at = incoming.server.seen_at;
                self.server.seen_revision = incoming.server.seen_revision;
            }
            self.known = true;
        }
        self.read_settled = self.read_settled.max(incoming.read_settled);
        self.bookmark_settled = self.bookmark_settled.max(incoming.bookmark_settled);
        self.read = self
            .read
            .iter()
            .chain(incoming.read.iter())
            .filter(|intent| intent.sequence > self.read_settled)
            .max_by_key(|intent| (intent.sequence, intent.expected_revision))
            .cloned();
        self.bookmark = self
            .bookmark
            .iter()
            .chain(incoming.bookmark.iter())
            .filter(|intent| intent.sequence > self.bookmark_settled)
            .max_by_key(|intent| (intent.sequence, intent.expected_revision))
            .cloned();
        if self.known {
            if let Some(read) = self.read.as_ref()
                && (read.generation < self.server.generation
                    || (read.generation == self.server.generation
                        && read.expected_revision < self.server.seen_revision
                        && !read_sequence.is_some_and(|sent| read.sequence >= sent)))
            {
                self.acknowledged_read(read.sequence);
            }
            if let Some(bookmark) = self.bookmark.as_ref()
                && bookmark.expected_revision < self.server.bookmark_revision
                && !bookmark_sequence.is_some_and(|sent| bookmark.sequence >= sent)
            {
                self.acknowledged_bookmark(bookmark.sequence);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> Item {
        Item { fid: 3, hash: format!("0x{:040x}", 9), ..Item::default() }
    }

    fn account() -> Account {
        Account { fid: 3, ..Account::default() }
    }

    #[test]
    fn old_membership_cannot_undo_newer_bookmark_or_read_state() {
        let mut cache = CachedItem::default();
        let row = Item {
            seen: true,
            seen_at: 200,
            seen_revision: 5,
            bookmarked: true,
            bookmark_at: 201,
            bookmark_revision: 9,
            ..item()
        };
        cache.observe(row.clone(), &account());
        cache.observe(item(), &account());
        assert_eq!(cache.server, row);
        assert_eq!(cache.bookmarked(), Some(true));
        assert!(cache.seen(0));
    }

    #[test]
    fn a_remote_clear_fences_all_older_offline_observations() {
        let mut cache = CachedItem {
            known: true,
            server: item(),
            read: Some(PendingRead {
                generation: 0,
                expected_revision: 0,
                intent: ReadIntent::Seen { at: 150 },
                sequence: 1,
            }),
            ..CachedItem::default()
        };
        let account = Account { generation: 1, cleared_at: 200, ..account() };
        cache.reconcile(&account);
        assert!(cache.read.is_none());
        assert!(!cache.seen(1));
    }

    #[test]
    fn per_item_tombstones_reject_stale_offline_reads_and_bookmarks() {
        let mut cache = CachedItem {
            read: Some(PendingRead {
                generation: 0,
                expected_revision: 2,
                intent: ReadIntent::Seen { at: 150 },
                sequence: 1,
            }),
            bookmark: Some(PendingBookmark { desired: true, expected_revision: 4, sequence: 2 }),
            ..CachedItem::default()
        };
        cache.observe(Item { seen_revision: 3, bookmark_revision: 5, ..item() }, &account());
        assert!(!cache.pending());
        assert!(!cache.seen(0));
        assert_eq!(cache.bookmarked(), Some(false));
    }

    #[test]
    fn a_lost_acknowledgement_never_removes_a_newer_local_action() {
        let mut cache = CachedItem {
            read: Some(PendingRead { generation: 0, expected_revision: 0, intent: ReadIntent::Unread, sequence: 2 }),
            bookmark: Some(PendingBookmark { desired: false, expected_revision: 0, sequence: 3 }),
            ..CachedItem::default()
        };
        cache.acknowledged_read(1);
        cache.acknowledged_bookmark(2);
        assert!(cache.pending());
        cache.acknowledged_read(2);
        cache.acknowledged_bookmark(3);
        assert!(!cache.pending());
    }

    #[test]
    fn initial_unknown_state_does_not_claim_a_post_is_unbookmarked() {
        let mut cache = CachedItem::default();
        assert_eq!(cache.bookmarked(), None);
        cache.observe(item(), &account());
        assert_eq!(cache.bookmarked(), Some(false));
        assert!(!cache.pending());
    }

    #[test]
    fn other_accounts_and_noncanonical_rows_never_enter_the_cache() {
        let mut cache = CachedItem::default();
        cache.observe(Item { fid: 4, ..item() }, &account());
        cache.observe(Item { hash: "bad".into(), ..item() }, &account());
        assert!(!cache.known);
        assert_eq!(canonical_hash(&format!("0x{:040X}", 10)), Some(format!("0x{:040x}", 10)));
        assert_eq!(canonical_hash("x"), None);
    }

    #[test]
    fn late_disk_snapshots_cannot_resurrect_settled_or_replace_newer_intent() {
        let mut old = CachedItem {
            server: item(),
            bookmark: Some(PendingBookmark { desired: true, expected_revision: 0, sequence: 1 }),
            ..CachedItem::default()
        };
        let mut newer = old.clone();
        newer.bookmark = Some(PendingBookmark { desired: false, expected_revision: 0, sequence: 2 });
        old.merge_disk(&newer);
        assert_eq!(old.bookmark.as_ref().unwrap().sequence, 2);
        let stale = old.clone();
        old.acknowledged_bookmark(2);
        old.merge_disk(&stale);
        assert!(old.bookmark.is_none());
        assert_eq!(old.bookmark_settled, 2);
    }

    #[test]
    fn persisted_remote_tombstones_also_fence_another_tabs_outbox() {
        let mut pending = CachedItem {
            server: item(),
            read: Some(PendingRead {
                generation: 0,
                expected_revision: 0,
                intent: ReadIntent::Seen { at: 10 },
                sequence: 1,
            }),
            ..CachedItem::default()
        };
        let remote = CachedItem { known: true, server: Item { seen_revision: 2, ..item() }, ..CachedItem::default() };
        pending.merge_disk(&remote);
        assert!(pending.read.is_none());
        assert_eq!(pending.read_settled, 1);
    }

    #[test]
    fn accepted_add_preserves_a_newer_remove_through_observe_and_disk_merge() {
        let sent = PendingBookmark { desired: true, expected_revision: 0, sequence: 1 };
        let mut cache = CachedItem {
            known: true,
            server: item(),
            bookmark: Some(PendingBookmark { desired: false, expected_revision: 0, sequence: 2 }),
            ..CachedItem::default()
        };
        let stale_disk = cache.clone();
        cache.accepted_bookmark(
            &sent,
            Item { bookmarked: true, bookmark_at: 150, bookmark_revision: 1, ..item() },
            &account(),
        );
        assert_eq!(cache.bookmarked(), Some(false));
        assert_eq!(cache.bookmark.as_ref().unwrap().expected_revision, 1);
        cache.merge_disk(&stale_disk);
        assert_eq!(cache.bookmark.as_ref().unwrap().expected_revision, 1);
        assert_eq!(cache.bookmark.as_ref().unwrap().sequence, 2);
        cache.observe(Item { bookmark_revision: 2, ..item() }, &account());
        assert!(cache.bookmark.is_none());
        assert_eq!(cache.bookmark_settled, 2);
    }

    #[test]
    fn accepted_seen_preserves_a_newer_unread_through_observe_and_disk_merge() {
        let sent =
            PendingRead { generation: 0, expected_revision: 0, intent: ReadIntent::Seen { at: 150 }, sequence: 1 };
        let mut cache = CachedItem {
            known: true,
            server: item(),
            read: Some(PendingRead { generation: 0, expected_revision: 0, intent: ReadIntent::Unread, sequence: 2 }),
            ..CachedItem::default()
        };
        let stale_disk = cache.clone();
        cache.accepted_read(&sent, Item { seen: true, seen_at: 150, seen_revision: 1, ..item() }, &account());
        assert!(!cache.seen(0));
        assert_eq!(cache.read.as_ref().unwrap().expected_revision, 1);
        cache.merge_disk(&stale_disk);
        assert_eq!(cache.read.as_ref().unwrap().expected_revision, 1);
        cache.observe(Item { seen_revision: 2, ..item() }, &account());
        assert!(cache.read.is_none());
        assert_eq!(cache.read_settled, 2);
    }

    #[test]
    fn accepted_same_value_noop_does_not_drop_a_newer_action() {
        let mut cache = CachedItem {
            known: true,
            server: Item { bookmarked: true, bookmark_revision: 4, ..item() },
            bookmark: Some(PendingBookmark { desired: false, expected_revision: 4, sequence: 2 }),
            ..CachedItem::default()
        };
        let row = cache.server.clone();
        cache.accepted_bookmark(&PendingBookmark { desired: true, expected_revision: 4, sequence: 1 }, row, &account());
        assert_eq!(cache.bookmark.as_ref().unwrap().sequence, 2);
        assert_eq!(cache.bookmark_settled, 1);
    }

    #[test]
    fn successful_old_calls_never_rebase_across_foreign_changes_or_clears() {
        let sent = PendingBookmark { desired: true, expected_revision: 0, sequence: 1 };
        let mut cache = CachedItem {
            known: true,
            server: item(),
            bookmark: Some(PendingBookmark { desired: true, expected_revision: 0, sequence: 2 }),
            ..CachedItem::default()
        };
        cache.accepted_bookmark(&sent, Item { bookmark_revision: 2, ..item() }, &account());
        assert!(cache.bookmark.is_none());
        assert_eq!(cache.bookmarked(), Some(false));
        cache.read = Some(PendingRead {
            generation: 0,
            expected_revision: 0,
            intent: ReadIntent::Seen { at: 150 },
            sequence: 4,
        });
        cache.accepted_read(
            &PendingRead { generation: 0, expected_revision: 0, intent: ReadIntent::Seen { at: 100 }, sequence: 3 },
            Item { generation: 1, ..item() },
            &Account { generation: 1, cleared_at: 200, ..account() },
        );
        assert!(cache.read.is_none());
        assert!(!cache.seen(1));
    }

    #[test]
    fn existing_rows_cannot_mix_accounts_even_when_the_cast_hash_matches() {
        for known in [false, true] {
            let mut cache = CachedItem { known, server: item(), ..CachedItem::default() };
            cache.observe(
                Item { fid: 4, bookmarked: true, bookmark_revision: 9, ..item() },
                &Account { fid: 4, ..account() },
            );
            assert_eq!(cache.server, item());
            assert_eq!(cache.known, known);
        }
    }

    #[test]
    fn cached_equal_value_is_not_an_ack_for_a_queued_inverse() {
        let mut cache = CachedItem {
            known: true,
            server: item(),
            bookmark: Some(PendingBookmark { desired: false, expected_revision: 0, sequence: 2 }),
            ..CachedItem::default()
        };
        cache.merge_disk(&CachedItem { known: true, server: item(), ..CachedItem::default() });
        assert_eq!(cache.bookmark.as_ref().unwrap().sequence, 2);
    }

    #[test]
    fn inflight_disk_merge_waits_for_provenance_but_never_defeats_a_clear() {
        let sent =
            PendingRead { generation: 0, expected_revision: 0, intent: ReadIntent::Seen { at: 150 }, sequence: 1 };
        let mut cache = CachedItem {
            known: true,
            server: item(),
            read: Some(PendingRead { generation: 0, expected_revision: 0, intent: ReadIntent::Unread, sequence: 2 }),
            ..CachedItem::default()
        };
        let fresh = Item { seen: true, seen_at: 150, seen_revision: 1, ..item() };
        cache.merge_disk_preserving_inflight(
            &CachedItem { known: true, server: fresh.clone(), ..CachedItem::default() },
            Some(1),
            None,
        );
        assert_eq!(cache.read.as_ref().unwrap().expected_revision, 0);
        cache.accepted_read(&sent, fresh, &account());
        assert_eq!(cache.read.as_ref().unwrap().expected_revision, 1);
        cache.merge_disk_preserving_inflight(
            &CachedItem { known: true, server: Item { generation: 1, ..item() }, ..CachedItem::default() },
            Some(1),
            None,
        );
        assert!(cache.read.is_none());
    }

    #[test]
    fn stale_account_and_membership_cannot_roll_back_a_known_reset() {
        let mut cache = CachedItem {
            known: true,
            server: Item { generation: 1, ..item() },
            read: Some(PendingRead {
                generation: 1,
                expected_revision: 0,
                intent: ReadIntent::Seen { at: 250 },
                sequence: 2,
            }),
            ..CachedItem::default()
        };
        cache.observe(Item { seen: true, seen_at: 150, seen_revision: 10, ..item() }, &account());
        assert_eq!(cache.server.generation, 1);
        assert!(!cache.server.seen);
        assert_eq!(cache.read.as_ref().unwrap().generation, 1);
        cache.reconcile(&Account { generation: 1, cleared_at: 200, ..account() });
        assert!(cache.seen(1));
    }
}
