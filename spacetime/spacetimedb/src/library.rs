// SPDX-License-Identifier: AGPL-3.0-or-later
//! Durable private reading history and bookmarks. Only bounded indexed views
//! are exposed; an account's complete history is never a bootstrap payload.

use super::*;
use std::collections::BTreeSet;

const MAX_BATCH: usize = 100;
const MAX_PAGE: u32 = 50;

#[derive(Clone, Debug, PartialEq, Eq)]
#[spacetimedb::table(accessor = library_account)]
pub struct LibraryAccount {
    #[primary_key]
    pub fid: u64,
    pub generation: u64,
    pub revision: u64,
    pub seen_count: u64,
    pub bookmark_count: u64,
    pub cleared_at: i64,
    pub last_clear_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[spacetimedb::table(accessor = library_post,
    index(accessor = by_seen, btree(columns = [fid, generation, seen, seen_order])),
    index(accessor = by_bookmark, btree(columns = [fid, bookmarked, bookmark_order])))]
pub struct LibraryPost {
    #[primary_key]
    pub key: String,
    pub fid: u64,
    pub hash: String,
    pub generation: u64,
    pub seen: bool,
    pub seen_at: i64,
    pub seen_revision: u64,
    pub seen_order: String,
    pub bookmarked: bool,
    pub bookmark_at: i64,
    pub bookmark_revision: u64,
    pub bookmark_order: String,
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct SeenInput {
    pub hash: String,
    pub at: i64,
    pub expected_revision: u64,
}

/// Flat rows keep the existing SDK boundary small. `cursor` is empty for a
/// point result and is the exclusive keyset cursor for a paged result.
#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct LibraryItem {
    pub fid: u64,
    pub generation: u64,
    pub hash: String,
    pub seen: bool,
    pub seen_at: i64,
    pub seen_revision: u64,
    pub bookmarked: bool,
    pub bookmark_at: i64,
    pub bookmark_revision: u64,
    pub cursor: String,
}

fn account(fid: u64) -> LibraryAccount {
    LibraryAccount {
        fid,
        generation: 0,
        revision: 0,
        seen_count: 0,
        bookmark_count: 0,
        cleared_at: 0,
        last_clear_id: String::new(),
    }
}

fn hash(value: &str) -> Result<String, String> {
    if value.len() != 42 || !value.starts_with("0x") || !value[2..].bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("cast hash must be 0x followed by 40 hex digits".into());
    }
    Ok(value.to_ascii_lowercase())
}

fn key(fid: u64, hash: &str) -> String {
    format!("{fid}:{hash}")
}

fn increment(value: u64) -> Result<u64, String> {
    value.checked_add(1).ok_or_else(|| "library revision or count exhausted".into())
}

fn decrement(value: u64) -> Result<u64, String> {
    value.checked_sub(1).ok_or_else(|| "library count is inconsistent".into())
}

/// Reverse both components so a forward B-tree scan is DESC(time, hash).
fn order(at: i64, hash: &str) -> String {
    let reversed: String = hash[2..]
        .bytes()
        .map(|b| char::from(b"fedcba9876543210"[char::from(b).to_digit(16).expect("validated hash") as usize]))
        .collect();
    format!("{:020}:{reversed}", u64::MAX - at as u64)
}

fn after_key(cursor: &str) -> Result<String, String> {
    if cursor.len() > 63 {
        return Err("invalid library cursor".into());
    }
    let (at, value) = cursor.split_once(':').ok_or("invalid library cursor")?;
    let at: i64 = at.parse().map_err(|_| "invalid library cursor")?;
    if at < 0 {
        return Err("invalid library cursor".into());
    }
    Ok(order(at, &hash(value)?))
}

fn normalized(state: &LibraryAccount, hash: String, old: Option<LibraryPost>) -> LibraryPost {
    let mut post = old.unwrap_or_else(|| LibraryPost {
        key: key(state.fid, &hash),
        fid: state.fid,
        hash,
        generation: state.generation,
        seen: false,
        seen_at: 0,
        seen_revision: 0,
        seen_order: String::new(),
        bookmarked: false,
        bookmark_at: 0,
        bookmark_revision: 0,
        bookmark_order: String::new(),
    });
    if post.generation != state.generation {
        post.generation = state.generation;
        post.seen = false;
        post.seen_at = 0;
        post.seen_revision = 0;
        post.seen_order.clear();
    }
    post
}

