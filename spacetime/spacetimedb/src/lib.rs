// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing realtime: chat rooms + E2E-encrypted DMs (server stores ciphertext only).
use spacetimedb::{Identity, ReducerContext, Table, TimeDuration, Timestamp, ViewContext, view};

mod attest_key;
pub use attest_key::ATTEST_PUBLIC_KEY;

const MAX_MSG: usize = 4000;
const MAX_CT: usize = 16 * 1024;
const MAX_ROOM_NAME: usize = 64;
const MIN_INTERVAL_US: i64 = 250_000; // max ~4 sends/sec per identity
const TYPING_TTL_US: i64 = 6_000_000;
/// The longest an attestation may be valid for, from now. The Worker signs
/// them for 10 minutes; a longer one is refused, so a leaked attestation is
/// worth little.
pub const ATTEST_MAX_TTL_SECS: u64 = 3600;
/// What an attestation signs, so its signature can't stand for anything else
/// the key might ever sign. Allthing's Worker builds the same string
/// (`allthing_core::stdb::attestation_message`).
pub const ATTEST_CONTEXT: &str = "allthing-spacetimedb-fid-v1";
/// Small upstream clock differences may put an event just ahead of this
/// server. Accepted values are still capped at server time before storing.
const NOTIFICATION_FUTURE_ALLOWANCE_SECS: i64 = 60;

/// SpacetimeDB identity -> Farcaster FID. `verified` once `verify_fid` checked
/// Allthing's attestation that this identity signed in as `fid`.
#[spacetimedb::table(accessor = user, public)]
pub struct User {
    #[primary_key]
    pub identity: Identity,
    #[index(btree)]
    pub fid: u64,
    pub verified: bool,
    pub online: bool,
    pub last_seen: Timestamp,
}

/// Per-user public keys for E2E. Only PUBLIC keys; private keys never leave the client.
#[spacetimedb::table(accessor = user_key, public)]
pub struct UserKey {
    #[primary_key]
    pub identity: Identity,
    #[index(btree)]
    pub fid: u64,
    pub x25519_pub: Vec<u8>, // 32 bytes
    pub key_version: u32,
    pub updated_at: Timestamp,
}

#[spacetimedb::table(accessor = room, public)]
pub struct Room {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[unique]
    pub name: String,
    pub created_by: Identity,
    pub created_at: Timestamp,
}

#[spacetimedb::table(accessor = chat_message, public)]
pub struct ChatMessage {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[index(btree)]
    pub room_id: u64,
    pub sender: Identity,
    pub sender_fid: u64,
    pub text: String,
    pub sent_at: Timestamp,
}

#[spacetimedb::table(accessor = typing, public)]
pub struct Typing {
    #[primary_key]
    pub identity: Identity,
    pub room_id: u64,
    pub expires_at: Timestamp,
}

/// PRIVATE: exposed only via my_dm_inbox / my_dm_outbox views.
#[spacetimedb::table(accessor = dm_message)]
pub struct DmMessage {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[index(btree)]
    pub sender: Identity,
    #[index(btree)]
    pub recipient: Identity,
    pub sender_key_version: u32,
    pub recipient_key_version: u32,
    pub nonce: Vec<u8>,      // 24 bytes (XChaCha20-Poly1305)
    pub ciphertext: Vec<u8>, // AEAD output incl. tag
    pub sent_at: Timestamp,
}

/// Private per-identity rate limiter.
#[spacetimedb::table(accessor = rate_limit)]
pub struct RateLimit {
    #[primary_key]
    pub identity: Identity,
    pub last_at: Timestamp,
}

/// PRIVATE: an identity's signed account binding for notification state.
/// It is separate from public chat users: reading notifications must not
/// announce presence, publish a message key or create a chat account.
#[spacetimedb::table(accessor = notification_reader)]
pub struct NotificationReader {
    #[primary_key]
    pub identity: Identity,
    pub fid: u64,
}

/// PRIVATE: the account-wide notification watermark, shared by all of its
/// independently verified identities. No notification content is stored.
#[spacetimedb::table(accessor = notification_state)]
pub struct NotificationState {
    #[primary_key]
    pub fid: u64,
    /// Unix seconds, matching Hypersnap's notification event timestamps.
    pub seen_at: i64,
}

