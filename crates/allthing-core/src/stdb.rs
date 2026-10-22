// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing's realtime database on SpacetimeDB (the module in
//! `spacetime/spacetimedb`): where it is, and the attestation that lets a
//! reader's SpacetimeDB identity prove which Farcaster account signed in.
//!
//! After Sign In With Farcaster, the browser asks the Worker to attest its
//! SpacetimeDB identity (`POST /api/v1/auth/attest`). The Worker signs
//! `attestation_message(identity, fid, expiry)` with its own Ed25519 key,
//! the `ATTEST_KEY` secret, used for nothing else; the browser hands the
//! signature to the module's `verify_fid` reducer, which checks it against
//! the key's public half pinned in the module and marks the identity's FID
//! verified. The attestation names the identity, so it is useless to anyone
//! else, and lapses after `ATTEST_TTL`.

use serde::{Deserialize, Serialize};

/// The SpacetimeDB host Allthing's database is on.
pub const STDB_URI: &str = "wss://maincloud.spacetimedb.com";
/// Allthing's database on it.
pub const STDB_DATABASE: &str = "allthing";
/// What an attestation signs, before its fields, so its signature can't
/// stand for anything else. The module uses the same string.
pub const ATTEST_CONTEXT: &str = "allthing-spacetimedb-fid-v1";
/// How long an attestation is valid for, in seconds (the module refuses
/// any valid for more than an hour).
pub const ATTEST_TTL: u64 = 600;

/// What the Worker signs for `identity` signed in as `fid` until `expiry`
/// (Unix seconds).
pub fn attestation_message(identity: &str, fid: u64, expiry: u64) -> String {
    format!("{ATTEST_CONTEXT}|{identity}|{fid}|{expiry}")
}

/// Whether `s` is a SpacetimeDB identity as the module spells it: 64
/// lower-case hex digits, no `0x`.
pub fn is_identity(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `POST /api/v1/auth/attest`'s body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestRequest {
    /// The browser's SpacetimeDB identity.
    pub identity: String,
}

/// The Worker's attestation: what `verify_fid(fid, expiry, signature)`
/// takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attestation {
    pub identity: String,
    pub fid: u64,
    pub expiry: u64,
    /// The Ed25519 signature of `attestation_message`, 128 hex digits.
    pub signature: String,
}

/// The PKCS #8 wrapping of a raw 32-byte Ed25519 private key (RFC 8410),
/// as WebCrypto imports it.
pub fn ed25519_pkcs8(seed: &[u8; 32]) -> [u8; 48] {
    const PREFIX: [u8; 16] =
        [0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20];
    let mut out = [0; 48];
    out[..16].copy_from_slice(&PREFIX);
    out[16..].copy_from_slice(seed);
    out
}

/// The `ATTEST_KEY` secret: 64 hex digits (32 bytes), with or without `0x`.
pub fn parse_attest_key(secret: &str) -> Option<[u8; 32]> {
    let hex = secret.trim();
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_matches_the_modules_test_vector() {
        // spacetime/spacetimedb/src/lib.rs `attestations` signs this very
        // string with RFC 8032's first test key.
        assert_eq!(
            attestation_message("c200431f50cf410304546489e2600c8446408f1aa727c91d86bc00aef6724d15", 3, 1_800_000_000),
            "allthing-spacetimedb-fid-v1|c200431f50cf410304546489e2600c8446408f1aa727c91d86bc00aef6724d15|3|1800000000"
        );
    }

    #[test]
    fn identities() {
        assert!(is_identity("c200431f50cf410304546489e2600c8446408f1aa727c91d86bc00aef6724d15"));
        assert!(!is_identity("0xc200431f50cf410304546489e2600c8446408f1aa727c91d86bc00aef6724d15"));
        assert!(!is_identity("C200431F50CF410304546489E2600C8446408F1AA727C91D86BC00AEF6724D15"));
        assert!(!is_identity("c200"));
        assert!(!is_identity(&"g".repeat(64)));
    }

    #[test]
    fn keys() {
        let hex = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
        let key = parse_attest_key(hex).unwrap();
        assert_eq!(key[0], 0x9d);
        assert_eq!(parse_attest_key(&format!(" 0x{hex}\n")), Some(key));
        assert_eq!(parse_attest_key(&hex[2..]), None);
        assert_eq!(parse_attest_key(&hex.replace('9', "z")), None);
        assert_eq!(parse_attest_key("é".repeat(32).as_str()), None);
        let der = ed25519_pkcs8(&key);
        assert_eq!(der[..2], [0x30, 0x2e]);
        assert_eq!(der[16..], key);
    }
}