fn item(state: &LibraryAccount, post: LibraryPost, kind: Option<u8>) -> LibraryItem {
    let post = normalized(state, post.hash.clone(), Some(post));
    let cursor = kind.map_or_else(String::new, |kind| {
        format!("{}:{}", if kind == 0 { post.seen_at } else { post.bookmark_at }, post.hash)
    });
    LibraryItem {
        fid: state.fid,
        generation: state.generation,
        hash: post.hash,
        seen: post.seen,
        seen_at: post.seen_at,
        seen_revision: post.seen_revision,
        bookmarked: post.bookmarked,
        bookmark_at: post.bookmark_at,
        bookmark_revision: post.bookmark_revision,
        cursor,
    }
}

#[view(accessor = my_library_state, public)]
fn my_library_state(ctx: &ViewContext) -> Vec<LibraryAccount> {
    let Some(reader) = ctx.db.notification_reader().identity().find(ctx.sender()) else { return Vec::new() };
    vec![ctx.db.library_account().fid().find(reader.fid).unwrap_or_else(|| account(reader.fid))]
}

#[view(accessor = my_library_membership, public)]
fn my_library_membership(ctx: &ViewContext, hashes_csv: String) -> Vec<LibraryItem> {
    let Some(reader) = ctx.db.notification_reader().identity().find(ctx.sender()) else { return Vec::new() };
    let Ok(hashes) = membership_hashes(&hashes_csv) else { return Vec::new() };
    let state = ctx.db.library_account().fid().find(reader.fid).unwrap_or_else(|| account(reader.fid));
    hashes
        .into_iter()
        .map(|hash| {
            let found = ctx.db.library_post().key().find(key(reader.fid, &hash));
            item(&state, normalized(&state, hash, found), None)
        })
        .collect()
}

fn membership_hashes(csv: &str) -> Result<Vec<String>, String> {
    if csv.is_empty() || csv.len() > MAX_BATCH * 43 - 1 {
        return Err("membership requires 1-100 hashes".into());
    }
    let hashes: BTreeSet<_> = csv.split(',').map(hash).collect::<Result<_, _>>()?;
    if hashes.is_empty() || hashes.len() > MAX_BATCH {
        return Err("membership requires 1-100 hashes".into());
    }
    Ok(hashes.into_iter().collect())
}

#[view(accessor = my_library_page, public)]
fn my_library_page(ctx: &ViewContext, kind: u8, generation: u64, after: String, limit: u32) -> Vec<LibraryItem> {
    let Some(reader) = ctx.db.notification_reader().identity().find(ctx.sender()) else { return Vec::new() };
    if kind > 1 || limit == 0 || limit > MAX_PAGE {
        return Vec::new();
    }
    let state = ctx.db.library_account().fid().find(reader.fid).unwrap_or_else(|| account(reader.fid));
    if generation != state.generation {
        return Vec::new();
    }
    let start = if after.is_empty() {
        String::new()
    } else {
        let Ok(mut key) = after_key(&after) else { return Vec::new() };
        // The SDK exposes inclusive RangeFrom but not arbitrary Bound pairs.
        // Every stored key has exactly 61 ASCII bytes: appending NUL makes
        // this lower bound strictly greater than the cursor's complete key,
        // and still less than every subsequent valid complete key.
        key.push('\0');
        key
    };
    let bounds = start.as_str()..;
    let count = limit as usize + 1;
    if kind == 0 {
        ctx.db
            .library_post()
            .by_seen()
            .filter((reader.fid, generation, true, bounds))
            .take(count)
            .map(|post| item(&state, post, Some(kind)))
            .collect()
    } else {
        ctx.db
            .library_post()
            .by_bookmark()
            .filter((reader.fid, true, bounds))
            .take(count)
            .map(|post| item(&state, post, Some(kind)))
            .collect()
    }
}

fn reader(ctx: &ReducerContext) -> Result<u64, String> {
    ctx.db
        .notification_reader()
        .identity()
        .find(ctx.sender())
        .map(|r| r.fid)
        .ok_or_else(|| "verify your Farcaster account first".into())
}

