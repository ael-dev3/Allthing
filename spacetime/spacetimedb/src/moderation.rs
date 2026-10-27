// SPDX-License-Identifier: AGPL-3.0-or-later
//! Private account filters shared by an account's attested browser identities.
//! False rows remain as revision tombstones so an offline tab cannot undo a clear.

use super::*;

/// No public base table or blocked-by view exists. Every row is owned by a
/// Farcaster account, not a replaceable browser identity.
#[derive(Clone, Debug, PartialEq, Eq)]
#[spacetimedb::table(accessor = account_filter)]
pub struct AccountFilter {
    #[primary_key]
    pub key: String,
    #[index(btree)]
    pub fid: u64,
    pub target_fid: u64,
    pub target_username: String,
    pub muted: bool,
    pub blocked: bool,
    pub mute_revision: u64,
    pub block_revision: u64,
    pub updated_at: i64,
}

fn key(fid: u64, target_fid: u64) -> String {
    format!("{fid}:{target_fid}")
}

fn initial(fid: u64, target_fid: u64) -> AccountFilter {
    AccountFilter {
        key: key(fid, target_fid),
        fid,
        target_fid,
        target_username: String::new(),
        muted: false,
        blocked: false,
        mute_revision: 0,
        block_revision: 0,
        updated_at: 0,
    }
}

/// A parameterless caller view is the complete authoritative snapshot,
/// including clears. No caller-supplied owner FID can expand its scope.
#[view(accessor = my_account_filters, public)]
fn my_account_filters(ctx: &ViewContext) -> Vec<AccountFilter> {
    let Some(reader) = ctx.db.notification_reader().identity().find(ctx.sender()) else { return Vec::new() };
    ctx.db.account_filter().fid().filter(reader.fid).collect()
}

fn apply(row: &mut AccountFilter, kind: u8, desired: bool, expected: u64, now: i64) -> Result<bool, String> {
    let (value, revision) = match kind {
        0 => (&mut row.muted, &mut row.mute_revision),
        1 => (&mut row.blocked, &mut row.block_revision),
        _ => return Err("unknown account filter".into()),
    };
    // A lost acknowledgement may be retried safely. An initial false write
    // still creates a tombstone, fencing a concurrent stale true write.
    if *value == desired && *revision > 0 {
        return Ok(false);
    }
    if *revision != expected {
        return Err("account filter revision changed".into());
    }
    *revision = revision.checked_add(1).ok_or("account filter revision exhausted")?;
    *value = desired;
    row.updated_at = row.updated_at.max(now.max(0));
    Ok(true)
}

/// Display/matching metadata only: this never establishes account ownership.
/// Like the app's profile-name parser, accept ENS labels as well as fnames.
fn username(value: &str) -> Result<String, String> {
    let value = value.trim().strip_prefix('@').unwrap_or(value.trim()).to_lowercase();
    if value.is_empty() || value.strip_prefix("fid:").is_some_and(|fid| fid.bytes().all(|b| b.is_ascii_digit())) {
        return Ok(String::new());
    }
    if value.starts_with('.')
        || value.chars().count() > 253
        || value.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '/' | '?' | '#' | '@' | ':'))
    {
        return Err("invalid account username".into());
    }
    Ok(value)
}

/// Kind 0 changes mute, kind 1 changes block. Independent compare-and-set
/// revisions prevent a stale mute action from restoring a cleared block.
#[spacetimedb::reducer]
pub fn set_account_filter(
    ctx: &ReducerContext,
    target_fid: u64,
    target_username: String,
    kind: u8,
    desired: bool,
    expected_revision: u64,
) -> Result<(), String> {
    let fid = ctx
        .db
        .notification_reader()
        .identity()
        .find(ctx.sender())
        .map(|reader| reader.fid)
        .ok_or("verify your Farcaster account first")?;
    if target_fid == 0 || target_fid == fid {
        return Err("choose another valid Farcaster account".into());
    }
    let target_username = username(&target_username)?;
    let table = ctx.db.account_filter();
    let old = table.key().find(key(fid, target_fid));
    let exists = old.is_some();
    let mut row = old.unwrap_or_else(|| initial(fid, target_fid));
    if apply(&mut row, kind, desired, expected_revision, ctx.timestamp.to_micros_since_unix_epoch() / 1_000_000)? {
        if !target_username.is_empty() {
            row.target_username = target_username;
        }
        if exists {
            table.key().update(row);
        } else {
            table.insert(row);
        }
    }
    Ok(())
}

