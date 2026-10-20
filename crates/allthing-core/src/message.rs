// SPDX-License-Identifier: AGPL-3.0-or-later
//! Farcaster messages a signed-in reader writes through Allthing: a like or
//! a recast, a cast or a reply, a follow, and taking any of them back. Each
//! is encoded as Snapchain's `message.proto` defines it, hashed with BLAKE3
//! (its first 20 bytes), and signed by an app key, an Ed25519 key pair the
//! reader approved in the Farcaster app ([`crate::app_key`]). The browser
//! signs, with a key it can't export (WebCrypto), and hands the envelope to
//! the node's `submitMessage`; Allthing's servers never see the key.
//!
//! Also here: the node's answers about the reader's own messages, which
//! Allthing reads from the browser too (what they liked and recasted, and
//! whether their key is registered).

use serde_json::Value;

use crate::params::percent_encode;
use crate::provenance::{hex_encode, normalize_key};

/// The node Allthing reads from (Hypersnap, a Snapchain mainnet node),
/// which relays the messages it is given to Snapchain's validators.
pub const NODE: &str = "https://haatz.quilibrium.com";
/// Farcaster timestamps count seconds from 2021-01-01T00:00:00Z.
pub const EPOCH: i64 = 1_609_459_200;
/// `FARCASTER_NETWORK_MAINNET`.
const MAINNET: u64 = 1;
/// `HASH_SCHEME_BLAKE3` and `SIGNATURE_SCHEME_ED25519`.
const BLAKE3: u64 = 1;
const ED25519: u64 = 1;

/// A cast, by its author and its hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CastId {
    pub fid: u64,
    pub hash: [u8; 20],
}

impl CastId {
    /// From an author's fid and a cast's `0x…` hash.
    pub fn new(fid: u64, hash: &str) -> Option<CastId> {
        let hex = hash.strip_prefix("0x")?;
        if fid == 0 || hex.len() != 40 {
            return None;
        }
        let mut bytes = [0u8; 20];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
        }
        Some(CastId { fid, hash: bytes })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reaction {
    Like = 1,
    Recast = 2,
}

impl Reaction {
    /// As the node's HTTP API names it.
    fn query(self) -> &'static str {
        match self {
            Reaction::Like => "Like",
            Reaction::Recast => "Recast",
        }
    }
}

/// The longest text a cast may have, in bytes. Over 320 bytes it is a
/// "long cast", which every account may post; over 1,024 only Farcaster
/// Pro's 10,000-byte casts, which Allthing doesn't offer.
pub const MAX_CAST_BYTES: usize = 1024;
const SHORT_CAST_BYTES: usize = 320;
/// The most embeds (links and quoted casts) and mentions a cast may carry.
pub const MAX_EMBEDS: usize = 2;
pub const MAX_MENTIONS: usize = 10;
/// The longest embedded link, in bytes.
pub const MAX_URL_BYTES: usize = 256;

/// What a cast embeds: a link (an image, a GIF, a page), or another cast,
/// which it quotes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Embed {
    Url(String),
    Cast(CastId),
}

/// What a cast belongs to: the cast it replies to, or a channel's URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parent {
    Cast(CastId),
    Url(String),
}

/// A new cast, as Farcaster stores it: its text without the accounts it
/// mentions, which are listed apart with the byte offset each stood at, in
/// order (`crate::text::split_mentions`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewCast {
    pub text: String,
    pub mentions: Vec<(u64, u32)>,
    pub embeds: Vec<Embed>,
    pub parent: Option<Parent>,
}