/// Only this caller's signed account binding can select a watermark. A
/// client-supplied FID or a public, unverified chat claim grants no access.
#[view(accessor = my_notification_state, public)]
fn my_notification_state(ctx: &ViewContext) -> Vec<NotificationState> {
    let Some(reader) = ctx.db.notification_reader().identity().find(ctx.sender()) else { return Vec::new() };
    ctx.db.notification_state().fid().find(reader.fid).into_iter().collect()
}

#[view(accessor = my_dm_inbox, public)]
fn my_dm_inbox(ctx: &ViewContext) -> Vec<DmMessage> {
    ctx.db.dm_message().recipient().filter(ctx.sender()).collect()
}
#[view(accessor = my_dm_outbox, public)]
fn my_dm_outbox(ctx: &ViewContext) -> Vec<DmMessage> {
    ctx.db.dm_message().sender().filter(ctx.sender()).collect()
}

fn rate(ctx: &ReducerContext) -> Result<(), String> {
    let t = ctx.db.rate_limit();
    if let Some(r) = t.identity().find(ctx.sender()) {
        if ctx.timestamp.to_micros_since_unix_epoch() - r.last_at.to_micros_since_unix_epoch() < MIN_INTERVAL_US {
            return Err("rate limited".into());
        }
        t.identity().update(RateLimit { identity: ctx.sender(), last_at: ctx.timestamp });
    } else {
        t.insert(RateLimit { identity: ctx.sender(), last_at: ctx.timestamp });
    }
    Ok(())
}

fn me(ctx: &ReducerContext) -> Result<User, String> {
    ctx.db.user().identity().find(ctx.sender()).ok_or_else(|| "register first".into())
}

/// The caller, allowed to speak: once the attestation key is pinned, only a
/// verified FID may publish a key, chat, create rooms, type or send DMs, so
/// no one can speak as someone else's FID.
fn speaker(ctx: &ReducerContext) -> Result<User, String> {
    let u = me(ctx)?;
    if attestations_on(&ATTEST_PUBLIC_KEY) && !u.verified {
        return Err("verify your Farcaster account first".into());
    }
    Ok(u)
}

#[spacetimedb::reducer(init)]
pub fn init(ctx: &ReducerContext) {
    ctx.db.room().insert(Room { id: 0, name: "general".into(), created_by: ctx.sender(), created_at: ctx.timestamp });
}

#[spacetimedb::reducer(client_connected)]
pub fn on_connect(ctx: &ReducerContext) {
    if let Some(u) = ctx.db.user().identity().find(ctx.sender()) {
        ctx.db.user().identity().update(User { online: true, last_seen: ctx.timestamp, ..u });
    }
}

#[spacetimedb::reducer(client_disconnected)]
pub fn on_disconnect(ctx: &ReducerContext) {
    if let Some(u) = ctx.db.user().identity().find(ctx.sender()) {
        ctx.db.user().identity().update(User { online: false, last_seen: ctx.timestamp, ..u });
    }
    ctx.db.typing().identity().delete(ctx.sender());
}

/// Claim a FID, unverified (`verify_fid` verifies it). Claiming the FID this
/// identity already verified keeps it verified; claiming another is refused.
#[spacetimedb::reducer]
pub fn register(ctx: &ReducerContext, fid: u64) -> Result<(), String> {
    if fid == 0 {
        return Err("bad fid".into());
    }
    let u = User { identity: ctx.sender(), fid, verified: false, online: true, last_seen: ctx.timestamp };
    if let Some(old) = ctx.db.user().identity().find(ctx.sender()) {
        if old.verified && old.fid != fid {
            return Err("fid locked (verified)".into());
        }
        ctx.db.user().identity().update(User { verified: old.verified, ..u });
    } else {
        ctx.db.user().insert(u);
    }
    Ok(())
}