// What the app mirrors of the database: the rows the bridge
// (web/stdb/bridge.ts) reports, as JSON, with identities and bytes as hex
// and times in microseconds since the Unix epoch.

use std::collections::BTreeMap;

/// The general room, which the module seeds as its first.
pub const GENERAL_ROOM: u64 = 1;
/// The longest room message, in bytes (the module's limit).
pub const MAX_CHAT_BYTES: usize = 4000;

/// `user`: an identity, the FID it claims, and whether `verify_fid`
/// verified it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct UserRow {
    pub identity: String,
    pub fid: u64,
    pub verified: bool,
    pub online: bool,
    pub last_seen: i64,
}

/// `user_key`: an identity's X25519 public key and its version.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct KeyRow {
    pub identity: String,
    pub fid: u64,
    /// 64 hex digits.
    pub x25519_pub: String,
    pub key_version: u32,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RoomRow {
    pub id: u64,
    pub name: String,
}

/// `chat_message`: a message in a room, in the clear.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ChatRow {
    pub id: u64,
    pub room_id: u64,
    pub sender: String,
    pub sender_fid: u64,
    pub text: String,
    pub sent_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TypingRow {
    pub identity: String,
    pub room_id: u64,
    pub expires_at: i64,
}

/// A row of `my_dm_inbox` or `my_dm_outbox`: an encrypted message, of which
/// the database knows only who, when, the key versions, the nonce and the
/// ciphertext (hex).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DmRow {
    pub id: u64,
    pub sender: String,
    pub recipient: String,
    pub sender_key_version: u32,
    pub recipient_key_version: u32,
    pub nonce: String,
    pub ciphertext: String,
    pub sent_at: i64,
}

/// An event from the bridge: a row that came, went or changed, or the
/// connection lost.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Event {
    Disconnected { error: Option<String> },
    Insert { table: String, row: serde_json::Value },
    Delete { table: String, row: serde_json::Value },
    Update { table: String, row: serde_json::Value },
}

/// What a reducer call came to, as the bridge reports it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Called {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
}

impl Called {
    pub fn result(self) -> Result<(), String> {
        if self.ok { Ok(()) } else { Err(self.error.unwrap_or_else(|| "the database refused".into())) }
    }
}

/// The app's copy of the rows it follows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mirror {
    pub users: BTreeMap<String, UserRow>,
    pub keys: BTreeMap<String, KeyRow>,
    pub rooms: BTreeMap<u64, RoomRow>,
    pub chat: BTreeMap<u64, ChatRow>,
    pub typing: BTreeMap<String, TypingRow>,
    /// The reader's inbox and outbox together, by id.
    pub dms: BTreeMap<u64, DmRow>,
}

/// One of the reader's encrypted conversations, by the other identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub peer: String,
    pub last_at: i64,
    pub count: usize,
}

impl Mirror {
    /// Apply a row event; whether it was for a table the app follows.
    pub fn apply(&mut self, event: &Event) -> bool {
        let (table, row, insert) = match event {
            Event::Insert { table, row } | Event::Update { table, row } => (table, row, true),
            Event::Delete { table, row } => (table, row, false),
            _ => return false,
        };
        fn put<K: Ord, V: serde::de::DeserializeOwned>(
            map: &mut BTreeMap<K, V>,
            row: &serde_json::Value,
            insert: bool,
            key: impl Fn(&V) -> K,
        ) -> bool {
            let Ok(value) = serde_json::from_value::<V>(row.clone()) else { return false };
            let k = key(&value);
            if insert {
                map.insert(k, value);
            } else {
                map.remove(&k);
            }
            true
        }
        match table.as_str() {
            "user" => put(&mut self.users, row, insert, |r: &UserRow| r.identity.clone()),
            "user_key" => put(&mut self.keys, row, insert, |r: &KeyRow| r.identity.clone()),
            "room" => put(&mut self.rooms, row, insert, |r: &RoomRow| r.id),
            "chat_message" => put(&mut self.chat, row, insert, |r: &ChatRow| r.id),
            "typing" => put(&mut self.typing, row, insert, |r: &TypingRow| r.identity.clone()),
            "my_dm_inbox" | "my_dm_outbox" => put(&mut self.dms, row, insert, |r: &DmRow| r.id),
            _ => false,
        }
    }

    /// Whether `identity` verified the FID it claims.
    pub fn verified(&self, identity: &str) -> bool {
        self.users.get(identity).is_some_and(|u| u.verified)
    }