impl NewCast {
    /// Why the network would refuse it, if it would, in words a reader can
    /// act on.
    pub fn check(&self) -> Result<(), &'static str> {
        if self.text.trim().is_empty() && self.embeds.is_empty() && self.mentions.is_empty() {
            return Err("Write something, or add a GIF.");
        }
        if self.text.len() > MAX_CAST_BYTES {
            return Err("That's too long for a cast: 1,024 bytes at most.");
        }
        if self.embeds.len() > MAX_EMBEDS {
            return Err("A cast can carry two embeds at most.");
        }
        if self.mentions.len() > MAX_MENTIONS {
            return Err("A cast can mention ten accounts at most.");
        }
        let urls = self.embeds.iter().filter_map(|e| match e {
            Embed::Url(url) => Some(url),
            Embed::Cast(_) => None,
        });
        let parent_url = match &self.parent {
            Some(Parent::Url(url)) => Some(url),
            _ => None,
        };
        if urls.chain(parent_url).any(|url| url.is_empty() || url.len() > MAX_URL_BYTES) {
            return Err("A link in it is too long for a cast.");
        }
        let in_order = self.mentions.windows(2).all(|w| w[0].1 <= w[1].1);
        if !in_order || self.mentions.iter().any(|(fid, at)| *fid == 0 || *at as usize > self.text.len()) {
            return Err("Its mentions are out of place.");
        }
        Ok(())
    }

    /// `CastAddBody`, fields in order.
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if !self.mentions.is_empty() {
            // Packed, as proto3 writes repeated numbers.
            let mut fids = Vec::new();
            for (fid, _) in &self.mentions {
                varint(&mut fids, *fid);
            }
            nested(&mut out, 2, &fids);
        }
        if let Some(Parent::Cast(id)) = &self.parent {
            nested(&mut out, 3, &cast_id(id));
        }
        bytes(&mut out, 4, self.text.as_bytes());
        if !self.mentions.is_empty() {
            let mut positions = Vec::new();
            for (_, at) in &self.mentions {
                varint(&mut positions, u64::from(*at));
            }
            nested(&mut out, 5, &positions);
        }
        for embed in &self.embeds {
            let mut one = Vec::new();
            match embed {
                Embed::Url(url) => bytes(&mut one, 1, url.as_bytes()),
                Embed::Cast(id) => nested(&mut one, 2, &cast_id(id)),
            }
            nested(&mut out, 6, &one);
        }
        if let Some(Parent::Url(url)) = &self.parent {
            bytes(&mut out, 7, url.as_bytes());
        }
        // `CastType`: CAST (0, left out) or LONG_CAST (1).
        uint(&mut out, 8, u64::from(self.text.len() > SHORT_CAST_BYTES));
        out
    }
}

/// What a message does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    CastAdd(NewCast),
    ReactionAdd(Reaction, CastId),
    ReactionRemove(Reaction, CastId),
    Follow(u64),
    Unfollow(u64),
    /// Take back one of the reader's own casts, by its hash.
    CastRemove([u8; 20]),
}

impl Body {
    /// Its `MessageType`.
    fn message_type(&self) -> u64 {
        match self {
            Body::CastAdd(_) => 1,
            Body::CastRemove(_) => 2,
            Body::ReactionAdd(..) => 3,
            Body::ReactionRemove(..) => 4,
            Body::Follow(_) => 5,
            Body::Unfollow(_) => 6,
        }
    }

    /// Its field in `MessageData`'s `body`, and its encoding.
    fn encode(&self) -> (u64, Vec<u8>) {
        let mut out = Vec::new();
        match self {
            Body::CastAdd(cast) => (5, cast.encode()),
            Body::ReactionAdd(kind, target) | Body::ReactionRemove(kind, target) => {
                uint(&mut out, 1, *kind as u64);
                nested(&mut out, 2, &cast_id(target));
                (7, out)
            }
            Body::Follow(target) | Body::Unfollow(target) => {
                bytes(&mut out, 1, b"follow");
                uint(&mut out, 3, *target);
                (14, out)
            }
            Body::CastRemove(hash) => {
                bytes(&mut out, 1, hash);
                (6, out)
            }
        }
    }
}

/// What is hashed and signed: who, when, and what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageData {
    pub fid: u64,
    /// Farcaster time: seconds since [`EPOCH`].
    pub timestamp: u32,
    pub body: Body,
}

impl MessageData {
    /// A message from `fid` at `unix` seconds.
    pub fn new(fid: u64, unix: i64, body: Body) -> MessageData {
        MessageData { fid, timestamp: (unix - EPOCH).clamp(0, u32::MAX as i64) as u32, body }
    }

    /// Its protobuf encoding, fields in order, as `data_bytes`.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        uint(&mut out, 1, self.body.message_type());
        uint(&mut out, 2, self.fid);
        uint(&mut out, 3, u64::from(self.timestamp));
        uint(&mut out, 4, MAINNET);
        let (field, body) = self.body.encode();
        nested(&mut out, field, &body);
        out
    }
}

/// What the app key signs: BLAKE3 of the data's bytes, cut to 20 bytes.
pub fn message_hash(data_bytes: &[u8]) -> [u8; 20] {
    let mut hash = [0u8; 20];
    hash.copy_from_slice(&blake3::hash(data_bytes).as_bytes()[..20]);
    hash
}