/// Verify the caller's FID with Allthing's attestation: the Ed25519
/// signature, by the key pinned in `attest_key.rs`, of
/// `allthing-spacetimedb-fid-v1|<identity hex>|<fid>|<expiry>`, which
/// Allthing's Worker signs for a reader signed in with Farcaster as `fid`
/// (`POST /api/v1/auth/attest`). The identity is the caller's own, so an
/// attestation can't be used by anyone else. Sets `verified`, and moves the
/// identity (and its key row) to `fid` if it had claimed another.
#[spacetimedb::reducer]
pub fn verify_fid(ctx: &ReducerContext, fid: u64, expiry: u64, signature: Vec<u8>) -> Result<(), String> {
    let now = (ctx.timestamp.to_micros_since_unix_epoch() / 1_000_000).max(0) as u64;
    let identity = ctx.sender().to_hex().to_string();
    check_attestation(&ATTEST_PUBLIC_KEY, &identity, fid, expiry, now, &signature)?;
    let u = User { identity: ctx.sender(), fid, verified: true, online: true, last_seen: ctx.timestamp };
    if ctx.db.user().identity().find(ctx.sender()).is_some() {
        ctx.db.user().identity().update(u);
    } else {
        ctx.db.user().insert(u);
    }
    if let Some(k) = ctx.db.user_key().identity().find(ctx.sender())
        && k.fid != fid
    {
        ctx.db.user_key().identity().update(UserKey { fid, ..k });
    }
    Ok(())
}

/// Verify notification access without touching public chat users, keys or
/// presence. The same identity-bound Worker attestation is required even
/// when chat's development-only unverified mode is available.
#[spacetimedb::reducer]
pub fn verify_notification_fid(ctx: &ReducerContext, fid: u64, expiry: u64, signature: Vec<u8>) -> Result<(), String> {
    let now = (ctx.timestamp.to_micros_since_unix_epoch() / 1_000_000).max(0) as u64;
    let identity = ctx.sender().to_hex().to_string();
    check_attestation(&ATTEST_PUBLIC_KEY, &identity, fid, expiry, now, &signature)?;
    let readers = ctx.db.notification_reader();
    let previous = readers.identity().find(ctx.sender());
    notification_fid(previous.as_ref().map(|reader| reader.fid), fid)?;
    if previous.is_none() {
        readers.insert(NotificationReader { identity: ctx.sender(), fid });
    }
    Ok(())
}

/// Advance the signed account's watermark. Stale or offline sessions may
/// replay older values safely: a transaction can only increase the value.
/// This deliberately does not share chat's per-identity send rate limiter.
#[spacetimedb::reducer]
pub fn mark_notifications_seen(ctx: &ReducerContext, seen_at: i64) -> Result<(), String> {
    let reader =
        ctx.db.notification_reader().identity().find(ctx.sender()).ok_or("verify your Farcaster account first")?;
    let now = ctx.timestamp.to_micros_since_unix_epoch() / 1_000_000;
    let states = ctx.db.notification_state();
    let old = states.fid().find(reader.fid);
    let merged = notification_seen_at(old.as_ref().map(|state| state.seen_at), seen_at, now)?;
    match old {
        Some(state) if merged > state.seen_at => {
            states.fid().update(NotificationState { fid: reader.fid, seen_at: merged });
        }
        None => {
            states.insert(NotificationState { fid: reader.fid, seen_at: merged });
        }
        _ => {}
    }
    Ok(())
}

#[spacetimedb::reducer]
pub fn set_public_key(ctx: &ReducerContext, x25519_pub: Vec<u8>) -> Result<(), String> {
    let u = speaker(ctx)?;
    public_key_shape(&x25519_pub)?;
    let t = ctx.db.user_key();
    if let Some(k) = t.identity().find(ctx.sender()) {
        t.identity().update(UserKey {
            x25519_pub,
            key_version: k.key_version + 1,
            updated_at: ctx.timestamp,
            fid: u.fid,
            ..k
        });
    } else {
        t.insert(UserKey { identity: ctx.sender(), fid: u.fid, x25519_pub, key_version: 1, updated_at: ctx.timestamp });
    }
    Ok(())
}

#[spacetimedb::reducer]
pub fn create_room(ctx: &ReducerContext, name: String) -> Result<(), String> {
    speaker(ctx)?;
    rate(ctx)?;
    let name = room_name(&name)?;
    ctx.db
        .room()
        .try_insert(Room { id: 0, name, created_by: ctx.sender(), created_at: ctx.timestamp })
        .map(|_| ())
        .map_err(|_| "room exists".into())
}

#[spacetimedb::reducer]
pub fn send_message(ctx: &ReducerContext, room_id: u64, text: String) -> Result<(), String> {
    let u = speaker(ctx)?;
    let text = chat_text(&text)?;
    if ctx.db.room().id().find(room_id).is_none() {
        return Err("no such room".into());
    }
    rate(ctx)?;
    ctx.db.typing().identity().delete(ctx.sender());
    ctx.db.chat_message().insert(ChatMessage {
        id: 0,
        room_id,
        sender: ctx.sender(),
        sender_fid: u.fid,
        text,
        sent_at: ctx.timestamp,
    });
    Ok(())
}