    /// The FID `identity` claims, if it registered.
    pub fn fid_of(&self, identity: &str) -> Option<u64> {
        self.users.get(identity).map(|u| u.fid)
    }

    /// The key to write to `fid` with: of its identities that have a key,
    /// verified ones first, then the most recently updated.
    pub fn key_for_fid(&self, fid: u64) -> Option<&KeyRow> {
        self.keys
            .values()
            .filter(|k| self.fid_of(&k.identity) == Some(fid))
            .max_by_key(|k| (self.verified(&k.identity), k.updated_at, k.identity.clone()))
    }

    /// The messages in `room`, oldest first.
    pub fn room_messages(&self, room: u64) -> Vec<&ChatRow> {
        let mut rows: Vec<&ChatRow> = self.chat.values().filter(|m| m.room_id == room).collect();
        rows.sort_by_key(|m| (m.sent_at, m.id));
        rows
    }

    /// Who else is typing in `room` at `now_us`.
    pub fn typing_in(&self, room: u64, me: &str, now_us: i64) -> Vec<u64> {
        let mut fids: Vec<u64> = self
            .typing
            .values()
            .filter(|t| t.room_id == room && t.identity != me && t.expires_at > now_us)
            .filter_map(|t| self.fid_of(&t.identity))
            .collect();
        fids.sort_unstable();
        fids.dedup();
        fids
    }

    /// How many accounts are online: distinct FIDs.
    pub fn online(&self) -> usize {
        let mut fids: Vec<u64> = self.users.values().filter(|u| u.online).map(|u| u.fid).collect();
        fids.sort_unstable();
        fids.dedup();
        fids.len()
    }

    /// The reader's (`me`) encrypted conversations, newest first.
    pub fn threads(&self, me: &str) -> Vec<Thread> {
        let mut by_peer: BTreeMap<&str, Thread> = BTreeMap::new();
        for dm in self.dms.values() {
            let peer = if dm.sender == me { dm.recipient.as_str() } else { dm.sender.as_str() };
            let thread = by_peer.entry(peer).or_insert(Thread { peer: peer.to_owned(), last_at: 0, count: 0 });
            thread.last_at = thread.last_at.max(dm.sent_at);
            thread.count += 1;
        }
        let mut threads: Vec<Thread> = by_peer.into_values().collect();
        threads.sort_by(|a, b| b.last_at.cmp(&a.last_at).then_with(|| a.peer.cmp(&b.peer)));
        threads
    }

    /// The messages between the reader (`me`) and `peer`, oldest first.
    pub fn thread(&self, me: &str, peer: &str) -> Vec<&DmRow> {
        let mut rows: Vec<&DmRow> = self
            .dms
            .values()
            .filter(|d| (d.sender == me && d.recipient == peer) || (d.sender == peer && d.recipient == me))
            .collect();
        rows.sort_by_key(|d| (d.sent_at, d.id));
        rows
    }
}

/// The SQL the app subscribes to: everything it shows, and no other room's
/// messages.
pub fn subscriptions() -> Vec<String> {
    vec![
        "SELECT * FROM user".into(),
        "SELECT * FROM user_key".into(),
        "SELECT * FROM room".into(),
        format!("SELECT * FROM chat_message WHERE room_id = {GENERAL_ROOM}"),
        "SELECT * FROM typing".into(),
        "SELECT * FROM my_dm_inbox".into(),
        "SELECT * FROM my_dm_outbox".into(),
    ]
}

/// Bytes from hex, if it is hex.
pub fn unhex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect()
}