/// The `Message` a node's `submitMessage` takes: the data's bytes, their
/// hash, the app key's signature of the hash, and the app key itself.
pub fn envelope(data_bytes: &[u8], hash: &[u8; 20], signature: &[u8; 64], signer: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data_bytes.len() + 140);
    bytes(&mut out, 2, hash);
    uint(&mut out, 3, BLAKE3);
    bytes(&mut out, 4, signature);
    uint(&mut out, 5, ED25519);
    bytes(&mut out, 6, signer);
    bytes(&mut out, 7, data_bytes);
    out
}

/// A signed message's parts, read back from the bytes [`envelope`] makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opened<'a> {
    pub hash: &'a [u8],
    pub hash_scheme: u64,
    pub signature: &'a [u8],
    pub signature_scheme: u64,
    pub signer: &'a [u8],
    pub data: &'a [u8],
}

/// A `Message`'s parts: `None` unless the bytes are protobuf with data and
/// a hash.
pub fn open_envelope(bytes: &[u8]) -> Option<Opened<'_>> {
    let mut opened = Opened { hash: &[], hash_scheme: 0, signature: &[], signature_scheme: 0, signer: &[], data: &[] };
    for (field, value) in read_fields(bytes)? {
        match (field, value) {
            (2, Field::Bytes(b)) => opened.hash = b,
            (3, Field::Int(n)) => opened.hash_scheme = n,
            (4, Field::Bytes(b)) => opened.signature = b,
            (5, Field::Int(n)) => opened.signature_scheme = n,
            (6, Field::Bytes(b)) => opened.signer = b,
            (7, Field::Bytes(b)) => opened.data = b,
            _ => {}
        }
    }
    (!opened.data.is_empty() && !opened.hash.is_empty()).then_some(opened)
}

/// What a message's data says: its type, author, time (Farcaster seconds),
/// network, and the field its body is in (5 for a cast).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DataRead {
    pub message_type: u64,
    pub fid: u64,
    pub timestamp: u64,
    pub network: u64,
    pub body_field: u64,
}

/// `MessageData`, read from its bytes: `None` unless they are protobuf.
pub fn read_data(data: &[u8]) -> Option<DataRead> {
    let mut read = DataRead::default();
    for (field, value) in read_fields(data)? {
        match (field, value) {
            (1, Field::Int(n)) => read.message_type = n,
            (2, Field::Int(n)) => read.fid = n,
            (3, Field::Int(n)) => read.timestamp = n,
            (4, Field::Int(n)) => read.network = n,
            (field, Field::Bytes(_)) if field >= 5 => read.body_field = field,
            _ => {}
        }
    }
    Some(read)
}

/// Where the node takes a message (the bytes of an [`envelope`], as
/// `application/octet-stream`).
pub fn submit_url() -> String {
    format!("{NODE}/v1/submitMessage")
}

/// What became of a submitted message, from the node's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Submitted {
    Merged,
    /// The node has it already: as good as merged.
    Duplicate,
    /// The key isn't one of the account's: not registered yet, or removed.
    UnknownKey,
    /// The node is busy or down just now.
    Unavailable,
    /// Refused for another reason, in the node's words.
    Refused(String),
}

/// The node's answer to `submitMessage`.
pub fn submitted(status: u16, body: &str) -> Submitted {
    if (200..300).contains(&status) {
        return Submitted::Merged;
    }
    if status == 429 || status >= 500 {
        return Submitted::Unavailable;
    }
    let failure = serde_json::from_str::<Value>(body).ok();
    let field = |name: &str| failure.as_ref()?.get(name)?.as_str().map(str::to_owned);
    let detail = field("error_detail").or_else(|| field("error")).unwrap_or_default();
    // Snapchain's codes: "bad_request.validation_failure/invalid signer",
    // "bad_request.duplicate".
    if detail.contains("invalid signer") || detail.contains("unknown signer") {
        Submitted::UnknownKey
    } else if detail.contains("duplicate") {
        Submitted::Duplicate
    } else {
        // Its message, without the gRPC framing around it or Snapchain's
        // code before it ("bad_request.validation_failure/").
        let message = detail.split("message: \"").nth(1).and_then(|m| m.split('"').next()).unwrap_or(&detail);
        let message = match message.split_once('/') {
            Some((code, words)) if code.starts_with("bad_request.") => words,
            _ => message,
        };
        Submitted::Refused(message.trim().to_owned())
    }
}

/// The account's recent reactions of one kind, newest first.
pub fn reactions_url(fid: u64, kind: Reaction, page_size: u32) -> String {
    format!("{NODE}/v1/reactionsByFid?fid={fid}&reaction_type={}&pageSize={page_size}&reverse=true", kind.query())
}

