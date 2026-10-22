// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing realtime: chat rooms + E2E-encrypted DMs (server stores ciphertext only).
use spacetimedb::{Identity, ReducerContext, Table, TimeDuration, Timestamp, ViewContext, view};

const MAX_MSG: usize = 4000;
const MAX_CT: usize = 16 * 1024;
const MAX_ROOM_NAME: usize = 64;
const MIN_INTERVAL_US: i64 = 250_000; // max ~4 sends/sec per identity
const TYPING_TTL_US: i64 = 6_000_000;

/// SpacetimeDB identity -> Farcaster FID. `verified` false until attestation lands (TODO).
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

/// Claim a FID. Unverified until `verify_fid` is implemented.
/// TODO(FID verification): accept `attestation: Vec<u8>` = Allthing-server Ed25519 signature over
/// (identity_hex, fid, expiry) after Sign-In-With-Farcaster; verify against a pinned Allthing pubkey
/// (e.g. ed25519-dalek in-module), then set verified=true. Alternatively verify an EIP-712 SIWF
/// custody signature (needs on-chain custody lookup -> better done by Allthing backend).
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
        ctx.db.user().identity().update(u);
    } else {
        ctx.db.user().insert(u);
    }
    Ok(())
}

#[spacetimedb::reducer]
pub fn set_public_key(ctx: &ReducerContext, x25519_pub: Vec<u8>) -> Result<(), String> {
    let u = me(ctx)?;
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
    me(ctx)?;
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
    let u = me(ctx)?;
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
    me(ctx)?;
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
    me(ctx)?;
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

#[cfg(test)]
mod tests {
    use super::*;

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