fn save_account(ctx: &ReducerContext, state: LibraryAccount) {
    let table = ctx.db.library_account();
    if table.fid().find(state.fid).is_some() {
        table.fid().update(state);
    } else {
        table.insert(state);
    }
}

fn save_post(ctx: &ReducerContext, post: LibraryPost) {
    let table = ctx.db.library_post();
    if table.key().find(&post.key).is_some() {
        table.key().update(post);
    } else {
        table.insert(post);
    }
}

fn inputs(entries: Vec<SeenInput>, now: i64) -> Result<Vec<SeenInput>, String> {
    if entries.is_empty() || entries.len() > MAX_BATCH {
        return Err("mark seen requires 1-100 entries".into());
    }
    let mut unique = BTreeSet::new();
    entries
        .into_iter()
        .map(|entry| {
            let hash = hash(&entry.hash)?;
            if !unique.insert(hash.clone()) {
                return Err("duplicate seen hash".into());
            }
            if entry.at < 0 || entry.at > now.max(0).saturating_add(60) {
                return Err("seen time is outside the allowed range".into());
            }
            Ok(SeenInput { hash, at: entry.at.min(now.max(0)), ..entry })
        })
        .collect()
}

fn apply_seen(
    state: &mut LibraryAccount,
    old: Option<LibraryPost>,
    entry: SeenInput,
) -> Result<Option<LibraryPost>, String> {
    // No import/replay from before an explicit clear, including a legacy
    // browser that only learns the current generation when it reconnects.
    if state.cleared_at > 0 && entry.at <= state.cleared_at {
        return Ok(None);
    }
    let mut post = normalized(state, entry.hash, old);
    // A removed item's durable revision fences pre-removal observations.
    if entry.expected_revision > post.seen_revision {
        return Err("seen revision changed".into());
    }
    if !post.seen && entry.expected_revision != post.seen_revision {
        return Ok(None);
    }
    if post.seen && entry.at <= post.seen_at {
        return Ok(None);
    }
    if !post.seen {
        state.seen_count = increment(state.seen_count)?;
    }
    post.seen = true;
    post.seen_at = post.seen_at.max(entry.at);
    post.seen_revision = increment(post.seen_revision)?;
    post.seen_order = order(post.seen_at, &post.hash);
    Ok(Some(post))
}

#[spacetimedb::reducer]
pub fn mark_seen_batch(ctx: &ReducerContext, generation: u64, entries: Vec<SeenInput>) -> Result<(), String> {
    let fid = reader(ctx)?;
    let mut state = ctx.db.library_account().fid().find(fid).unwrap_or_else(|| account(fid));
    if state.generation != generation {
        return Err("history generation changed".into());
    }
    let entries = inputs(entries, ctx.timestamp.to_micros_since_unix_epoch() / 1_000_000)?;
    let mut changed = false;
    for entry in entries {
        let old = ctx.db.library_post().key().find(key(fid, &entry.hash));
        if let Some(post) = apply_seen(&mut state, old, entry)? {
            save_post(ctx, post);
            changed = true;
        }
    }
    if changed {
        state.revision = increment(state.revision)?;
        save_account(ctx, state);
    }
    Ok(())
}

fn clear(state: &mut LibraryAccount, expected: u64, operation: &str, now: i64) -> Result<bool, String> {
    if operation.is_empty()
        || operation.len() > 64
        || !operation.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("invalid history clear operation id".into());
    }
    if state.last_clear_id == operation && expected.checked_add(1) == Some(state.generation) {
        return Ok(false);
    }
    if state.generation != expected {
        return Err("history generation changed".into());
    }
    state.generation = increment(state.generation)?;
    state.revision = increment(state.revision)?;
    state.seen_count = 0;
    state.cleared_at = state.cleared_at.max(now.max(0));
    state.last_clear_id = operation.into();
    Ok(true)
}

#[spacetimedb::reducer]
pub fn clear_seen(ctx: &ReducerContext, expected_generation: u64, operation_id: String) -> Result<(), String> {
    let fid = reader(ctx)?;
    let mut state = ctx.db.library_account().fid().find(fid).unwrap_or_else(|| account(fid));
    if clear(&mut state, expected_generation, &operation_id, ctx.timestamp.to_micros_since_unix_epoch() / 1_000_000)? {
        save_account(ctx, state);
    }
    Ok(())
}