/// The account's reaction of one kind to one cast, if there is one.
pub fn reaction_url(fid: u64, kind: Reaction, target: &CastId) -> String {
    format!(
        "{NODE}/v1/reactionById?fid={fid}&reaction_type={}&target_fid={}&target_hash={}",
        kind.query(),
        target.fid,
        percent_encode(&hex_encode(&target.hash))
    )
}

/// The reader's follow of `target`, if there is one.
pub fn link_url(fid: u64, target: u64) -> String {
    format!("{NODE}/v1/linkById?fid={fid}&target_fid={target}&link_type=follow")
}

/// Whether the answer to [`link_url`] says the reader follows the account:
/// `None` when the node couldn't say.
pub fn follows(status: u16, body: &str) -> Option<bool> {
    if status == 200 {
        let message = serde_json::from_str::<Value>(body).ok()?;
        return Some(message.get("data")?.get("type")?.as_str() == Some("MESSAGE_TYPE_LINK_ADD"));
    }
    (status == 400 && body.contains("NotFound")).then_some(false)
}

/// Who a username (an fname, or an ENS name such as `dwr.eth`) belongs to,
/// for a mention. Names are lowercase.
pub fn username_url(name: &str) -> String {
    format!("{NODE}/v1/userNameProofByName?name={}", percent_encode(name))
}

/// The account in an answer to [`username_url`]: `Some(None)` when the name
/// is no one's, `None` when the node couldn't say.
pub fn username_fid(status: u16, body: &str) -> Option<Option<u64>> {
    if status == 200 {
        let proof = serde_json::from_str::<Value>(body).ok()?;
        return Some(proof.get("fid").and_then(Value::as_u64).filter(|fid| *fid > 0));
    }
    (status == 400 && body.contains("NotFound")).then_some(None)
}

/// The account's registered keys.
pub fn signers_url(fid: u64) -> String {
    format!("{NODE}/v1/signersByFid?fid={fid}")
}

/// The casts in an answer to [`reactions_url`], by hash, lowercase.
pub fn reacted(body: &str) -> Option<Vec<String>> {
    let page = serde_json::from_str::<Value>(body).ok()?;
    Some(page.get("messages")?.as_array()?.iter().filter_map(target_hash).map(str::to_ascii_lowercase).collect())
}

fn target_hash(message: &Value) -> Option<&str> {
    message.get("data")?.get("reactionBody")?.get("targetCastId")?.get("hash")?.as_str()
}

/// Whether the answer to [`reaction_url`] says the reaction is there:
/// `None` when the node couldn't say.
pub fn reaction_found(status: u16, body: &str) -> Option<bool> {
    if status == 200 {
        let message = serde_json::from_str::<Value>(body).ok()?;
        return Some(message.get("data")?.get("type")?.as_str() == Some("MESSAGE_TYPE_REACTION_ADD"));
    }
    // The node answers a missing reaction with a 400 that says so.
    (status == 400 && body.contains("NotFound")).then_some(false)
}

/// Whether the answer to [`signers_url`] lists `key` (`0x…`): `None` when
/// it isn't an answer.
pub fn lists_key(body: &str, key: &str) -> Option<bool> {
    let answer = serde_json::from_str::<Value>(body).ok()?;
    let key = key.to_ascii_lowercase();
    let listed = |s: &Value| s.get("key").and_then(Value::as_str).and_then(normalize_key);
    Some(answer.get("signers")?.as_array()?.iter().filter_map(listed).any(|k| k == key))
}

/// A protobuf field's value: a varint, or length-delimited bytes, the only
/// kinds Farcaster's messages use.
enum Field<'a> {
    Int(u64),
    Bytes(&'a [u8]),
}

/// Protobuf bytes' top-level fields, by number: `None` if they hold another
/// kind, or end in the middle of one.
fn read_fields(mut bytes: &[u8]) -> Option<Vec<(u64, Field<'_>)>> {
    let mut fields = Vec::new();
    while !bytes.is_empty() {
        let tag = read_varint(&mut bytes)?;
        let value = match tag & 7 {
            0 => Field::Int(read_varint(&mut bytes)?),
            2 => {
                let length = usize::try_from(read_varint(&mut bytes)?).ok()?;
                let (value, rest) = bytes.split_at_checked(length)?;
                bytes = rest;
                Field::Bytes(value)
            }
            _ => return None,
        };
        fields.push((tag >> 3, value));
    }
    Some(fields)
}

fn read_varint(bytes: &mut &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = bytes.split_first()?;
        *bytes = rest;
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Some(value);
        }
    }
    None
}