/// Lower-case hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod mirror_tests {
    use super::*;
    use serde_json::json;

    const ME: &str = "c200000000000000000000000000000000000000000000000000000000000001";
    const ALICE: &str = "c200000000000000000000000000000000000000000000000000000000000002";
    const ALICE2: &str = "c200000000000000000000000000000000000000000000000000000000000003";

    fn event(text: &str) -> Event {
        serde_json::from_str(text).unwrap()
    }

    fn dm(id: u64, sender: &str, recipient: &str, at: i64) -> serde_json::Value {
        json!({"id": id, "sender": sender, "recipient": recipient, "sender_key_version": 1,
            "recipient_key_version": 1, "nonce": "00", "ciphertext": "11", "sent_at": at})
    }

    #[test]
    fn reads_the_bridges_events() {
        assert_eq!(event(r#"{"t":"disconnected","error":null}"#), Event::Disconnected { error: None });
        assert_eq!(
            event(r#"{"t":"insert","table":"room","row":{"id":1,"name":"general"}}"#),
            Event::Insert { table: "room".into(), row: json!({"id": 1, "name": "general"}) }
        );
        let called: Called = serde_json::from_str(r#"{"ok":false,"error":"rate limited"}"#).unwrap();
        assert_eq!(called.result(), Err("rate limited".into()));
        let called: Called = serde_json::from_str(r#"{"ok":true}"#).unwrap();
        assert_eq!(called.result(), Ok(()));
    }

    #[test]
    fn mirrors_rows_and_answers_questions() {
        let mut m = Mirror::default();
        let user = |identity: &str, fid: u64, verified: bool, online: bool| Event::Insert {
            table: "user".into(),
            row: json!({"identity": identity, "fid": fid, "verified": verified, "online": online, "last_seen": 0}),
        };
        assert!(m.apply(&user(ME, 1, true, true)));
        assert!(m.apply(&user(ALICE, 2, true, true)));
        assert!(m.apply(&user(ALICE2, 2, false, false)));
        assert!(!m.apply(&Event::Insert { table: "elsewhere".into(), row: json!({}) }));
        assert!(!m.apply(&Event::Insert { table: "user".into(), row: json!({"identity": 3}) }));
        assert_eq!(m.online(), 2);
        assert!(m.verified(ALICE) && !m.verified(ALICE2) && !m.verified("nobody"));

        let key = |identity: &str, fid: u64, at: i64| Event::Insert {
            table: "user_key".into(),
            row: json!({"identity": identity, "fid": fid, "x25519_pub": "aa", "key_version": 1, "updated_at": at}),
        };
        m.apply(&key(ALICE, 2, 10));
        m.apply(&key(ALICE2, 2, 20));
        // The verified identity's key, though the other is newer.
        assert_eq!(m.key_for_fid(2).unwrap().identity, ALICE);
        m.apply(&user(ALICE2, 2, true, true));
        assert_eq!(m.key_for_fid(2).unwrap().identity, ALICE2);
        assert_eq!(m.key_for_fid(9), None);

        for (id, sender, recipient, at) in [(1, ME, ALICE, 5), (2, ALICE, ME, 7), (3, ALICE2, ME, 6)] {
            m.apply(&Event::Insert { table: "my_dm_inbox".into(), row: dm(id, sender, recipient, at) });
        }
        let threads = m.threads(ME);
        assert_eq!(threads.iter().map(|t| (t.peer.as_str(), t.count)).collect::<Vec<_>>(), [(ALICE, 2), (ALICE2, 1)]);
        assert_eq!(m.thread(ME, ALICE).iter().map(|d| d.id).collect::<Vec<_>>(), [1, 2]);
        m.apply(&Event::Delete { table: "my_dm_outbox".into(), row: dm(1, ME, ALICE, 5) });
        assert_eq!(m.thread(ME, ALICE).len(), 1);

        let chat = |id: u64, room: u64, at: i64| Event::Insert {
            table: "chat_message".into(),
            row: json!({"id": id, "room_id": room, "sender": ALICE, "sender_fid": 2, "text": "hi", "sent_at": at}),
        };
        m.apply(&chat(2, 1, 20));
        m.apply(&chat(1, 1, 10));
        m.apply(&chat(3, 2, 5));
        assert_eq!(m.room_messages(1).iter().map(|c| c.id).collect::<Vec<_>>(), [1, 2]);

        let typing = |identity: &str, expires: i64| Event::Insert {
            table: "typing".into(),
            row: json!({"identity": identity, "room_id": 1, "expires_at": expires}),
        };
        m.apply(&typing(ALICE, 100));
        m.apply(&typing(ME, 100));
        m.apply(&typing(ALICE2, 50));
        assert_eq!(m.typing_in(1, ME, 60), [2]);
        assert_eq!(m.typing_in(1, ME, 100), Vec::<u64>::new());
    }

    #[test]
    fn hex_round_trips_and_subscriptions_stay_in_the_general_room() {
        assert_eq!(unhex("00ff10"), Some(vec![0, 255, 16]));
        assert_eq!(unhex("0"), None);
        assert_eq!(unhex("zz"), None);
        assert_eq!(hex(&[0, 255, 16]), "00ff10");
        assert!(subscriptions().contains(&"SELECT * FROM chat_message WHERE room_id = 1".to_owned()));
        assert!(subscriptions().iter().all(|q| !q.contains("dm_message ")));
    }
}