/// Server-side enforcement applies even to custom clients sending opaque
/// encrypted bytes. It intentionally reveals neither direction nor owner.
pub(super) fn dm_allowed(ctx: &ReducerContext, sender_fid: u64, recipient_fid: u64) -> Result<(), String> {
    let table = ctx.db.account_filter();
    let sender_blocks = table.key().find(key(sender_fid, recipient_fid)).is_some_and(|row| row.blocked);
    let recipient_blocks = table.key().find(key(recipient_fid, sender_fid)).is_some_and(|row| row.blocked);
    if sender_blocks || recipient_blocks {
        return Err("conversation unavailable".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mute_and_block_compare_and_set_independently() {
        let mut row = initial(3, 4);
        assert_eq!(apply(&mut row, 0, true, 0, 10), Ok(true));
        assert_eq!(apply(&mut row, 1, true, 0, 11), Ok(true));
        assert!(row.muted && row.blocked);
        assert_eq!((row.mute_revision, row.block_revision), (1, 1));
        assert_eq!(apply(&mut row, 0, false, 1, 12), Ok(true));
        assert!(!row.muted && row.blocked);
        assert_eq!((row.mute_revision, row.block_revision), (2, 1));
        assert_eq!(apply(&mut row, 1, false, 1, 13), Ok(true));
        assert!(!row.muted && !row.blocked);
        assert_eq!((row.mute_revision, row.block_revision), (2, 2));
    }

    #[test]
    fn clears_fence_stale_offline_writes_and_lost_ack_is_idempotent() {
        let mut row = initial(3, 4);
        assert_eq!(apply(&mut row, 1, true, 0, 10), Ok(true));
        assert_eq!(apply(&mut row, 1, true, 0, 11), Ok(false));
        assert_eq!(row.block_revision, 1);
        assert_eq!(apply(&mut row, 1, false, 1, 12), Ok(true));
        assert_eq!(apply(&mut row, 1, true, 0, 13), Err("account filter revision changed".into()));
        assert!(!row.blocked);
        assert_eq!(row.block_revision, 2);
        assert_eq!(apply(&mut row, 1, false, 1, 14), Ok(false));
        assert_eq!(row.updated_at, 12);
        assert_eq!(apply(&mut row, 1, true, 3, 15), Err("account filter revision changed".into()));
    }

    #[test]
    fn initial_clear_is_a_tombstone_and_bad_kind_or_overflow_cannot_mutate() {
        let mut row = initial(3, 4);
        assert_eq!(apply(&mut row, 0, false, 0, -1), Ok(true));
        assert_eq!(row.mute_revision, 1);
        assert_eq!(apply(&mut row, 0, true, 0, 10), Err("account filter revision changed".into()));
        let old = row.clone();
        assert!(apply(&mut row, 2, true, 0, 10).is_err());
        assert_eq!(row, old);
        row.block_revision = u64::MAX;
        let old = row.clone();
        assert!(apply(&mut row, 1, true, u64::MAX, 10).is_err());
        assert_eq!(row, old);
    }

    #[test]
    fn usernames_are_bounded_normalized_metadata() {
        assert_eq!(username(" @Alice.ETH "), Ok("alice.eth".into()));
        assert_eq!(username("🌴.eth"), Ok("🌴.eth".into()));
        assert_eq!(username(""), Ok(String::new()));
        assert_eq!(username("fid:123"), Ok(String::new()));
        assert!(username(&"a".repeat(253)).is_ok());
        for bad in [".eth", "a/b", "a?b", "a#b", "a@b", "a b", "a\0b", "fid:bad", &"a".repeat(254)] {
            assert!(username(bad).is_err(), "{bad:?}");
        }
    }
}