#[spacetimedb::reducer]
pub fn set_typing(ctx: &ReducerContext, room_id: u64, typing: bool) -> Result<(), String> {
    speaker(ctx)?;
    ctx.db.typing().identity().delete(ctx.sender());
    if typing {
        if ctx.db.room().id().find(room_id).is_none() {
            return Err("no such room".into());
        }
        ctx.db.typing().insert(Typing {
            identity: ctx.sender(),
            room_id,
            expires_at: ctx.timestamp + TimeDuration::from_micros(TYPING_TTL_US),
        });
    }
    Ok(())
}

/// Store an E2E-encrypted DM. Server validates shape only; never sees plaintext.
#[spacetimedb::reducer]
pub fn send_dm(
    ctx: &ReducerContext,
    recipient: Identity,
    sender_key_version: u32,
    recipient_key_version: u32,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
) -> Result<(), String> {
    speaker(ctx)?;
    if recipient == ctx.sender() {
        return Err("cannot DM self".into());
    }
    if ctx.db.user_key().identity().find(recipient).is_none() {
        return Err("recipient has no key".into());
    }
    dm_shape(&nonce, &ciphertext)?;
    rate(ctx)?;
    ctx.db.dm_message().insert(DmMessage {
        id: 0,
        sender: ctx.sender(),
        recipient,
        sender_key_version,
        recipient_key_version,
        nonce,
        ciphertext,
        sent_at: ctx.timestamp,
    });
    Ok(())
}

/// Either party may delete a DM.
#[spacetimedb::reducer]
pub fn delete_dm(ctx: &ReducerContext, id: u64) -> Result<(), String> {
    let m = ctx.db.dm_message().id().find(id).ok_or("not found")?;
    if m.sender != ctx.sender() && m.recipient != ctx.sender() {
        return Err("not found".into());
    }
    ctx.db.dm_message().id().delete(id);
    Ok(())
}

// Checks on what clients send, kept apart from the database so they can be
// unit-tested natively (`cargo test` in this directory).

/// Notification bindings are immutable and never accept a zero FID, even
/// if called apart from attestation validation in a future reducer.
fn notification_fid(previous: Option<u64>, fid: u64) -> Result<(), String> {
    if fid == 0 {
        return Err("bad fid".into());
    }
    if previous.is_some_and(|old| old != fid) {
        return Err("notification fid locked (verified)".into());
    }
    Ok(())
}

/// Validate a proposed watermark before taking the monotonic maximum.
/// A client's bad clock cannot permanently hide future notifications.
fn notification_seen_at(previous: Option<i64>, seen_at: i64, now: i64) -> Result<i64, String> {
    let now = now.max(0);
    if seen_at < 0 || seen_at > now.saturating_add(NOTIFICATION_FUTURE_ALLOWANCE_SECS) {
        return Err("notification time is outside the allowed range".into());
    }
    Ok(previous.unwrap_or(0).max(seen_at.min(now)))
}

/// A room name: trimmed, lower-cased, 1–64 of `[a-z0-9-_]`.
pub fn room_name(name: &str) -> Result<String, String> {
    let name = name.trim().to_lowercase();
    if name.is_empty()
        || name.len() > MAX_ROOM_NAME
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("room name: 1-64 chars [a-z0-9-_]".into());
    }
    Ok(name)
}

/// A room message: trimmed, 1–4000 bytes.
pub fn chat_text(text: &str) -> Result<String, String> {
    let text = text.trim().to_string();
    if text.is_empty() || text.len() > MAX_MSG {
        return Err("message 1-4000 bytes".into());
    }
    Ok(text)
}

/// An X25519 public key: 32 bytes.
pub fn public_key_shape(key: &[u8]) -> Result<(), String> {
    if key.len() != 32 {
        return Err("x25519 key must be 32 bytes".into());
    }
    Ok(())
}