fn bookmark(
    state: &mut LibraryAccount,
    old: Option<LibraryPost>,
    hash: String,
    desired: bool,
    expected: u64,
    now: i64,
) -> Result<Option<LibraryPost>, String> {
    let mut post = normalized(state, hash, old);
    if post.bookmarked == desired && (desired || post.bookmark_revision > 0) {
        return Ok(None);
    }
    if expected != post.bookmark_revision {
        return Err("bookmark revision changed".into());
    }
    if desired {
        state.bookmark_count = increment(state.bookmark_count)?;
    } else if post.bookmarked {
        state.bookmark_count = decrement(state.bookmark_count)?;
    }
    post.bookmarked = desired;
    post.bookmark_revision = increment(post.bookmark_revision)?;
    post.bookmark_at = if desired { now.max(0) } else { 0 };
    post.bookmark_order = if desired { order(post.bookmark_at, &post.hash) } else { String::new() };
    Ok(Some(post))
}

#[spacetimedb::reducer]
pub fn set_bookmark(ctx: &ReducerContext, hash: String, desired: bool, expected_revision: u64) -> Result<(), String> {
    let fid = reader(ctx)?;
    let hash = self::hash(&hash)?;
    let mut state = ctx.db.library_account().fid().find(fid).unwrap_or_else(|| account(fid));
    let old = ctx.db.library_post().key().find(key(fid, &hash));
    if let Some(post) = bookmark(
        &mut state,
        old,
        hash,
        desired,
        expected_revision,
        ctx.timestamp.to_micros_since_unix_epoch() / 1_000_000,
    )? {
        state.revision = increment(state.revision)?;
        save_post(ctx, post);
        save_account(ctx, state);
    }
    Ok(())
}

fn forget(
    state: &mut LibraryAccount,
    old: Option<LibraryPost>,
    hash: String,
    expected: u64,
) -> Result<Option<LibraryPost>, String> {
    let mut post = normalized(state, hash, old);
    if !post.seen && post.seen_revision > 0 {
        return Ok(None);
    }
    if post.seen_revision != expected {
        return Err("seen revision changed".into());
    }
    if post.seen {
        state.seen_count = decrement(state.seen_count)?;
    }
    post.seen = false;
    post.seen_at = 0;
    post.seen_order.clear();
    post.seen_revision = increment(post.seen_revision)?;
    Ok(Some(post))
}