fn cast_id(id: &CastId) -> Vec<u8> {
    let mut out = Vec::new();
    uint(&mut out, 1, id.fid);
    bytes(&mut out, 2, &id.hash);
    out
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// A varint field, left out at zero, as proto3 does.
fn uint(out: &mut Vec<u8>, field: u64, value: u64) {
    if value != 0 {
        varint(out, field << 3);
        varint(out, value);
    }
}

/// A length-delimited field, left out when empty.
fn bytes(out: &mut Vec<u8>, field: u64, value: &[u8]) {
    if !value.is_empty() {
        nested(out, field, value);
    }
}

/// A length-delimited field, written even when empty: a message in a
/// `oneof` is there by being there.
fn nested(out: &mut Vec<u8>, field: u64, value: &[u8]) {
    varint(out, (field << 3) | 2);
    varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        let s: String = s.split_whitespace().collect();
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    const CAST: &str = "0x0102030405060708090a0b0c0d0e0f1011121314";

    #[test]
    fn a_cast_id_comes_from_its_author_and_hash() {
        let id = CastId::new(3, CAST).unwrap();
        assert_eq!(id.fid, 3);
        assert_eq!(hex_encode(&id.hash), CAST);
        assert_eq!(CastId::new(3, "0x0102"), None);
        assert_eq!(CastId::new(0, CAST), None);
        assert_eq!(CastId::new(3, &CAST.replace("0x01", "0xzz")), None);
        assert_eq!(CastId::new(3, &CAST[2..]), None, "a hash is written 0x…");
    }

    #[test]
    fn farcaster_time_counts_from_2021() {
        assert_eq!(MessageData::new(1, EPOCH + 90, Body::Follow(2)).timestamp, 90);
        assert_eq!(MessageData::new(1, 0, Body::Follow(2)).timestamp, 0);
    }

    // The encodings below are worked out by hand from message.proto:
    // MessageData { type = 1, fid = 2, timestamp = 3, network = 4, oneof
    // body { cast_remove_body = 6, reaction_body = 7, link_body = 14 } }.

    #[test]
    fn a_like_is_encoded_as_message_proto_says() {
        let like = MessageData::new(
            1234,
            EPOCH + 0x1234_5678,
            Body::ReactionAdd(Reaction::Like, CastId::new(3, CAST).unwrap()),
        );
        let expected = unhex(
            "08 03          10 d2 09     18 f8 ac d1 91 01     20 01
             3a 1c   08 01   12 18   08 03   12 14 0102030405060708090a0b0c0d0e0f1011121314",
        );
        assert_eq!(like.encode(), expected);
        let unrecast =
            MessageData::new(1234, EPOCH + 1, Body::ReactionRemove(Reaction::Recast, CastId::new(3, CAST).unwrap()));
        assert_eq!(
            unrecast.encode(),
            unhex(
                "08 04  10 d2 09  18 01  20 01  3a 1c 08 02 12 18 08 03 12 14 0102030405060708090a0b0c0d0e0f1011121314"
            )
        );
    }

    #[test]
    fn a_follow_and_a_removed_cast_are_encoded_as_message_proto_says() {
        // LinkBody { type = 1 ("follow"), target_fid = 3 }.
        assert_eq!(
            MessageData::new(7, EPOCH + 2, Body::Follow(300)).encode(),
            unhex("08 05  10 07  18 02  20 01  72 0b  0a 06 666f6c6c6f77  18 ac 02")
        );
        assert_eq!(
            MessageData::new(7, EPOCH + 2, Body::Unfollow(300)).encode(),
            unhex("08 06  10 07  18 02  20 01  72 0b  0a 06 666f6c6c6f77  18 ac 02")
        );
        // CastRemoveBody { target_hash = 1 }.
        let remove = MessageData::new(7, EPOCH + 2, Body::CastRemove(CastId::new(7, CAST).unwrap().hash));
        assert_eq!(
            remove.encode(),
            unhex("08 02  10 07  18 02  20 01  32 16  0a 14 0102030405060708090a0b0c0d0e0f1011121314")
        );
    }

    #[test]
    fn the_hash_is_blake3_cut_to_20_bytes() {
        // BLAKE3 of the empty input is af1349b9f5f9a1a6a0404dea36dcc949…
        assert_eq!(hex_encode(&message_hash(b"")), "0xaf1349b9f5f9a1a6a0404dea36dcc9499bcb25c9");
        assert_eq!(message_hash(b"abc").len(), 20);
    }

    #[test]
    fn the_envelope_carries_the_data_its_hash_and_the_signature() {
        let data = MessageData::new(7, EPOCH + 2, Body::Follow(300)).encode();
        let hash = message_hash(&data);
        let signature = [0x5a; 64];
        let signer = [0x11; 32];
        let message = envelope(&data, &hash, &signature, &signer);
        let mut expected = vec![0x12, 0x14];
        expected.extend_from_slice(&hash);
        expected.extend_from_slice(&[0x18, 0x01, 0x22, 0x40]);
        expected.extend_from_slice(&signature);
        expected.extend_from_slice(&[0x28, 0x01, 0x32, 0x20]);
        expected.extend_from_slice(&signer);
        expected.extend_from_slice(&[0x3a, data.len() as u8]);
        expected.extend_from_slice(&data);
        assert_eq!(message, expected);
    }

    #[test]
    fn node_urls() {
        let target = CastId::new(218_957, "0x5c5b64edc095ec945bb85cd7f5eb31e7ca187045").unwrap();
        assert_eq!(submit_url(), "https://haatz.quilibrium.com/v1/submitMessage");
        assert_eq!(
            reactions_url(3, Reaction::Like, 100),
            "https://haatz.quilibrium.com/v1/reactionsByFid?fid=3&reaction_type=Like&pageSize=100&reverse=true"
        );
        assert_eq!(
            reaction_url(3, Reaction::Recast, &target),
            "https://haatz.quilibrium.com/v1/reactionById?fid=3&reaction_type=Recast&target_fid=218957&target_hash=0x5c5b64edc095ec945bb85cd7f5eb31e7ca187045"
        );
        assert_eq!(signers_url(3), "https://haatz.quilibrium.com/v1/signersByFid?fid=3");
    }

    // The node's answers below are as Hypersnap gave them on 19 October 2026.

    #[test]
    fn reads_the_nodes_answers_about_the_readers_reactions() {
        let page = r#"{"messages":[{"data":{"type":"MESSAGE_TYPE_REACTION_ADD","fid":3,"timestamp":182871013,"network":"FARCASTER_NETWORK_MAINNET","reactionBody":{"type":"REACTION_TYPE_LIKE","targetCastId":{"fid":218957,"hash":"0x5C5b64edc095ec945bb85cd7f5eb31e7ca187045"}}},"hash":"0xbede949bb7f37473d430b7cfd1186fe30d52f9df","hashScheme":"HASH_SCHEME_BLAKE3","signature":"R+Euc==","signatureScheme":"SIGNATURE_SCHEME_ED25519","signer":"0xc887"},{"data":{"type":"MESSAGE_TYPE_REACTION_ADD","fid":3,"reactionBody":{"type":"REACTION_TYPE_LIKE","targetUrl":"https://example.com"}}}],"nextPageToken":"AwAA"}"#;
        assert_eq!(reacted(page), Some(vec!["0x5c5b64edc095ec945bb85cd7f5eb31e7ca187045".to_owned()]));
        assert_eq!(reacted(r#"{"messages":[]}"#), Some(vec![]));
        assert_eq!(reacted("Invalid query parameters"), None);

        let one = r#"{"data":{"type":"MESSAGE_TYPE_REACTION_ADD","fid":3,"reactionBody":{"type":"REACTION_TYPE_LIKE","targetCastId":{"fid":218957,"hash":"0x5c5b64edc095ec945bb85cd7f5eb31e7ca187045"}}},"hash":"0xbede"}"#;
        assert_eq!(reaction_found(200, one), Some(true));
        let missing = r#"{"error":"Failed to get reaction","error_detail":"status: NotFound, message: \"cast not found\", details: [], metadata: MetadataMap { headers: {} }"}"#;
        assert_eq!(reaction_found(400, missing), Some(false));
        assert_eq!(reaction_found(400, "Invalid query parameters: unknown variant"), None);
        assert_eq!(reaction_found(503, ""), None);
    }

    #[test]
    fn finds_the_readers_key_among_their_signers() {
        let signers = r#"{"signers":[{"source":"SIGNER_SOURCE_ONCHAIN","key":"0xC887f5bf385a4718eaee166481f1832198938cf33e98a82dc81a0b4b81ffe33d","keyType":1,"fid":3}],"gaslessSignerCount":0}"#;
        let key = "0xc887f5bf385a4718eaee166481f1832198938cf33e98a82dc81a0b4b81ffe33d";
        assert_eq!(lists_key(signers, key), Some(true));
        assert_eq!(lists_key(signers, &key.replace("c887", "d887")), Some(false));
        assert_eq!(
            lists_key(r#"{"signers":[],"gaslessSignerCount":0,"gaslessSignerLimit":1000,"currentUserNonce":0}"#, key),
            Some(false)
        );
        assert_eq!(lists_key("<html>", key), None);
    }

    #[test]
    fn reads_what_the_node_made_of_a_message() {
        assert_eq!(submitted(200, "{}"), Submitted::Merged);
        let unknown = r#"{"error":"Failed to submit message","error_detail":"status: InvalidArgument, message: \"bad_request.validation_failure/invalid signer\", details: [], metadata: MetadataMap { headers: {\"x-err-code\": \"bad_request.validation_failure\"} }"}"#;
        assert_eq!(submitted(400, unknown), Submitted::UnknownKey);
        let bad = unknown.replace("invalid signer", "invalid signature");
        assert_eq!(submitted(400, &bad), Submitted::Refused("invalid signature".into()));
        let link =
            unknown.replace("bad_request.validation_failure/invalid signer", "embed https://a.co/b is not allowed");
        assert_eq!(submitted(400, &link), Submitted::Refused("embed https://a.co/b is not allowed".into()));
        let duplicate = unknown.replace("validation_failure/invalid signer", "duplicate");
        assert_eq!(submitted(400, &duplicate), Submitted::Duplicate);
        assert_eq!(submitted(502, "<html>Bad gateway</html>"), Submitted::Unavailable);
        assert_eq!(submitted(429, r#"{"error":"rate limited"}"#), Submitted::Unavailable);
        assert_eq!(submitted(400, "not json"), Submitted::Refused(String::new()));
    }

    // CastAddBody { mentions = 2 (packed), parent_cast_id = 3, text = 4,
    // mentions_positions = 5 (packed), embeds = 6, parent_url = 7, type = 8 };
    // Embed { url = 1, cast_id = 2 }; MessageData's cast_add_body is 5.

    #[test]
    fn a_cast_is_encoded_as_message_proto_says() {
        let cast = NewCast { text: "gm".into(), ..NewCast::default() };
        assert_eq!(
            MessageData::new(7, EPOCH + 2, Body::CastAdd(cast)).encode(),
            unhex("08 01  10 07  18 02  20 01  2a 04  22 02 676d")
        );
    }

    #[test]
    fn a_reply_with_a_mention_a_gif_and_a_quote() {
        // "hi  and you": @alice (fid 300) at byte 3.
        let cast = NewCast {
            text: "hi  and you".into(),
            mentions: vec![(300, 3)],
            embeds: vec![Embed::Url("https://g.co/a.gif".into()), Embed::Cast(CastId::new(3, CAST).unwrap())],
            parent: Some(Parent::Cast(CastId::new(2, CAST).unwrap())),
        };
        assert_eq!(cast.check(), Ok(()));
        let body = unhex(
            "12 02 ac 02
             1a 18 08 02 12 14 0102030405060708090a0b0c0d0e0f1011121314
             22 0b 6869202061 6e6420796f75
             2a 01 03
             32 14 0a 12 68747470733a2f2f672e636f2f612e676966
             32 1a 12 18 08 03 12 14 0102030405060708090a0b0c0d0e0f1011121314",
        );
        let mut expected = unhex("08 01  10 07  18 02  20 01  2a");
        expected.push(body.len() as u8);
        expected.extend_from_slice(&body);
        assert_eq!(MessageData::new(7, EPOCH + 2, Body::CastAdd(cast)).encode(), expected);
    }

    #[test]
    fn a_cast_in_a_channel_and_a_long_cast() {
        let url = "https://farcaster.xyz/~/channel/dev";
        let cast = NewCast { text: "x".into(), parent: Some(Parent::Url(url.into())), ..NewCast::default() };
        let mut expected = unhex("22 01 78 3a");
        expected.push(url.len() as u8);
        expected.extend_from_slice(url.as_bytes());
        assert_eq!(cast.encode(), expected);
        // Over 320 bytes: a long cast (type 1); at 320, a cast.
        let short = NewCast { text: "a".repeat(320), ..NewCast::default() };
        assert!(!short.encode().ends_with(&[0x40, 0x01]));
        let long = NewCast { text: "a".repeat(321), ..NewCast::default() };
        assert!(long.encode().ends_with(&[0x40, 0x01]));
        assert_eq!(long.check(), Ok(()));
    }

    #[test]
    fn a_cast_the_network_would_refuse_is_caught_first() {
        let empty = NewCast { text: "  ".into(), ..NewCast::default() };
        assert!(empty.check().is_err());
        let gif_only = NewCast { embeds: vec![Embed::Url("https://g.co/a.gif".into())], ..NewCast::default() };
        assert_eq!(gif_only.check(), Ok(()));
        assert!(NewCast { text: "a".repeat(1025), ..NewCast::default() }.check().is_err());
        let three = vec![Embed::Url("https://a.co".into()); 3];
        assert!(NewCast { text: "x".into(), embeds: three, ..NewCast::default() }.check().is_err());
        let long_url = vec![Embed::Url(format!("https://a.co/{}", "x".repeat(250)))];
        assert!(NewCast { text: "x".into(), embeds: long_url, ..NewCast::default() }.check().is_err());
        let unordered = NewCast { text: "a  b".into(), mentions: vec![(1, 3), (2, 1)], ..NewCast::default() };
        assert!(unordered.check().is_err());
        let beyond = NewCast { text: "a".into(), mentions: vec![(1, 2)], ..NewCast::default() };
        assert!(beyond.check().is_err());
        let many = NewCast { text: "x".into(), mentions: vec![(1, 0); 11], ..NewCast::default() };
        assert!(many.check().is_err());
    }

    #[test]
    fn a_username_is_looked_up_for_a_mention() {
        assert_eq!(username_url("dwr.eth"), "https://haatz.quilibrium.com/v1/userNameProofByName?name=dwr.eth");
        // As Hypersnap answered on 19 October 2026.
        let found = r#"{"timestamp":1623910375,"name":"dwr","owner":"0x74232bf61e994655592747e20bdf6fa9b9476f79","signature":"6+jZ","fid":3,"type":"USERNAME_TYPE_FNAME"}"#;
        assert_eq!(username_fid(200, found), Some(Some(3)));
        let missing = r#"{"error":"Failed to get username proof","error_detail":"status: NotFound, message: \"Username proof not found\""}"#;
        assert_eq!(username_fid(400, missing), Some(None));
        assert_eq!(username_fid(503, ""), None);
    }

    #[test]
    fn a_signed_message_reads_back_as_it_was_made() {
        let cast = NewCast { text: "Later".into(), ..NewCast::default() };
        let data = MessageData::new(3, 1_800_000_000, Body::CastAdd(cast)).encode();
        let hash = message_hash(&data);
        let bytes = envelope(&data, &hash, &[7; 64], &[9; 32]);
        let opened = open_envelope(&bytes).unwrap();
        assert_eq!((opened.hash, opened.hash_scheme), (&hash[..], 1));
        assert_eq!((opened.signature, opened.signature_scheme), (&[7u8; 64][..], 1));
        assert_eq!((opened.signer, opened.data), (&[9u8; 32][..], &data[..]));
        let read = read_data(opened.data).unwrap();
        let timestamp = (1_800_000_000 - EPOCH) as u64;
        assert_eq!(read, DataRead { message_type: 1, fid: 3, timestamp, network: 1, body_field: 5 });
        // A like, by its type and body.
        let like = MessageData::new(3, 1_800_000_000, Body::ReactionAdd(Reaction::Like, CastId::new(2, CAST).unwrap()));
        let read = read_data(&like.encode()).unwrap();
        assert_eq!((read.message_type, read.body_field), (3, 7));
        // Not protobuf, cut short, or without data.
        assert_eq!(open_envelope(&[0xff]), None);
        assert_eq!(open_envelope(&bytes[..bytes.len() - 1]), None);
        assert_eq!(open_envelope(&[]), None);
        assert_eq!(read_data(&[0x0b]), None);
        assert_eq!(read_data(&[0x08, 0x80]), None);
    }

    #[test]
    fn whether_the_reader_follows_an_account() {
        assert_eq!(link_url(3, 2), "https://haatz.quilibrium.com/v1/linkById?fid=3&target_fid=2&link_type=follow");
        // As Hypersnap answered on 19 October 2026.
        let found = r#"{"data":{"type":"MESSAGE_TYPE_LINK_ADD","fid":3,"timestamp":159794223,"network":"FARCASTER_NETWORK_MAINNET","linkBody":{"type":"follow","displayTimestamp":null,"targetFid":2}},"hash":"0xe0c3"}"#;
        assert_eq!(follows(200, found), Some(true));
        let missing =
            r#"{"error":"Failed to get link","error_detail":"status: NotFound, message: \"cast not found\""}"#;
        assert_eq!(follows(400, missing), Some(false));
        assert_eq!(follows(500, ""), None);
    }
}