/// A DM's shape: a 24-byte XChaCha20 nonce and 16 B–16 KiB of ciphertext
/// (the 16-byte Poly1305 tag at least).
pub fn dm_shape(nonce: &[u8], ciphertext: &[u8]) -> Result<(), String> {
    if nonce.len() != 24 {
        return Err("nonce must be 24 bytes".into());
    }
    if ciphertext.len() < 16 || ciphertext.len() > MAX_CT {
        return Err("ciphertext 16..16384 bytes".into());
    }
    Ok(())
}

/// Whether attestations are checked: the pinned key isn't the all-zero
/// placeholder.
pub fn attestations_on(key: &[u8; 32]) -> bool {
    key.iter().any(|b| *b != 0)
}

/// What Allthing's attestation key signs for `identity` (lower-case hex,
/// big-endian, no `0x`: `Identity::to_hex`) signed in as `fid` until
/// `expiry` (Unix seconds).
pub fn attestation_message(identity: &str, fid: u64, expiry: u64) -> String {
    format!("{ATTEST_CONTEXT}|{identity}|{fid}|{expiry}")
}

/// Check an attestation at `now` (Unix seconds): the key is pinned, the FID
/// is real, it hasn't expired and isn't valid for too long, and the
/// signature is the key's over `attestation_message`.
pub fn check_attestation(
    key: &[u8; 32],
    identity: &str,
    fid: u64,
    expiry: u64,
    now: u64,
    signature: &[u8],
) -> Result<(), String> {
    use ed25519_dalek::{Signature, VerifyingKey};
    if !attestations_on(key) {
        return Err("FID verification isn't set up yet".into());
    }
    if fid == 0 {
        return Err("bad fid".into());
    }
    if expiry <= now {
        return Err("attestation expired".into());
    }
    if expiry > now + ATTEST_MAX_TTL_SECS {
        return Err("attestation valid for too long".into());
    }
    let signature = Signature::from_slice(signature).map_err(|_| "attestation signature must be 64 bytes")?;
    let key = VerifyingKey::from_bytes(key).map_err(|_| "bad attestation key")?;
    key.verify_strict(attestation_message(identity, fid, expiry).as_bytes(), &signature)
        .map_err(|_| "attestation signature doesn't verify".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// The test vector Allthing's Worker side checks too
    /// (`allthing_core::stdb` tests): RFC 8032's first test key.
    const SEED: [u8; 32] = [
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c, 0xc4, 0x44, 0x49,
        0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae, 0x7f, 0x60,
    ];
    const IDENTITY: &str = "c200431f50cf410304546489e2600c8446408f1aa727c91d86bc00aef6724d15";
    const VECTOR_MESSAGE: &str =
        "allthing-spacetimedb-fid-v1|c200431f50cf410304546489e2600c8446408f1aa727c91d86bc00aef6724d15|3|1800000000";
    const VECTOR_SIGNATURE: &str = include_str!("../tests/attestation-vector.sig");

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.trim().len()).step_by(2).map(|i| u8::from_str_radix(&s.trim()[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn attestations() {
        let signer = SigningKey::from_bytes(&SEED);
        let key = signer.verifying_key().to_bytes();
        let now = 1_799_999_500;
        assert_eq!(attestation_message(IDENTITY, 3, 1_800_000_000), VECTOR_MESSAGE);
        let signature = signer.sign(VECTOR_MESSAGE.as_bytes()).to_bytes();
        assert_eq!(signature.to_vec(), unhex(VECTOR_SIGNATURE));
        assert_eq!(check_attestation(&key, IDENTITY, 3, 1_800_000_000, now, &signature), Ok(()));
        // Anyone else's identity, another FID or expiry, a flipped bit.
        let other = IDENTITY.replace("c200", "c201");
        assert!(check_attestation(&key, &other, 3, 1_800_000_000, now, &signature).is_err());
        assert!(check_attestation(&key, IDENTITY, 4, 1_800_000_000, now, &signature).is_err());
        assert!(check_attestation(&key, IDENTITY, 3, 1_800_000_001, now, &signature).is_err());
        let mut bad = signature;
        bad[0] ^= 1;
        assert!(check_attestation(&key, IDENTITY, 3, 1_800_000_000, now, &bad).is_err());
        assert!(check_attestation(&key, IDENTITY, 3, 1_800_000_000, now, &signature[..63]).is_err());
        // Expired, or valid for too long.
        assert!(check_attestation(&key, IDENTITY, 3, 1_800_000_000, 1_800_000_000, &signature).is_err());
        assert!(check_attestation(&key, IDENTITY, 3, 1_800_000_000, 1_800_000_000 - 3601, &signature).is_err());
        // Another key, or none pinned.
        let stranger = SigningKey::from_bytes(&[7; 32]).verifying_key().to_bytes();
        assert!(check_attestation(&stranger, IDENTITY, 3, 1_800_000_000, now, &signature).is_err());
        assert_eq!(
            check_attestation(&[0; 32], IDENTITY, 3, 1_800_000_000, now, &signature),
            Err("FID verification isn't set up yet".into())
        );
        assert!(!attestations_on(&[0; 32]));
    }

    #[test]
    fn notification_accounts_cannot_be_rebound() {
        assert_eq!(notification_fid(None, 3), Ok(()));
        assert_eq!(notification_fid(Some(3), 3), Ok(()));
        assert!(notification_fid(None, 0).is_err());
        assert!(notification_fid(Some(3), 0).is_err());
        assert!(notification_fid(Some(3), 4).is_err());
    }

    #[test]
    fn notification_watermarks_merge_all_session_orders() {
        let now = 1_800_000_000;
        assert_eq!(notification_seen_at(None, 0, now), Ok(0));
        assert_eq!(notification_seen_at(None, now - 10, now), Ok(now - 10));
        assert_eq!(notification_seen_at(Some(now - 5), now - 10, now), Ok(now - 5));
        assert_eq!(notification_seen_at(Some(now - 10), now - 5, now), Ok(now - 5));
        assert_eq!(notification_seen_at(Some(now - 5), now - 5, now), Ok(now - 5));
        for values in [[now - 10, now - 5, now - 20], [now - 20, now - 5, now - 10]] {
            let mut seen = None;
            for value in values {
                seen = Some(notification_seen_at(seen, value, now).unwrap());
            }
            assert_eq!(seen, Some(now - 5));
        }
    }

    #[test]
    fn notification_watermarks_never_persist_a_future_clock() {
        let now = 1_800_000_000;
        assert_eq!(notification_seen_at(None, now, now), Ok(now));
        assert_eq!(notification_seen_at(None, now + NOTIFICATION_FUTURE_ALLOWANCE_SECS, now), Ok(now));
        assert!(notification_seen_at(None, now + NOTIFICATION_FUTURE_ALLOWANCE_SECS + 1, now).is_err());
        assert!(notification_seen_at(Some(now), -1, now).is_err());
        assert!(notification_seen_at(None, i64::MAX, now).is_err());
        assert_eq!(notification_seen_at(None, 0, -1), Ok(0));
        assert_eq!(notification_seen_at(None, i64::MAX, i64::MAX), Ok(i64::MAX));
    }

    #[test]
    fn room_names() {
        assert_eq!(room_name("  General ").unwrap(), "general");
        assert_eq!(room_name("dev_chat-2").unwrap(), "dev_chat-2");
        assert!(room_name("").is_err());
        assert!(room_name("   ").is_err());
        assert!(room_name("has space").is_err());
        assert!(room_name("émoji").is_err());
        assert!(room_name(&"a".repeat(64)).is_ok());
        assert!(room_name(&"a".repeat(65)).is_err());
    }

    #[test]
    fn chat_texts() {
        assert_eq!(chat_text(" hi \n").unwrap(), "hi");
        assert!(chat_text(" \n\t").is_err());
        assert!(chat_text(&"x".repeat(4000)).is_ok());
        assert!(chat_text(&"x".repeat(4001)).is_err());
        // Bytes, not characters: 1001 four-byte characters are too many.
        assert!(chat_text(&"\u{1F438}".repeat(1001)).is_err());
    }

    #[test]
    fn key_and_dm_shapes() {
        assert!(public_key_shape(&[0; 32]).is_ok());
        assert!(public_key_shape(&[0; 31]).is_err());
        assert!(public_key_shape(&[0; 33]).is_err());
        assert!(dm_shape(&[0; 24], &[0; 16]).is_ok());
        assert!(dm_shape(&[0; 24], &[0; MAX_CT]).is_ok());
        assert!(dm_shape(&[0; 12], &[0; 16]).is_err());
        assert!(dm_shape(&[0; 24], &[0; 15]).is_err());
        assert!(dm_shape(&[0; 24], &vec![0; MAX_CT + 1]).is_err());
    }
}