#[spacetimedb::reducer]
pub fn forget_seen(ctx: &ReducerContext, generation: u64, hash: String, expected_revision: u64) -> Result<(), String> {
    let fid = reader(ctx)?;
    let hash = self::hash(&hash)?;
    let mut state = ctx.db.library_account().fid().find(fid).unwrap_or_else(|| account(fid));
    if state.generation != generation {
        return Err("history generation changed".into());
    }
    let old = ctx.db.library_post().key().find(key(fid, &hash));
    if let Some(post) = forget(&mut state, old, hash, expected_revision)? {
        state.revision = increment(state.revision)?;
        save_post(ctx, post);
        save_account(ctx, state);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(n: u64) -> String {
        format!("0x{n:040x}")
    }
    fn entry(n: u64, at: i64, expected_revision: u64) -> SeenInput {
        SeenInput { hash: h(n), at, expected_revision }
    }

    #[test]
    fn hashes_batches_cursors_and_sort_keys_are_bounded() {
        assert_eq!(hash(&h(10).to_uppercase().replacen("0X", "0x", 1)), Ok(h(10)));
        for bad in ["", "0x", "garbage", &format!("0x{}", "g".repeat(40))] {
            assert!(hash(bad).is_err());
        }
        assert!(membership_hashes("").is_err());
        assert_eq!(membership_hashes(&format!("{},{}", h(1), h(1))).unwrap(), [h(1)]);
        assert!(membership_hashes(&(0..101).map(h).collect::<Vec<_>>().join(",")).is_err());
        assert!(inputs(vec![entry(1, 100, 0), entry(1, 100, 0)], 100).is_err());
        assert!(inputs(vec![entry(1, -1, 0)], 100).is_err());
        assert!(inputs(vec![entry(1, 161, 0)], 100).is_err());
        assert_eq!(inputs(vec![entry(1, 160, 0)], 100).unwrap()[0].at, 100);
        assert!(after_key("-1:0x0000000000000000000000000000000000000001").is_err());
        assert!(after_key("100:garbage").is_err());
        assert_eq!(after_key(&format!("100:{}", h(1))), Ok(order(100, &h(1))));
        assert!(order(101, &h(1)) < order(100, &h(2)));
        assert!(order(100, &h(2)) < order(100, &h(1)));
    }

    #[test]
    fn seen_membership_never_expires_and_stale_times_only_merge_up() {
        let mut state = account(1234);
        let post = apply_seen(&mut state, None, entry(1, 1, 0)).unwrap().unwrap();
        assert_eq!(state.seen_count, 1);
        let later = apply_seen(&mut state, Some(post.clone()), entry(1, 10_000_000, 0)).unwrap().unwrap();
        assert_eq!(later.seen_at, 10_000_000);
        assert_eq!(state.seen_count, 1);
        assert!(apply_seen(&mut state, Some(later.clone()), entry(1, 2, 0)).unwrap().is_none());
        assert!(item(&state, later, None).seen);
    }

    #[test]
    fn clear_is_generation_scoped_idempotent_and_preserves_bookmarks() {
        let mut state = account(1234);
        let seen = apply_seen(&mut state, None, entry(1, 100, 0)).unwrap().unwrap();
        let post = bookmark(&mut state, Some(seen), h(1), true, 0, 100).unwrap().unwrap();
        assert!(clear(&mut state, 0, "operation-1", 200).unwrap());
        assert_eq!((state.generation, state.seen_count, state.bookmark_count, state.cleared_at), (1, 0, 1, 200));
        let projection = item(&state, post.clone(), None);
        assert!(!projection.seen && projection.bookmarked);
        assert!(apply_seen(&mut state, Some(post.clone()), entry(1, 199, 0)).unwrap().is_none());
        let fresh = apply_seen(&mut state, Some(post), entry(1, 201, 0)).unwrap().unwrap();
        assert!(!clear(&mut state, 0, "operation-1", 300).unwrap());
        assert_eq!(state.seen_count, 1);
        assert_eq!(fresh.seen_at, 201);
        assert!(clear(&mut state, 0, "another-old-operation", 300).is_err());
        assert!(clear(&mut state, 1, "", 300).is_err());
        state.generation = u64::MAX;
        assert!(clear(&mut state, u64::MAX, "overflow", 300).is_err());
    }

    #[test]
    fn bookmark_tombstones_prevent_offline_resurrection() {
        let mut state = account(1234);
        let on = bookmark(&mut state, None, h(1), true, 0, 100).unwrap().unwrap();
        assert_eq!(state.bookmark_count, 1);
        assert!(bookmark(&mut state, Some(on.clone()), h(1), true, 0, 200).unwrap().is_none());
        let off = bookmark(&mut state, Some(on), h(1), false, 1, 200).unwrap().unwrap();
        assert_eq!(state.bookmark_count, 0);
        assert!(bookmark(&mut state, Some(off.clone()), h(1), true, 0, 300).is_err());
        assert!(bookmark(&mut state, Some(off.clone()), h(1), false, 0, 300).unwrap().is_none());
        let on = bookmark(&mut state, Some(off), h(1), true, 2, 300).unwrap().unwrap();
        assert_eq!(on.bookmark_revision, 3);
        assert_eq!(state.bookmark_count, 1);
        let tombstone = bookmark(&mut state, None, h(2), false, 0, 100).unwrap().unwrap();
        assert_eq!(tombstone.bookmark_revision, 1);
        assert!(bookmark(&mut state, Some(tombstone), h(2), true, 0, 300).is_err());
    }

    #[test]
    fn individual_forget_fences_queued_observations() {
        let mut state = account(1234);
        let post = apply_seen(&mut state, None, entry(1, 100, 0)).unwrap().unwrap();
        let post = forget(&mut state, Some(post), h(1), 1).unwrap().unwrap();
        assert_eq!(state.seen_count, 0);
        assert!(apply_seen(&mut state, Some(post.clone()), entry(1, 200, 0)).unwrap().is_none());
        let fresh = apply_seen(&mut state, Some(post), entry(1, 200, 2)).unwrap().unwrap();
        assert_eq!(state.seen_count, 1);
        assert_eq!(fresh.seen_revision, 3);
        assert!(forget(&mut state, Some(fresh), h(1), 1).is_err());
    }
}
