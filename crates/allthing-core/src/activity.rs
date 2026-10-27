// SPDX-License-Identifier: AGPL-3.0-or-later
//! Daily active accounts, counted as SnapMeter counts them
//! (github.com/ael-dev3/SnapMeter, `docs/metrics.md`): an account (fid) is
//! active when one of its messages was merged into Farcaster. Allthing reads
//! the merges from a shard's canonical hub events, as the newest casts are
//! read (`latest`), and counts each account once per window, whatever it did
//! and however often.
//!
//! - **What counts:** a successful `MERGE_MESSAGE` whose message type is on
//!   SnapMeter's closed allowlist for Snapchain (types 1–8 and 11–21: casts,
//!   reactions, follows, verifications, profile changes, username proofs,
//!   frame actions, storage lending, keys, channels). Pruned, revoked and
//!   failed merges, onchain events and blocks don't; nor do message types
//!   the list doesn't name.
//! - **When:** the time of the block that confirmed it (the block's own
//!   `BLOCK_CONFIRMED` event, which comes first), carried across pages.
//!   Coverage advances only after all declared block events were read.
//! - **Who:** the message's fid, from 1 to 2³² − 1, and the key that signed
//!   it, which says which app it was sent from (`provenance`).
//!
//! The cron keeps each UTC hour's and each UTC day's active fids as a
//! compact set (`Fids`), so every count is exact: an account is counted once
//! in a window however many hours or shards it was seen in.
//!
//! The same read picks out emoji reactions, replies that are one Pepe and
//! nothing else (`emoji::reaction`), and the casts removed, and reactions
//! revoked or pruned, so the cron can keep each cast's reactions for the
//! cards of listings (`api::Annotations::reactions`). No other cast text is
//! kept, or even built.

use std::collections::HashMap;

use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

use crate::emoji::{Emoji, ReactionReply, reaction_text};
use crate::latest::{Cursor, FARCASTER_EPOCH, SHARDS, base64_to_hex};
use crate::model::UserSummary;
#[cfg(test)]
use crate::provenance::{base64_decode, request_fid_from_metadata};
use crate::provenance::{normalize_key, verified_request_fid_from_metadata};

/// What kind of action a counted message is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    Cast,
    Reaction,
    Link,
    Verification,
    UserData,
    UsernameProof,
    Other,
    StorageLending,
    Key,
    Channel,
}

impl Family {
    /// A message type's family, if it counts: SnapMeter's closed allowlist,
    /// Snapchain's message types 1–8 and 11–21 by name.
    pub fn of(kind: &str) -> Option<Family> {
        Some(match kind.strip_prefix("MESSAGE_TYPE_")? {
            "CAST_ADD" | "CAST_REMOVE" => Family::Cast,
            "REACTION_ADD" | "REACTION_REMOVE" => Family::Reaction,
            "LINK_ADD" | "LINK_REMOVE" | "LINK_COMPACT_STATE" => Family::Link,
            "VERIFICATION_ADD_ETH_ADDRESS" | "VERIFICATION_REMOVE" => Family::Verification,
            "USER_DATA_ADD" => Family::UserData,
            "USERNAME_PROOF" => Family::UsernameProof,
            "FRAME_ACTION" => Family::Other,
            "LEND_STORAGE" => Family::StorageLending,
            "KEY_ADD" | "KEY_REMOVE" => Family::Key,
            "CHANNEL_UPDATE" | "CHANNEL_MEMBER" | "CHANNEL_PIN" | "CHANNEL_MODERATE" => Family::Channel,
            _ => return None,
        })
    }
}

/// One counted message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub fid: u64,
    /// The signing key, as `provenance::normalize_key` gives it: lowercase
    /// `0x` hex.
    pub signer: String,
    /// Unix seconds.
    pub at: i64,
    pub family: Family,
}

/// What one page of a shard's events says about activity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActivityPage {
    pub actions: Vec<Action>,
    /// The newest event on the page, to read on from.
    pub last_id: Option<u64>,
    /// The newest block time on the page, Unix seconds: how far the shard has
    /// been read.
    pub reached: Option<i64>,
    /// The oldest block time on the page, Unix seconds.
    pub started: Option<i64>,
    /// The block of the page's first event: one past the block read before
    /// it, unless the node no longer had the blocks between.
    pub first_block: Option<u64>,
    /// How many events the page held, of every kind.
    pub count: usize,
    /// Signing keys added onchain on the page (`key_pair`), each with the
    /// app that requested it, 0 for none.
    #[cfg(test)]
    pub keys: Vec<(u64, u64)>,
    /// Canonically observed registrations, matched by every signer byte.
    pub exact_keys: Vec<(String, u64)>,
    /// Emoji reactions merged on the page: replies that are one emoji and
    /// nothing else (`emoji::reaction`), each with the hash of the cast it
    /// replies to.
    pub reactions: Vec<(String, ReactionReply)>,
    /// Casts the page removes, or whose emoji reactions it revokes or
    /// prunes: their hashes, lowercase `0x` hex.
    pub removed: Vec<String>,
}

#[derive(Deserialize)]
struct Page {
    events: Vec<Event>,
}

#[derive(Deserialize)]
struct Event {
    id: u64,
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "blockNumber")]
    block: Option<u64>,
    #[serde(rename = "shardIndex")]
    shard: Option<u32>,
    #[serde(rename = "blockConfirmedBody")]
    confirmed: Option<Confirmed>,
    #[serde(rename = "mergeMessageBody")]
    merge: Option<Merge>,
    /// A message gone with its key (revoked) or over its account's storage
    /// (pruned).
    #[serde(rename = "revokeMessageBody")]
    revoke: Option<Merge>,
    #[serde(rename = "pruneMessageBody")]
    prune: Option<Merge>,
    #[serde(rename = "mergeOnChainEventBody")]
    chain: Option<OnChain>,
}

#[derive(Deserialize)]
struct OnChain {
    #[serde(rename = "onChainEvent")]
    event: Option<ChainEvent>,
}

#[derive(Deserialize)]
struct ChainEvent {
    #[serde(rename = "type")]
    kind: Option<String>,
    fid: Option<u64>,
    #[serde(rename = "signerEventBody")]
    signer: Option<SignerEvent>,
}

#[derive(Deserialize)]
struct SignerEvent {
    #[serde(rename = "keyType")]
    key_type: Option<u32>,
    key: Option<String>,
    #[serde(rename = "eventType")]
    kind: Option<String>,
    metadata: Option<String>,
    #[serde(rename = "metadataType")]
    metadata_type: Option<u32>,
}

#[derive(Deserialize)]
struct Confirmed {
    #[serde(rename = "blockNumber")]
    block: Option<u64>,
    #[serde(rename = "shardIndex")]
    shard: Option<u32>,
    timestamp: Option<i64>,
    #[serde(rename = "totalEvents")]
    total: Option<u32>,
}

#[derive(Deserialize)]
struct Merge {
    message: Option<Message>,
}

/// Only what is counted is read, and what an emoji reaction needs; serde
/// skips the rest of each message (its signature, a cast's text that isn't
/// one emoji) without building it.
#[derive(Deserialize)]
struct Message {
    hash: Option<String>,
    data: Option<Data>,
    signer: Option<String>,
}

#[derive(Deserialize)]
struct Data {
    #[serde(rename = "type")]
    kind: Option<String>,
    fid: Option<u64>,
    #[cfg(test)]
    timestamp: Option<i64>,
    #[serde(rename = "castAddBody")]
    add: Option<CastAdd>,
    #[serde(rename = "castRemoveBody")]
    remove: Option<CastRemove>,
}

#[derive(Deserialize)]
struct CastAdd {
    text: Option<ReactionText>,
    /// Set on replies.
    #[serde(rename = "parentCastId")]
    parent: Option<CastId>,
    /// Zero-sized items: counted, not built.
    embeds: Option<Vec<IgnoredAny>>,
}

#[derive(Deserialize)]
struct CastId {
    hash: Option<String>,
}

#[derive(Deserialize)]
struct CastRemove {
    #[serde(rename = "targetHash")]
    target: Option<String>,
}

/// A cast's text, read only for the emoji it is when it is one emoji and
/// nothing else (`emoji::reaction_text`); any other text isn't kept.
struct ReactionText(Option<&'static Emoji>);

impl<'de> Deserialize<'de> for ReactionText {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Text;
        impl serde::de::Visitor<'_> for Text {
            type Value = ReactionText;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a cast's text")
            }

            fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<ReactionText, E> {
                Ok(ReactionText(reaction_text(text)))
            }
        }
        deserializer.deserialize_str(Text)
    }
}

/// A hash as lowercase `0x` hex: events give a message's in hex and a
/// removal's target in base64.
fn hex_hash(value: &str) -> Option<String> {
    let hex = value.strip_prefix("0x").filter(|h| h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit()));
    match hex {
        Some(_) => Some(value.to_ascii_lowercase()),
        None => base64_to_hex(value),
    }
}

/// The emoji reaction a message is, at `at`, with the hash of the cast it
/// replies to: a cast that replies to one, with no embeds and one emoji for
/// its text.
fn reaction(hash: Option<&str>, data: &Data, at: Option<i64>) -> Option<(String, ReactionReply)> {
    if data.kind.as_deref() != Some("MESSAGE_TYPE_CAST_ADD") {
        return None;
    }
    let add = data.add.as_ref()?;
    let emoji = add.text.as_ref()?.0?;
    if add.embeds.as_ref().is_some_and(|embeds| !embeds.is_empty()) {
        return None;
    }
    let parent = add.parent.as_ref()?.hash.as_deref().and_then(hex_hash)?;
    let fid = data.fid.filter(|fid| *fid > 0)?;
    Some((parent, ReactionReply { hash: hex_hash(hash?)?, emoji: emoji.code.to_owned(), fid, ts: at? }))
}

/// Farcaster time (seconds since 2021-01-01), as Unix seconds, if it is one.
fn unix(farcaster: i64) -> Option<i64> {
    u32::try_from(farcaster).ok().filter(|t| *t > 0).map(|t| i64::from(t) + FARCASTER_EPOCH)
}

/// Read a `/v1/events` page: its counted actions, where it ends, and how far
/// in time it reaches. `None` if it isn't an events page.
#[cfg(test)]
fn activity(json: &[u8]) -> Option<ActivityPage> {
    let page: Page = serde_json::from_slice(json).ok()?;
    let mut out = ActivityPage { count: page.events.len(), ..Default::default() };
    // Each block's confirmation comes before its messages.
    let mut blocks: HashMap<u64, i64> = HashMap::new();
    for event in page.events {
        out.last_id = Some(out.last_id.map_or(event.id, |id| id.max(event.id)));
        out.first_block = out.first_block.or(event.block.or(Some(event.id >> 14)));
        match event.kind.as_deref() {
            Some("HUB_EVENT_TYPE_BLOCK_CONFIRMED") => {
                let Some(confirmed) = event.confirmed else { continue };
                let (Some(block), Some(at)) = (confirmed.block.or(event.block), confirmed.timestamp.and_then(unix))
                else {
                    continue;
                };
                blocks.insert(block, at);
                out.reached = Some(out.reached.map_or(at, |r| r.max(at)));
                out.started = Some(out.started.map_or(at, |s| s.min(at)));
            }
            Some("HUB_EVENT_TYPE_MERGE_MESSAGE") => {
                let Some(Message { hash, data: Some(data), signer }) = event.merge.and_then(|m| m.message) else {
                    continue;
                };
                let block_time = event.block.and_then(|b| blocks.get(&b).copied());
                let at = block_time.or_else(|| data.timestamp.and_then(unix));
                out.reactions.extend(reaction(hash.as_deref(), &data, at));
                out.removed.extend(data.remove.as_ref().and_then(|r| r.target.as_deref()).and_then(hex_hash));
                let Some(signer) = signer else { continue };
                let Some(family) = data.kind.as_deref().and_then(Family::of) else { continue };
                let Some(fid) = data.fid.filter(|fid| (1..=u64::from(u32::MAX)).contains(fid)) else { continue };
                let Some(signer) = normalize_key(&signer) else { continue };
                let Some(at) = at else { continue };
                out.actions.push(Action { fid, signer, at, family });
            }
            // Not activity, but a reaction revoked or pruned is gone.
            Some("HUB_EVENT_TYPE_REVOKE_MESSAGE" | "HUB_EVENT_TYPE_PRUNE_MESSAGE") => {
                let Some(Message { hash: Some(hash), data: Some(data), .. }) =
                    event.revoke.or(event.prune).and_then(|m| m.message)
                else {
                    continue;
                };
                if reaction(Some(&hash), &data, Some(0)).is_some() {
                    out.removed.extend(hex_hash(&hash));
                }
            }
            // A key added for an account: which app requested it is in its
            // signed key request (`provenance`), so it needn't be looked up.
            Some("HUB_EVENT_TYPE_MERGE_ON_CHAIN_EVENT") => {
                let Some(ChainEvent { fid: Some(fid), signer: Some(body), .. }) = event.chain.and_then(|c| c.event)
                else {
                    continue;
                };
                if body.kind.as_deref() != Some("SIGNER_EVENT_TYPE_ADD") {
                    continue;
                }
                let Some(pair) = body.key.as_deref().and_then(normalize_key).and_then(|key| key_pair(fid, &key)) else {
                    continue;
                };
                let app = body
                    .metadata
                    .as_deref()
                    .filter(|_| body.metadata_type == Some(1))
                    .and_then(base64_decode)
                    .and_then(|metadata| request_fid_from_metadata(&metadata));
                out.keys.push((pair, app.unwrap_or(0)));
            }
            _ => {}
        }
    }
    Some(out)
}

/// The isolated format of canonical activity counts. Older sets are preserved.
pub const VERIFICATION_VERSION: u8 = 2;
/// Canonical event source, with shard parameters recorded separately.
pub const ACTIVITY_SOURCE: &str = "https://haatz.quilibrium.com/v1/events";

/// One canonical block, carried across event pages. `seen` includes its
/// confirmation; only a whole block advances verified coverage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockProgress {
    pub shard: u32,
    pub block: u64,
    pub at: i64,
    pub total: u32,
    pub seen: u32,
}

impl BlockProgress {
    pub fn complete(&self) -> bool {
        self.seen == self.total
    }

    pub fn last_id(&self) -> u64 {
        (self.block << 14) | u64::from(self.seen - 1)
    }
}

/// A checked page and its durable per-shard progress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedPage {
    pub page: ActivityPage,
    pub block: Option<BlockProgress>,
    pub gaps: Vec<(i64, i64)>,
}

/// A one-event reverse probe establishes only the starting height; it
/// cannot establish temporal coverage or contribute a count.
pub fn activity_head(json: &[u8], shard: u32) -> Option<u64> {
    let page: Page = serde_json::from_slice(json).ok()?;
    if page.events.len() != 1 {
        return None;
    }
    let event = page.events.first()?;
    (event.shard == Some(shard) && event.block == Some(event.id >> 14)).then_some(event.id)
}

/// Read ascending Hypersnap events after `after`. Reject a malformed page
/// as a whole, so its cursor remains retryable. Timestamps come exclusively
/// from canonical block confirmations, including a carried partial block.
pub fn verified_activity(
    json: &[u8],
    shard: u32,
    after: u64,
    carried: Option<&BlockProgress>,
    now: i64,
) -> Option<VerifiedPage> {
    let mut input: Page = serde_json::from_slice(json).ok()?;
    let mut previous = after;
    let mut block = carried.cloned();
    if block.as_ref().is_some_and(|b| {
        b.shard != shard || b.seen == 0 || b.seen > b.total || b.total > 16_384 || b.last_id() != after
    }) {
        return None;
    }
    let mut canonical = HashMap::new();
    if let Some(b) = &block {
        canonical.insert(b.block, b.at);
    }
    let mut gaps = Vec::new();
    let mut reached = None;
    let mut started = None;
    for event in &mut input.events {
        if !matches!(
            event.kind.as_deref(),
            Some(
                "HUB_EVENT_TYPE_BLOCK_CONFIRMED"
                    | "HUB_EVENT_TYPE_MERGE_MESSAGE"
                    | "HUB_EVENT_TYPE_REVOKE_MESSAGE"
                    | "HUB_EVENT_TYPE_PRUNE_MESSAGE"
                    | "HUB_EVENT_TYPE_MERGE_ON_CHAIN_EVENT"
                    | "HUB_EVENT_TYPE_MERGE_USERNAME_PROOF"
                    | "HUB_EVENT_TYPE_MERGE_FAILURE"
                    | "HUB_EVENT_TYPE_CHANNEL_OWNER_CHANGE_HINT"
            )
        ) {
            return None;
        }
        let height = event.id >> 14;
        let sequence = (event.id & 16_383) as u32;
        if event.id <= previous || event.block != Some(height) || event.shard != Some(shard) {
            return None;
        }
        let new_block = block.as_ref().is_none_or(|b| b.block != height);
        if new_block {
            if sequence != 0 || event.kind.as_deref() != Some("HUB_EVENT_TYPE_BLOCK_CONFIRMED") {
                return None;
            }
            let confirmed = event.confirmed.as_ref()?;
            let at = unix(confirmed.timestamp?)?;
            let total = confirmed.total.filter(|n| (1..=16_384).contains(n))?;
            if confirmed.block != Some(height) || confirmed.shard != Some(shard) || at > now {
                return None;
            }
            if let Some(prior) = &block {
                if height <= prior.block || at < prior.at {
                    return None;
                }
                if height != prior.block + 1 || !prior.complete() {
                    gaps.push((prior.at, at.saturating_add(1)));
                }
            }
            canonical.insert(height, at);
            started = Some(started.map_or(at, |s: i64| s.min(at)));
            block = Some(BlockProgress { shard, block: height, at, total, seen: 1 });
        } else {
            let current = block.as_mut()?;
            if sequence != current.seen
                || current.complete()
                || event.kind.as_deref() == Some("HUB_EVENT_TYPE_BLOCK_CONFIRMED")
            {
                return None;
            }
            current.seen += 1;
        }
        let current = block.as_ref()?;
        if current.complete() {
            reached = Some(current.at - 1);
        }
        previous = event.id;
    }
    // Extraction uses the canonical time table, including the carried block;
    // a user's signed timestamp never supplies a counting timestamp.
    let mut out = ActivityPage { count: input.events.len(), ..ActivityPage::default() };
    for event in input.events {
        out.last_id = Some(event.id);
        out.first_block = out.first_block.or(event.block);
        if event.kind.as_deref() == Some("HUB_EVENT_TYPE_MERGE_MESSAGE") {
            let Some(Message { hash, data: Some(data), signer }) = event.merge.and_then(|m| m.message) else {
                return None;
            };
            let at = *canonical.get(&event.block?)?;
            out.reactions.extend(reaction(hash.as_deref(), &data, Some(at)));
            out.removed.extend(data.remove.as_ref().and_then(|r| r.target.as_deref()).and_then(hex_hash));
            let kind = data.kind.as_deref()?;
            if kind != "MESSAGE_TYPE_NONE"
                && kind != "MESSAGE_TYPE_SIGNER_ADD"
                && kind != "MESSAGE_TYPE_SIGNER_REMOVE"
                && Family::of(kind).is_none()
            {
                return None;
            }
            if let Some(family) = Family::of(kind) {
                let fid = data.fid.filter(|fid| (1..=u64::from(u32::MAX)).contains(fid))?;
                let signer = exact_signer(signer.as_deref()?)?;
                out.actions.push(Action { fid, signer, at, family });
            }
        } else if matches!(
            event.kind.as_deref(),
            Some("HUB_EVENT_TYPE_REVOKE_MESSAGE" | "HUB_EVENT_TYPE_PRUNE_MESSAGE")
        ) {
            if let Some(Message { hash: Some(hash), data: Some(data), .. }) =
                event.revoke.or(event.prune).and_then(|m| m.message)
                && reaction(Some(&hash), &data, Some(0)).is_some()
            {
                out.removed.extend(hex_hash(&hash));
            }
        } else if event.kind.as_deref() == Some("HUB_EVENT_TYPE_MERGE_ON_CHAIN_EVENT")
            && let Some(ChainEvent { kind: Some(kind), fid: Some(fid), signer: Some(body) }) =
                event.chain.and_then(|c| c.event)
            && kind == "EVENT_TYPE_SIGNER"
            && body.key_type == Some(1)
            && body.kind.as_deref() == Some("SIGNER_EVENT_TYPE_ADD")
            && let Some(identity) = body.key.as_deref().and_then(|key| signer_identity(fid, key))
        {
            let app = match (body.metadata_type, body.metadata.as_deref()) {
                (Some(0), Some("")) => Some(0),
                (Some(1), Some(metadata)) => verified_request_fid_from_metadata(metadata),
                _ => None,
            };
            if let Some(app) = app {
                out.exact_keys.push((identity, app));
            }
        }
        // Missing or malformed registration metadata remains unknown.
    }
    out.reached = reached;
    out.started = started;
    Some(VerifiedPage { page: out, block, gaps })
}

/// An Ed25519 key, with every byte retained.
pub fn exact_signer(key: &str) -> Option<String> {
    if key.len() == 66
        && key.starts_with("0x")
        && key.as_bytes()[2..].iter().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
    {
        Some(key.to_owned())
    } else {
        normalize_key(key).filter(|key| key.len() == 66)
    }
}

/// A full signer identity. Its fixed-width decimal prefix orders accounts
/// together without truncating either FIDs or keys.
pub fn signer_identity(fid: u64, key: &str) -> Option<String> {
    if !(1..=u64::from(u32::MAX)).contains(&fid) {
        return None;
    }
    Some(format!("{fid:010}:{}", exact_signer(key)?))
}

pub fn identity_fid(identity: &str) -> Option<u64> {
    let bytes = identity.as_bytes();
    if bytes.len() != 77
        || &bytes[10..13] != b":0x"
        || !bytes[13..].iter().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
    {
        return None;
    }
    let mut fid = 0_u32;
    for &digit in &bytes[..10] {
        if !digit.is_ascii_digit() {
            return None;
        }
        fid = fid.checked_mul(10)?.checked_add(u32::from(digit - b'0'))?;
    }
    (fid > 0).then_some(u64::from(fid))
}

/// Sorted, complete signer identities, serialized without integer or key
/// prefix loss. Old compact key sets are never read in this namespace.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExactKeys(Vec<String>);

impl ExactKeys {
    pub fn of(keys: impl IntoIterator<Item = String>) -> Self {
        let mut keys: Vec<_> = keys.into_iter().collect();
        keys.sort_unstable();
        keys.dedup();
        Self(keys)
    }
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }
    pub fn extend(&mut self, keys: &Self) {
        let mut merged = Vec::with_capacity(self.0.len() + keys.0.len());
        let mut own = std::mem::take(&mut self.0).into_iter().peekable();
        let mut other = keys.0.iter().peekable();
        while let (Some(a), Some(b)) = (own.peek(), other.peek()) {
            match a.cmp(b) {
                std::cmp::Ordering::Less => merged.push(own.next().expect("peeked")),
                std::cmp::Ordering::Greater => merged.push(other.next().expect("peeked").clone()),
                std::cmp::Ordering::Equal => {
                    merged.push(own.next().expect("peeked"));
                    other.next();
                }
            }
        }
        merged.extend(own);
        merged.extend(other.cloned());
        self.0 = merged;
    }
    pub fn encode(&self) -> String {
        serde_json::to_string(&self.0).expect("string array")
    }
    pub fn decode(text: &str) -> Option<Self> {
        let keys: Vec<String> = serde_json::from_str(text).ok()?;
        if !keys.iter().all(|k| identity_fid(k).is_some()) || !keys.windows(2).all(|k| k[0] < k[1]) {
            return None;
        }
        Some(Self(keys))
    }
    pub fn chunks(&self, size: usize) -> Vec<&[String]> {
        let mut chunks = Vec::new();
        let mut rest = self.0.as_slice();
        while !rest.is_empty() {
            let mut end = size.max(1).min(rest.len());
            while end < rest.len() && identity_fid(&rest[end]) == identity_fid(&rest[end - 1]) {
                end += 1;
            }
            let (chunk, after) = rest.split_at(end);
            chunks.push(chunk);
            rest = after;
        }
        chunks
    }
}

/// The UTC day of Unix time `at`, as days since 1970-01-01.
pub fn day(at: i64) -> i64 {
    at.div_euclid(86_400)
}

/// The UTC hour of Unix time `at`, as hours since 1970-01-01T00:00Z.
pub fn hour(at: i64) -> i64 {
    at.div_euclid(3_600)
}

/// The hour an action at Unix time `at` is counted in. Hour `h` holds
/// `(h·3600, (h+1)·3600]`, start-exclusive and end-inclusive as SnapMeter's
/// windows are, so the 24 hours before `t` (on the hour) are its rolling
/// window `(t − 24 h, t]` exactly.
pub fn bucket(at: i64) -> i64 {
    (at - 1).div_euclid(3_600)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Each base64 character's value, or 255.
const B64_VALUES: [u8; 256] = {
    let mut table = [255u8; 256];
    let mut i = 0;
    while i < 64 {
        table[B64[i] as usize] = i as u8;
        i += 1;
    }
    table
};

/// Increasing `values` as stored: each one's gap from the one before as a
/// LEB128 varint, in unpadded URL-safe base64.
fn encode_gaps(values: impl IntoIterator<Item = u64>) -> String {
    let mut bytes = Vec::new();
    let mut previous = 0u64;
    for value in values {
        let mut gap = value - previous;
        previous = value;
        loop {
            let low = (gap & 0x7f) as u8;
            gap >>= 7;
            if gap == 0 {
                bytes.push(low);
                break;
            }
            bytes.push(low | 0x80);
        }
    }
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, &b| n << 8 | u32::from(b)) << (8 * (3 - chunk.len()));
        for i in 0..=chunk.len() {
            out.push(char::from(B64[(n >> (18 - 6 * i) & 63) as usize]));
        }
    }
    out
}

/// Stored values, increasing; `None` if `text` isn't a stored set.
fn decode_gaps(text: &str) -> Option<Vec<u64>> {
    // Unpadded base64 never leaves one character over.
    if text.len() % 4 == 1 {
        return None;
    }
    let mut bytes = Vec::with_capacity(text.len() * 3 / 4);
    let (mut buffer, mut bits) = (0u32, 0);
    for c in text.bytes() {
        let value = B64_VALUES[c as usize];
        if value == 255 {
            return None;
        }
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    let mut values = Vec::with_capacity(bytes.len() / 2);
    let (mut previous, mut gap, mut shift) = (0u64, 0u64, 0);
    for b in bytes {
        let low = u64::from(b & 0x7f);
        // A u64 takes 10 bytes at most, the last holding one bit.
        if shift > 63 || (shift == 63 && low > 1) {
            return None;
        }
        gap |= low << shift;
        if b & 0x80 == 0 {
            previous = previous.checked_add(gap)?;
            values.push(previous);
            (gap, shift) = (0, 0);
        } else {
            shift += 7;
        }
    }
    // Every value after the first is a positive gap away, so they increase.
    (shift == 0 && values.windows(2).all(|w| w[0] < w[1])).then_some(values)
}

/// The union of two increasing lists.
fn merge<T: Ord + Copy>(a: &[T], b: &[T]) -> Vec<T> {
    let mut all = Vec::with_capacity(a.len() + b.len());
    let (mut a, mut b) = (a.iter().peekable(), b.iter().peekable());
    loop {
        let next = match (a.peek(), b.peek()) {
            (Some(&&x), Some(&&y)) if x < y => a.next(),
            (Some(&&x), Some(&&y)) if x > y => b.next(),
            (Some(_), Some(_)) => {
                b.next();
                a.next()
            }
            (Some(_), None) => a.next(),
            (None, Some(_)) => b.next(),
            (None, None) => break,
        };
        all.extend(next.copied());
    }
    all
}

/// A set of fids, sorted. Stored as text (`encode_gaps`): a day's 6,000
/// active accounts take about 16 KB.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fids(Vec<u32>);

impl Fids {
    /// The set of `fids`, in any order, with repeats.
    pub fn of(fids: impl IntoIterator<Item = u64>) -> Fids {
        let mut fids: Vec<u32> = fids.into_iter().filter_map(|f| u32::try_from(f).ok()).collect();
        fids.sort_unstable();
        fids.dedup();
        Fids(fids)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether `fid` is in the set: a binary search, with no allocation.
    pub fn contains(&self, fid: u64) -> bool {
        u32::try_from(fid).is_ok_and(|fid| self.0.binary_search(&fid).is_ok())
    }

    /// Add `other`'s fids.
    pub fn extend(&mut self, other: &Fids) {
        self.0 = merge(&self.0, &other.0);
    }

    /// As stored.
    pub fn encode(&self) -> String {
        encode_gaps(self.0.iter().map(|&fid| u64::from(fid)))
    }

    /// From storage; `None` if `text` isn't a stored set of fids.
    pub fn decode(text: &str) -> Option<Fids> {
        let values: Vec<u32> = decode_gaps(text)?.into_iter().map(|v| u32::try_from(v).ok()).collect::<Option<_>>()?;
        let set = Fids(values);
        (set.0.first().is_none_or(|fid| *fid > 0) && set.encode() == text).then_some(set)
    }

    /// How many distinct fids `sets` hold between them.
    pub fn union_len<'a>(sets: impl IntoIterator<Item = &'a Fids>) -> usize {
        let sets: Vec<&Fids> = sets.into_iter().collect();
        let mut values = Vec::with_capacity(sets.iter().map(|set| set.len()).sum());
        for set in sets {
            values.extend_from_slice(&set.0);
        }
        values.sort_unstable();
        values.dedup();
        values.len()
    }
}

/// A signing key as the cron keeps it: its account's fid in the high 32
/// bits and the key's first four bytes in the low 32, which tell an
/// account's keys apart. `None` for an fid of 2³¹ or more, past what D1's
/// signed integers hold.
pub fn key_pair(fid: u64, key: &str) -> Option<u64> {
    if fid >= 1 << 31 {
        return None;
    }
    let hex = key.strip_prefix("0x").unwrap_or(key);
    let head = u32::from_str_radix(hex.get(..hex.len().min(8))?, 16).ok()?;
    Some(fid << 32 | u64::from(head))
}

/// The fid of the account a `key_pair` signs for.
pub fn pair_fid(pair: u64) -> u64 {
    pair >> 32
}

/// A set of signing keys (`key_pair`), sorted, so each account's keys are
/// together. Stored as text, as `Fids` are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Keys(Vec<u64>);

impl Keys {
    /// The set of `pairs`, in any order, with repeats.
    pub fn of(pairs: impl IntoIterator<Item = u64>) -> Keys {
        let mut pairs: Vec<u64> = pairs.into_iter().collect();
        pairs.sort_unstable();
        pairs.dedup();
        Keys(pairs)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_slice(&self) -> &[u64] {
        &self.0
    }

    /// Add `other`'s keys.
    pub fn extend(&mut self, other: &Keys) {
        self.0 = merge(&self.0, &other.0);
    }

    pub fn encode(&self) -> String {
        encode_gaps(self.0.iter().copied())
    }

    pub fn decode(text: &str) -> Option<Keys> {
        decode_gaps(text).map(Keys)
    }

    /// The keys in runs of about `size`, cut only between accounts, so an
    /// account counted in one run is in no other.
    pub fn chunks(&self, size: usize) -> Vec<&[u64]> {
        let mut out = Vec::new();
        let mut rest = self.0.as_slice();
        while !rest.is_empty() {
            let mut end = size.max(1).min(rest.len());
            while end < rest.len() && pair_fid(rest[end]) == pair_fid(rest[end - 1]) {
                end += 1;
            }
            let (chunk, after) = rest.split_at(end);
            out.push(chunk);
            rest = after;
        }
        out
    }
}

/// A UTC day as `YYYY-MM-DD`.
pub fn day_name(day: i64) -> String {
    // Howard Hinnant's civil-from-days.
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Hours of sets the follower keeps: two windows of 24 and the hour in
/// progress.
pub const KEEP_HOURS: i64 = 49;
/// Days of sets it keeps: the 30-day window.
pub const KEEP_DAYS: i64 = 30;
/// Days of counts it keeps.
pub const HISTORY_DAYS: usize = 60;

/// A canonical set name, isolated from preserved legacy sets.
pub fn hour_set(hour: i64) -> String {
    format!("v2h{hour}")
}

pub fn day_set(day: i64) -> String {
    format!("v2d{day}")
}

/// Days of the union the cron keeps once a day for a list to ask who of
/// its people was active this week (`week_set`).
pub const WEEK_DAYS: i64 = 7;

/// The name the union of the `WEEK_DAYS` days to the end of `day` is
/// stored under: `w<day>`. The cron writes it only once those days were
/// read in full (`Follow::covered`).
pub fn week_set(day: i64) -> String {
    format!("v2w{day}")
}

/// Which of some accounts were active in the `WEEK_DAYS` days to the end of
/// `day`, a UTC day, read in full.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveWeek {
    pub day: i64,
    /// Those of the accounts asked about that were, in the order asked.
    pub active: Vec<u64>,
}

impl ActiveWeek {
    /// Which of `fids` are in `week`, the union of the days to `day`.
    pub fn among(day: i64, week: &Fids, fids: &[u64]) -> ActiveWeek {
        ActiveWeek { day, active: fids.iter().copied().filter(|&fid| week.contains(fid)).collect() }
    }
}

/// The name an hour's set of active keys is stored under: `k<hour>`.
pub fn key_set(hour: i64) -> String {
    format!("v2k{hour}")
}

/// One UTC day's counts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayCount {
    /// Days since 1970-01-01.
    pub day: i64,
    /// Accounts active that day.
    pub active: u64,
    /// Counted messages, by family.
    #[serde(default)]
    pub actions: HashMap<Family, u64>,
    /// Read from its start to its end, with no gap.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub complete: bool,
}

/// How far one shard's events have been read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reached {
    pub shard: u32,
    /// The newest block time read, Unix seconds.
    pub at: i64,
}

/// The follower's saved state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Follow {
    /// Canonical verification format, isolated from legacy counts.
    #[serde(default)]
    pub verification_version: u8,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub starts: Vec<Reached>,
    #[serde(default)]
    pub blocks: Vec<BlockProgress>,
    /// Where each shard's events have been read to.
    #[serde(default)]
    pub cursors: Vec<Cursor>,
    #[serde(default)]
    pub reached: Vec<Reached>,
    /// The first block time read, Unix seconds: counting began here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    /// Spans whose events were never read, because the node no longer had
    /// them, as Unix seconds `[from, to)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<(i64, i64)>,
    /// Each day's counts, oldest first; the last ones may be in progress.
    #[serde(default)]
    pub days: Vec<DayCount>,
    /// Accounts active in the 30 days to the end of `day`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_30d: Option<Window>,
    /// The first block time from which each hour's active keys were kept,
    /// Unix seconds: counting by app began here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys_since: Option<i64>,
    /// Accounts active by app, counted again each hour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps_24h: Option<AppsWindow>,
    /// Active keys (`key_pair`) whose app the last count didn't know, to
    /// look up.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backlog: Vec<String>,
    /// The accounts of the apps with the most active accounts, read once a
    /// day, on `named`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub app_accounts: Vec<UserSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub named: Option<i64>,
}

/// Apps whose accounts the activity page names.
pub const NAMED_APPS: usize = 20;

/// Accounts active through one app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppActive {
    /// The app's fid.
    pub app: u64,
    pub active: u64,
}

/// Accounts active in the 24 hours to `to`, `(to − 24 h, to]`, by the app
/// that requested the key they signed with: an account that used two apps
/// counts for each.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppsWindow {
    /// Only the canonical full-key counter sets this format marker.
    #[serde(default)]
    pub verification_version: u8,
    /// The window's end, Unix seconds, on the hour.
    pub to: i64,
    /// When it was counted, Unix seconds.
    pub counted: i64,
    /// Accounts active in the window, through any app.
    pub active: u64,
    /// By app, most first.
    #[serde(default)]
    pub apps: Vec<AppActive>,
    /// Accounts that signed with a key no app requested.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub none: u64,
    /// Accounts that signed with a key whose app isn't known yet.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub unknown: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl AppsWindow {
    /// From `(app, accounts)` counts: app `None` for keys not looked up
    /// yet, and 0 for keys no app requested. Counts for the same app add
    /// up, so they can come from runs of keys that share no account.
    pub fn of(to: i64, counted: i64, active: u64, counts: impl IntoIterator<Item = (Option<u64>, u64)>) -> AppsWindow {
        let mut window = AppsWindow { to, counted, active, ..AppsWindow::default() };
        let mut apps: HashMap<u64, u64> = HashMap::new();
        for (app, n) in counts {
            match app {
                None => window.unknown += n,
                Some(0) => window.none += n,
                Some(app) => *apps.entry(app).or_default() += n,
            }
        }
        window.apps = apps.into_iter().map(|(app, active)| AppActive { app, active }).collect();
        window.apps.sort_by(|a, b| b.active.cmp(&a.active).then(a.app.cmp(&b.app)));
        window
    }

    /// A canonical full-key window with coherent bounds and counts. Old
    /// cached Home payloads default to version zero and must not be shown.
    pub fn is_verified(&self) -> bool {
        self.verification_version == VERIFICATION_VERSION
            && self.to >= FARCASTER_EPOCH
            && self.to.rem_euclid(3_600) == 0
            && self.counted >= self.to
            && self.active <= u64::from(u32::MAX)
            && self.unknown <= self.active
            && self.none <= self.active
            && self.apps.iter().all(|app| app.app > 0 && app.active <= self.active)
            && self.apps.iter().enumerate().all(|(i, app)| !self.apps[..i].iter().any(|prior| prior.app == app.app))
    }

    /// Accounts active through `app`.
    pub fn through(&self, app: u64) -> u64 {
        self.apps.iter().find(|a| a.app == app).map_or(0, |a| a.active)
    }

    /// The whole percent of active accounts whose keys' apps are all known
    /// (rounded down, so 100 means all of them).
    pub fn known_percent(&self) -> u64 {
        (self.active.saturating_sub(self.unknown) * 100).checked_div(self.active).unwrap_or(100)
    }

    /// Only the apps `keep` names, for a page that shows those.
    pub fn only(&self, keep: impl Fn(u64) -> bool) -> AppsWindow {
        AppsWindow { apps: self.apps.iter().copied().filter(|a| keep(a.app)).collect(), ..self.clone() }
    }
}

/// A count over the days to the end of `day`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub day: i64,
    pub active: u64,
    pub complete: bool,
}

impl Follow {
    /// Structural checks for this isolated state format. A corrupt state
    /// must be repaired explicitly, never silently reset or certified.
    pub fn valid(&self, now: i64) -> bool {
        let unique = |shards: Vec<u32>| {
            shards.iter().all(|s| SHARDS.contains(s))
                && shards.iter().enumerate().all(|(i, s)| !shards[..i].contains(s))
        };
        self.verification_version == VERIFICATION_VERSION
            && unique(self.cursors.iter().map(|s| s.shard).collect())
            && unique(self.reached.iter().map(|s| s.shard).collect())
            && unique(self.starts.iter().map(|s| s.shard).collect())
            && unique(self.blocks.iter().map(|s| s.shard).collect())
            && self.cursors.iter().all(|c| {
                self.blocks.iter().any(|b| {
                    b.shard == c.shard
                        && b.seen > 0
                        && b.seen <= b.total
                        && b.total <= 16_384
                        && b.block == c.event_id >> 14
                        && b.last_id() == c.event_id
                })
            })
            && self.blocks.iter().all(|b| {
                b.at >= FARCASTER_EPOCH
                    && b.at <= now
                    && self.cursors.iter().any(|c| c.shard == b.shard)
                    && self.starts.iter().any(|s| s.shard == b.shard && s.at <= b.at + 1)
                    && (!b.complete() || self.reached.iter().any(|r| r.shard == b.shard && r.at == b.at - 1))
            })
            && self.reached.iter().all(|r| {
                r.at >= FARCASTER_EPOCH && self.blocks.iter().any(|b| b.shard == r.shard && r.at < b.at) && r.at <= now
            })
            && self
                .starts
                .iter()
                .all(|s| s.at >= FARCASTER_EPOCH && s.at <= now + 1 && self.blocks.iter().any(|b| b.shard == s.shard))
            && self.since
                == SHARDS
                    .into_iter()
                    .map(|shard| self.starts.iter().find(|s| s.shard == shard).map(|s| s.at))
                    .collect::<Option<Vec<_>>>()
                    .and_then(|starts| starts.into_iter().max())
            && self.keys_since == self.since
            && self.gaps.iter().all(|&(from, to)| from >= FARCASTER_EPOCH && from < to && to <= now + 1)
            && self.days.windows(2).all(|d| d[0].day < d[1].day)
            && self.days.iter().all(|d| {
                (day(FARCASTER_EPOCH)..=day(now)).contains(&d.day)
                    && d.active <= u64::from(u32::MAX)
                    && (!d.complete || self.covered(d.day * 86_400, (d.day + 1) * 86_400))
            })
            && self.active_30d.is_none_or(|w| {
                (day(FARCASTER_EPOCH)..day(now)).contains(&w.day)
                    && w.active <= u64::from(u32::MAX)
                    && self.since.is_some_and(|since| since <= w.day * 86_400)
                    && self.reached().is_some_and(|reached| reached >= (w.day + 1) * 86_400)
                    && (!w.complete || self.covered((w.day - 29) * 86_400, (w.day + 1) * 86_400))
            })
            && self.backlog.iter().all(|k| identity_fid(k).is_some())
            && self
                .apps_24h
                .as_ref()
                .is_none_or(|w| w.is_verified() && w.counted <= now && self.covered_through(w.to - 86_400, w.to))
    }

    /// How far every shard has been read, Unix seconds: the least of them.
    pub fn reached(&self) -> Option<i64> {
        SHARDS
            .into_iter()
            .map(|shard| self.reached.iter().find(|r| r.shard == shard).map(|r| r.at))
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .min()
    }

    /// Whether `[from, to)` was read throughout: after counting began, with
    /// no gap in it, and every shard past its end.
    pub fn covered(&self, from: i64, to: i64) -> bool {
        self.verification_version == VERIFICATION_VERSION
            && SHARDS
                .into_iter()
                .all(|shard| self.starts.iter().find(|s| s.shard == shard).is_some_and(|s| s.at <= from))
            && self.since.is_some_and(|since| since <= from)
            && self.reached().is_some_and(|reached| reached >= to)
            && !self.gaps.iter().any(|&(a, b)| a < to && b > from)
    }

    /// Hourly and rolling windows include their right edge, unlike UTC
    /// calendar days. A missing event at `to` invalidates their count.
    pub fn covered_through(&self, from: i64, to: i64) -> bool {
        self.covered(from, to) && !self.gaps.iter().any(|&(a, b)| a <= to && b > from)
    }

    /// Whether accounts by app are due to be counted for the window to `to`
    /// at `now`: its keys were all kept, and it is a window not counted
    /// yet, or, while more than 1% of accounts' apps aren't known, the last
    /// count is 10 minutes old (keys are looked up a few at a time).
    pub fn apps_due(&self, to: i64, now: i64) -> bool {
        let kept = self.keys_since.is_some_and(|since| since <= to - 86_400) && self.covered_through(to - 86_400, to);
        kept && self.apps_24h.as_ref().is_none_or(|w| w.to < to || (w.known_percent() < 99 && now - w.counted >= 600))
    }

    /// The counts of `day`, made if new.
    pub fn day_mut(&mut self, day: i64) -> &mut DayCount {
        let at = match self.days.binary_search_by_key(&day, |d| d.day) {
            Ok(at) => at,
            Err(at) => {
                self.days.insert(at, DayCount { day, ..DayCount::default() });
                at
            }
        };
        &mut self.days[at]
    }

    /// Mark finished days that were read throughout as complete, and drop
    /// counts older than `HISTORY_DAYS`.
    pub fn settle(&mut self) {
        let covered: Vec<bool> = self.days.iter().map(|d| self.covered(d.day * 86_400, (d.day + 1) * 86_400)).collect();
        for (d, covered) in self.days.iter_mut().zip(covered) {
            d.complete = covered;
        }
        let excess = self.days.len().saturating_sub(HISTORY_DAYS);
        self.days.drain(..excess);
    }
}

/// What a run read, by hour and by day.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Batch {
    pub hours: HashMap<i64, Fids>,
    pub days: HashMap<i64, Fids>,
    pub actions: HashMap<i64, HashMap<Family, u64>>,
    /// The keys that signed, by hour.
    #[cfg(test)]
    pub keys: HashMap<i64, Keys>,
    pub exact_keys: HashMap<i64, ExactKeys>,
}

impl Batch {
    pub fn of(actions: &[Action]) -> Batch {
        let mut hours: HashMap<i64, Vec<u64>> = HashMap::new();
        let mut days: HashMap<i64, Vec<u64>> = HashMap::new();
        let mut counts: HashMap<i64, HashMap<Family, u64>> = HashMap::new();
        #[cfg(test)]
        let mut keys: HashMap<i64, Vec<u64>> = HashMap::new();
        let mut exact_keys: HashMap<i64, Vec<String>> = HashMap::new();
        for a in actions {
            exact_keys.entry(bucket(a.at)).or_default().extend(signer_identity(a.fid, &a.signer));
            #[cfg(test)]
            keys.entry(bucket(a.at)).or_default().extend(key_pair(a.fid, &a.signer));
            hours.entry(bucket(a.at)).or_default().push(a.fid);
            days.entry(day(a.at)).or_default().push(a.fid);
            *counts.entry(day(a.at)).or_default().entry(a.family).or_default() += 1;
        }
        Batch {
            hours: hours.into_iter().map(|(h, f)| (h, Fids::of(f))).collect(),
            days: days.into_iter().map(|(d, f)| (d, Fids::of(f))).collect(),
            actions: counts,
            #[cfg(test)]
            keys: keys.into_iter().map(|(h, k)| (h, Keys::of(k))).collect(),
            exact_keys: exact_keys.into_iter().map(|(h, k)| (h, ExactKeys::of(k))).collect(),
        }
    }
}

/// The evidence behind one shard's canonical coverage. A partial block is
/// exposed separately from the time through which counting is complete.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShardVerification {
    pub shard: u32,
    pub since: Option<i64>,
    pub reached: Option<i64>,
    pub cursor: Option<u64>,
    pub block: Option<u64>,
    pub block_at: Option<i64>,
    pub total_events: Option<u32>,
    pub read_events: Option<u32>,
}

/// What `/api/v1/activity` answers: Farcaster's active accounts, as the
/// cron last counted them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityPayload {
    #[serde(default)]
    pub verification_version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_since: Option<i64>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub shards: Vec<ShardVerification>,
    /// How far the events had been read, Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reached: Option<i64>,
    /// The end of the 24-hour windows, Unix seconds: the last hour read
    /// whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rolling_to: Option<i64>,
    /// Accounts active in the 24 hours to `rolling_to`, `(t − 24 h, t]`, and
    /// in the 24 before them; none while those hours weren't all read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rolling_24h: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_24h: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_30d: Option<Window>,
    /// Accounts active by app in the 24 hours to its `to`, counted each
    /// hour; none until a whole window's keys were kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps_24h: Option<AppsWindow>,
    /// The accounts of the apps with the most active accounts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub app_accounts: Vec<UserSummary>,
    /// The latest 30 UTC days, oldest first; the last is in progress.
    #[serde(default)]
    pub days: Vec<DayCount>,
    /// Accounts active in each of the `HOURLY` hours to `rolling_to`,
    /// oldest first, each hour `(h − 1 h, h]` as `bucket` keeps it: an
    /// account active in several hours counts in each. `None` for an hour
    /// not read whole; empty before any was.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hourly: Vec<Option<u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<(i64, i64)>,
    pub meta: crate::api::Meta,
}

impl ActivityPayload {
    /// Validate source-observed canonical evidence, including partial first
    /// runs. This does not attest independent consensus or reorg detection.
    /// A valid epoch is distinct from any particular window being complete.
    pub fn is_verified(&self, now: i64) -> bool {
        if self.verification_version != VERIFICATION_VERSION
            || self.source != ACTIVITY_SOURCE
            || self.meta.backend != crate::provenance::Backend::Hypersnap
            || self.meta.fetched_at.div_euclid(1_000) > now
            || self.meta.fetched_at < FARCASTER_EPOCH * 1_000
            || self.shards.len() != SHARDS.len()
            || !SHARDS.into_iter().all(|shard| self.shards.iter().filter(|s| s.shard == shard).count() == 1)
        {
            return false;
        }
        for shard in &self.shards {
            match (shard.since, shard.block, shard.block_at, shard.total_events, shard.read_events, shard.cursor) {
                (None, None, None, None, None, None) if shard.reached.is_none() => {}
                (Some(since), Some(block), Some(at), Some(total), Some(read), Some(cursor)) => {
                    if !(1..=16_384).contains(&total)
                        || read == 0
                        || read > total
                        || block != cursor >> 14
                        || u64::from(read - 1) != cursor & 16_383
                        || at < FARCASTER_EPOCH
                        || at > self.meta.fetched_at.div_euclid(1_000)
                        || since < FARCASTER_EPOCH
                        || since > at + 1
                        || shard.reached.is_some_and(|reached| reached < FARCASTER_EPOCH || reached >= at)
                        || (read == total && shard.reached != Some(at - 1))
                    {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        let since = self.shards.iter().map(|s| s.since).collect::<Option<Vec<_>>>().and_then(|s| s.into_iter().max());
        let reached =
            self.shards.iter().map(|s| s.reached).collect::<Option<Vec<_>>>().and_then(|s| s.into_iter().min());
        if self.verified_since != since
            || self.since != since
            || self.reached != reached
            || self.rolling_to != reached.map(rolling_to)
            || self.gaps.iter().any(|&(from, to)| from < FARCASTER_EPOCH || from >= to || to > now + 1)
            || !self.days.windows(2).all(|d| d[0].day < d[1].day)
            || self.days.iter().any(|d| {
                d.day < day(FARCASTER_EPOCH) - 1
                    || d.day > day(now)
                    || d.active > u64::from(u32::MAX)
                    || (d.complete && !self.covers(d.day * 86_400, (d.day + 1) * 86_400))
            })
        {
            return false;
        }
        if self.rolling_24h.is_some() && !self.rolling_to.is_some_and(|to| self.covers_through(to - 86_400, to))
            || self.previous_24h.is_some()
                && !self.rolling_to.is_some_and(|to| self.covers_through(to - 2 * 86_400, to - 86_400))
            || self.rolling_24h.is_some_and(|n| n > u64::from(u32::MAX))
            || self.previous_24h.is_some_and(|n| n > u64::from(u32::MAX))
        {
            return false;
        }
        if !self.hourly.is_empty() {
            let Some(to) = self.rolling_to else {
                return false;
            };
            if self.hourly.len() != HOURLY as usize
                || self.hourly.iter().enumerate().any(|(i, n)| {
                    let end = to - (HOURLY - 1 - i as i64) * 3_600;
                    n.is_some_and(|count| count > u64::from(u32::MAX) || !self.covers_through(end - 3_600, end))
                })
            {
                return false;
            }
        }
        if let Some(window) = self.active_30d
            && (window.day < day(FARCASTER_EPOCH)
                || window.day >= day(now)
                || window.active > u64::from(u32::MAX)
                || self.verified_since.is_none()
                || !self.reached.is_some_and(|reached| reached >= (window.day + 1) * 86_400)
                || (window.complete && !self.covers((window.day - 29) * 86_400, (window.day + 1) * 86_400)))
        {
            return false;
        }
        if let Some(window) = &self.apps_24h
            && (!window.is_verified()
                || window.counted > self.meta.fetched_at.div_euclid(1_000)
                || !self.covers_through(window.to - 86_400, window.to)
                || (self.rolling_to == Some(window.to) && self.rolling_24h != Some(window.active)))
        {
            return false;
        }
        true
    }

    /// Right-closed coverage for `(from,to]` activity windows.
    pub fn covers_through(&self, from: i64, to: i64) -> bool {
        self.covers(from, to) && !self.gaps.iter().any(|&(a, b)| a <= to && b > from)
    }

    /// Whether the proof includes this complete interval. Call
    /// `is_verified` first when consuming untrusted JSON.
    pub fn covers(&self, from: i64, to: i64) -> bool {
        from <= to
            && self.verified_since.is_some_and(|since| since <= from)
            && self.reached.is_some_and(|reached| reached >= to)
            && !self.gaps.iter().any(|&(a, b)| a < to && b > from)
    }
}

/// Accounts active in the 24 hours to `to` (on the hour), `(to − 24 h, to]`,
/// if all of them were read. `hours` holds their sets (see `bucket`); an
/// hour with no set had no activity.
pub fn rolling(follow: &Follow, hours: &HashMap<i64, Fids>, to: i64) -> Option<u64> {
    let last = hour(to) - 1;
    follow
        .covered_through(to - 86_400, to)
        .then(|| Fids::union_len(((last - 23)..=last).filter_map(|h| hours.get(&h))) as u64)
}

/// The end of the 24-hour windows when events have been read to `reached`:
/// the last hour read whole.
pub fn rolling_to(reached: i64) -> i64 {
    hour(reached) * 3_600
}

/// Hours counted one by one in the payload: the two 24-hour windows.
pub const HOURLY: i64 = 48;

/// Accounts active in each of the `HOURLY` hours to `to` (on the hour),
/// oldest first: `None` for an hour not read whole, and nothing at all
/// before one was. `hours` holds their sets (see `bucket`); an hour read
/// whole with no set had no activity.
pub fn hourly(follow: &Follow, hours: &HashMap<i64, Fids>, to: i64) -> Vec<Option<u64>> {
    let last = hour(to) - 1;
    let counts: Vec<Option<u64>> = ((last - HOURLY + 1)..=last)
        .map(|h| {
            follow.covered_through(h * 3_600, (h + 1) * 3_600).then(|| hours.get(&h).map_or(0, |set| set.len() as u64))
        })
        .collect();
    if counts.iter().any(Option::is_some) { counts } else { Vec::new() }
}

/// The average active accounts a day over the 7 UTC days to the end of the
/// newest day read whole, and over the 7 before them: SnapMeter's 7-day
/// trend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Weeks {
    /// The newer week's last day.
    pub to: i64,
    pub average: u64,
    pub before: u64,
}

/// The two weeks' averages, rounded, if all 14 days were read whole.
pub fn weeks(days: &[DayCount]) -> Option<Weeks> {
    let to = days.iter().rev().find(|d| d.complete)?.day;
    let average = |last: i64| {
        let week = ((last - 6)..=last)
            .map(|on| days.iter().find(|d| d.day == on && d.complete).map(|d| d.active))
            .collect::<Option<Vec<u64>>>()?;
        Some((week.iter().sum::<u64>() + 3) / 7)
    };
    Some(Weeks { to, average: average(to)?, before: average(to - 7)? })
}

/// The median count of the days read whole, once a week of them has been:
/// a typical day, for today's count so far to be measured against.
pub fn median_day(days: &[DayCount]) -> Option<u64> {
    let mut counts: Vec<u64> = days.iter().filter(|d| d.complete).map(|d| d.active).collect();
    if counts.len() < 7 {
        return None;
    }
    counts.sort_unstable();
    let mid = counts.len() / 2;
    Some(if counts.len().is_multiple_of(2) { (counts[mid - 1] + counts[mid]).div_ceil(2) } else { counts[mid] })
}

/// The payload for `follow`'s counts, with the sets of the 48 hours before
/// the hour in progress.
pub fn summary(follow: &Follow, hours: &HashMap<i64, Fids>, meta: crate::api::Meta) -> ActivityPayload {
    let reached = follow.reached();
    let to = reached.map(rolling_to);
    ActivityPayload {
        verification_version: follow.verification_version,
        verified_since: follow.since,
        source: ACTIVITY_SOURCE.into(),
        shards: SHARDS
            .into_iter()
            .map(|shard| {
                let block = follow.blocks.iter().find(|b| b.shard == shard);
                ShardVerification {
                    shard,
                    since: follow.starts.iter().find(|s| s.shard == shard).map(|s| s.at),
                    reached: follow.reached.iter().find(|s| s.shard == shard).map(|s| s.at),
                    cursor: follow.cursors.iter().find(|s| s.shard == shard).map(|s| s.event_id),
                    block: block.map(|b| b.block),
                    block_at: block.map(|b| b.at),
                    total_events: block.map(|b| b.total),
                    read_events: block.map(|b| b.seen),
                }
            })
            .collect(),
        reached,
        rolling_to: to,
        rolling_24h: to.and_then(|t| rolling(follow, hours, t)),
        previous_24h: to.and_then(|t| rolling(follow, hours, t - 86_400)),
        active_30d: follow.active_30d,
        apps_24h: follow.apps_24h.clone(),
        app_accounts: follow.app_accounts.clone(),
        days: follow.days.iter().rev().take(KEEP_DAYS as usize).rev().cloned().collect(),
        hourly: to.map(|t| hourly(follow, hours, t)).unwrap_or_default(),
        since: follow.since,
        gaps: follow.gaps.clone(),
        meta,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page with a block confirmed at Farcaster time 183,000,000
    /// (2026-10-20T01:20:00Z), its messages, and one whose block is on an
    /// earlier page.
    const PAGE: &str = r#"{"events":[
      {"type":"HUB_EVENT_TYPE_BLOCK_CONFIRMED","id":100,"blockNumber":7,"shardIndex":1,
       "blockConfirmedBody":{"blockNumber":7,"shardIndex":1,"timestamp":183000000,"totalEvents":4}},
      {"type":"HUB_EVENT_TYPE_MERGE_MESSAGE","id":101,"blockNumber":7,"shardIndex":1,"mergeMessageBody":{"message":{
        "data":{"type":"MESSAGE_TYPE_CAST_ADD","fid":42,"timestamp":182999990,"network":"FARCASTER_NETWORK_MAINNET","castAddBody":{"text":"gm"}},
        "hash":"0xab","signature":"c2ln","signatureScheme":"SIGNATURE_SCHEME_ED25519",
        "signer":"0x7BEB4B4B562532FE55E91D52EFB3C4AD14182DFB1D550B43C577DF4A19305B64"}}},
      {"type":"HUB_EVENT_TYPE_MERGE_MESSAGE","id":102,"blockNumber":7,"mergeMessageBody":{"message":{
        "data":{"type":"MESSAGE_TYPE_REACTION_ADD","fid":7,"timestamp":182999999},"signer":"0x01"}}},
      {"type":"HUB_EVENT_TYPE_MERGE_MESSAGE","id":103,"blockNumber":6,"mergeMessageBody":{"message":{
        "data":{"type":"MESSAGE_TYPE_LINK_ADD","fid":42,"timestamp":182999000},"signer":"0x02"}}},
      {"type":"HUB_EVENT_TYPE_MERGE_MESSAGE","id":104,"blockNumber":7,"mergeMessageBody":{"message":{
        "data":{"type":"MESSAGE_TYPE_NONE","fid":9,"timestamp":182999999},"signer":"0x03"}}},
      {"type":"HUB_EVENT_TYPE_MERGE_MESSAGE","id":105,"blockNumber":7,"mergeMessageBody":{"message":{
        "data":{"type":"MESSAGE_TYPE_CAST_ADD","fid":0,"timestamp":182999999},"signer":"0x04"}}},
      {"type":"HUB_EVENT_TYPE_PRUNE_MESSAGE","id":106,"blockNumber":7,"pruneMessageBody":{"message":{
        "data":{"type":"MESSAGE_TYPE_CAST_ADD","fid":5,"timestamp":182999999},"signer":"0x05"}}},
      {"type":"HUB_EVENT_TYPE_MERGE_ON_CHAIN_EVENT","id":107,"blockNumber":7}
    ]}"#;

    #[test]
    fn counts_only_qualifying_merges_at_their_block_time() {
        let page = activity(PAGE.as_bytes()).unwrap();
        assert_eq!(page.count, 8);
        assert_eq!(page.last_id, Some(107));
        let block = 183_000_000 + FARCASTER_EPOCH;
        assert_eq!((page.reached, page.started, page.first_block), (Some(block), Some(block), Some(7)));
        assert_eq!(
            page.actions,
            [
                Action {
                    fid: 42,
                    signer: "0x7beb4b4b562532fe55e91d52efb3c4ad14182dfb1d550b43c577df4a19305b64".into(),
                    at: block,
                    family: Family::Cast,
                },
                Action { fid: 7, signer: "0x01".into(), at: block, family: Family::Reaction },
                // Its block was confirmed on an earlier page: its signed time.
                Action { fid: 42, signer: "0x02".into(), at: 182_999_000 + FARCASTER_EPOCH, family: Family::Link },
            ]
        );
        assert_eq!(activity(b"{}"), None);
        assert_eq!(activity(br#"{"events":[]}"#), Some(ActivityPage::default()));
    }

    #[test]
    fn keeps_emoji_reactions_and_the_casts_that_take_them_back() {
        let parent = "0x1111111111111111111111111111111111111111";
        let message = |id: u32, hash: &str, body: &str| {
            format!(
                r#"{{"type":"HUB_EVENT_TYPE_MERGE_MESSAGE","id":{id},"blockNumber":9,"mergeMessageBody":{{"message":{{
                  "hash":"{hash}","signer":"0x0a","data":{{"fid":7,"timestamp":183000090,{body}}}}}}}}}"#
            )
        };
        let reply = |text: &str, embeds: &str| {
            format!(
                r#""type":"MESSAGE_TYPE_CAST_ADD","castAddBody":{{"text":"{text}","embeds":[{embeds}],"parentCastId":{{"fid":3,"hash":"0x1111111111111111111111111111111111111111"}}}}"#
            )
        };
        let gone = |kind: &str, body: &str, hash: &str, text: &str| {
            format!(
                r#"{{"type":"HUB_EVENT_TYPE_{kind}_MESSAGE","id":209,"{body}":{{"message":{{"hash":"{hash}",
                  "data":{{"type":"MESSAGE_TYPE_CAST_ADD","fid":8,"timestamp":183000000,
                  "castAddBody":{{"text":"{text}","parentCastId":{{"fid":3,"hash":"0x1111111111111111111111111111111111111111"}}}}}}}}}}}}"#
            )
        };
        let events = [
            r#"{"type":"HUB_EVENT_TYPE_BLOCK_CONFIRMED","id":200,"blockNumber":9,"blockConfirmedBody":{"blockNumber":9,"timestamp":183000100}}"#.to_owned(),
            // A reaction, its hashes in upper case.
            message(201, "0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", &reply(":pepe_no:", "")),
            // Spaces, a line break and capitals around it, escaped: still one.
            message(202, "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", &reply("  :PEPE_HEART_HUG:\\n", "")),
            // Not reactions: words with it, an image with it, a top-level cast.
            message(203, "0xcccccccccccccccccccccccccccccccccccccccc", &reply("gm :pepe_no:", "")),
            message(204, "0xdddddddddddddddddddddddddddddddddddddddd", &reply(":pepe_no:", r#"{"url":"https://example.com/a.png"}"#)),
            message(205, "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee", r#""type":"MESSAGE_TYPE_CAST_ADD","castAddBody":{"text":":pepe_no:","parentCastId":null}"#),
            // A cast removed: its target in base64.
            message(206, "0xffffffffffffffffffffffffffffffffffffffff", r#""type":"MESSAGE_TYPE_CAST_REMOVE","castRemoveBody":{"targetHash":"MwDCsNIIAE0e6G2gW0WL+TQUCqQ="}"#),
            // A reaction revoked with its key; a cast that isn't one pruned.
            gone("REVOKE", "revokeMessageBody", "0x2222222222222222222222222222222222222222", ":pepe_pls:"),
            gone("PRUNE", "pruneMessageBody", "0x3333333333333333333333333333333333333333", "an old cast"),
        ];
        let page = activity(format!(r#"{{"events":[{}]}}"#, events.join(",")).as_bytes()).unwrap();
        let at = 183_000_100 + FARCASTER_EPOCH;
        let reacted = |hash: &str, emoji: &str| {
            (parent.to_owned(), ReactionReply { hash: hash.into(), emoji: emoji.into(), fid: 7, ts: at })
        };
        assert_eq!(
            page.reactions,
            [
                reacted("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "pepe_no"),
                reacted("0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "pepe_heart_hug"),
            ]
        );
        assert_eq!(
            page.removed,
            ["0x3300c2b0d208004d1ee86da05b458bf934140aa4", "0x2222222222222222222222222222222222222222"]
        );
        // Every merge still counts as activity, a reaction's too.
        assert_eq!(page.actions.len(), 6);
    }

    #[test]
    fn the_allowlist_is_closed() {
        assert_eq!(Family::of("MESSAGE_TYPE_CAST_REMOVE"), Some(Family::Cast));
        assert_eq!(Family::of("MESSAGE_TYPE_LINK_COMPACT_STATE"), Some(Family::Link));
        assert_eq!(Family::of("MESSAGE_TYPE_KEY_ADD"), Some(Family::Key));
        assert_eq!(Family::of("MESSAGE_TYPE_CHANNEL_MODERATE"), Some(Family::Channel));
        for other in ["MESSAGE_TYPE_NONE", "MESSAGE_TYPE_SOMETHING_NEW", "CAST_ADD", "", "MESSAGE_TYPE_"] {
            assert_eq!(Family::of(other), None, "{other}");
        }
    }

    #[test]
    fn fid_sets_round_trip_compactly_and_count_exactly() {
        let a = Fids::of([42, 7, 3_400_000, 7, 1, u64::from(u32::MAX)]);
        assert_eq!(a.len(), 5);
        let text = a.encode();
        assert!(text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'), "{text}");
        assert_eq!(Fids::decode(&text), Some(a.clone()));
        assert_eq!(Fids::decode(""), Some(Fids::default()));
        assert_eq!(Fids::default().encode(), "");
        // Not a stored set: other characters, a varint cut short, a repeat.
        assert_eq!(Fids::decode("a b"), None);
        assert_eq!(Fids::decode(&Fids::of([300]).encode()[..1]), None);
        assert_eq!(Fids::decode(&Fids::of([300]).encode()[..2]), None);
        assert_eq!(Fids::decode("AQA"), None);
        // A day of 6,000 accounts spread over 3.4 million fids: about 2 bytes each.
        let day = Fids::of((0..6_000u64).map(|i| 1 + i * 566));
        assert!(day.encode().len() < 6_000 * 3, "{}", day.encode().len());
        assert_eq!(Fids::decode(&day.encode()), Some(day.clone()));
        // Union.
        let mut b = Fids::of([7, 8, 42]);
        b.extend(&a);
        assert_eq!(b, Fids::of([1, 7, 8, 42, 3_400_000, u64::from(u32::MAX)]));
        assert_eq!(Fids::union_len([&a, &Fids::of([7, 8]), &Fids::default()]), 6);
        assert_eq!(Fids::union_len(std::iter::empty()), 0);
        assert_eq!(Fids::union_len([&day, &day]), 6_000);
    }

    #[test]
    fn a_set_says_whether_it_holds_an_fid() {
        let set = Fids::of([1, 7, 42, 3_400_000, u64::from(u32::MAX)]);
        for fid in [1, 7, 42, 3_400_000, u64::from(u32::MAX)] {
            assert!(set.contains(fid), "{fid}");
        }
        for fid in [0, 2, 41, 43, 3_399_999, u64::from(u32::MAX) + 1, u64::MAX] {
            assert!(!set.contains(fid), "{fid}");
        }
        assert!(!Fids::default().contains(1));
    }

    #[test]
    fn a_week_says_who_of_those_asked_was_active_in_the_order_asked() {
        assert_eq!(week_set(20_385), "v2w20385");
        let week = Fids::of([3, 9, 12]);
        assert_eq!(
            ActiveWeek::among(20_385, &week, &[12, 4, 3, 3]),
            ActiveWeek { day: 20_385, active: vec![12, 3, 3] }
        );
        assert_eq!(ActiveWeek::among(20_385, &week, &[]).active, Vec::<u64>::new());
        assert_eq!(ActiveWeek::among(20_385, &Fids::default(), &[3]).active, Vec::<u64>::new());
    }

    /// A week's union of `n` accounts spread over today's 3.4 million fids,
    /// as stored: decoded, then asked about a page of 100 people.
    fn decode_and_ask(n: u64) -> std::time::Duration {
        let week = Fids::of((0..n).map(|i| 1 + i * (3_400_000 / n)));
        let stored = week.encode();
        let page: Vec<u64> = (0..100u64).map(|i| 1 + i * 33_997).collect();
        let started = std::time::Instant::now();
        let decoded = Fids::decode(std::hint::black_box(&stored)).unwrap();
        let active = ActiveWeek::among(1, &decoded, &page);
        std::hint::black_box(active);
        started.elapsed()
    }

    #[test]
    fn a_week_of_actives_is_read_and_asked_about_within_a_millisecond() {
        // The fastest of 5, so a busy machine's pauses don't count.
        let fastest = |n| (0..5).map(|_| decode_and_ask(n)).min().unwrap();
        let (today, later) = (fastest(6_200), fastest(50_000));
        println!("decoding a week of 6,200 accounts and 100 lookups: {today:?}; of 50,000: {later:?}");
        // In a release build (the Worker's), well under a millisecond for
        // 6,200 (and its 7-day union, a few times that); the tests'
        // unoptimized build is several times slower.
        let bound = if cfg!(debug_assertions) { 50 } else { 5 };
        assert!(today.as_millis() < bound, "6,200: {today:?}");
        assert!(later.as_millis() < bound * 4, "50,000: {later:?}");
    }

    fn follow(since: i64, reached: i64) -> Follow {
        Follow {
            verification_version: VERIFICATION_VERSION,
            starts: vec![Reached { shard: 1, at: since }, Reached { shard: 2, at: since }],
            reached: vec![Reached { shard: 1, at: reached }, Reached { shard: 2, at: reached + 30 }],
            since: Some(since),
            ..Follow::default()
        }
    }

    #[test]
    fn coverage_needs_the_start_the_end_and_no_gap() {
        let mut f = follow(1_000, 10_000);
        assert_eq!(f.reached(), Some(10_000));
        assert!(f.covered(1_000, 10_000));
        assert!(!f.covered(999, 5_000));
        assert!(!f.covered(5_000, 10_001));
        f.gaps.push((4_000, 4_500));
        assert!(!f.covered(1_000, 10_000));
        assert!(f.covered(4_500, 10_000));
        assert!(f.covered(1_000, 4_000));
        assert!(!Follow::default().covered(0, 1));
    }

    #[test]
    fn days_are_counted_in_order_and_settled() {
        let day0 = 20_000;
        let mut f = follow(day0 * 86_400 + 5, (day0 + 2) * 86_400 + 100);
        f.day_mut(day0 + 1).active = 7;
        f.day_mut(day0).active = 3;
        f.day_mut(day0 + 2).active = 1;
        assert_eq!(f.days.iter().map(|d| d.day).collect::<Vec<_>>(), [day0, day0 + 1, day0 + 2]);
        f.settle();
        // Begun 5 seconds into its first day; the third is in progress.
        assert_eq!(f.days.iter().map(|d| d.complete).collect::<Vec<_>>(), [false, true, false]);
        for d in 0..70 {
            f.day_mut(day0 + 3 + d);
        }
        f.settle();
        assert_eq!(f.days.len(), HISTORY_DAYS);
        assert_eq!(f.days.last().map(|d| d.day), Some(day0 + 72));
    }

    #[test]
    fn a_batch_sorts_actions_into_hours_and_days() {
        let at = 20_000 * 86_400 + 3_599;
        let actions = [
            Action { fid: 2, signer: "0x01".into(), at, family: Family::Cast },
            Action { fid: 1, signer: "0x02".into(), at, family: Family::Reaction },
            Action { fid: 2, signer: "0x01".into(), at: at + 1, family: Family::Reaction },
        ];
        let batch = Batch::of(&actions);
        // An hour ends on its last second: 00:59:59 and 01:00:00 are in hour 0.
        assert_eq!(batch.hours[&hour(at)], Fids::of([1, 2]));
        assert_eq!(batch.hours.len(), 1);
        assert_eq!((bucket(at + 1), bucket(at + 2)), (hour(at), hour(at) + 1));
        assert_eq!(batch.days[&20_000], Fids::of([1, 2]));
        assert_eq!(batch.actions[&20_000][&Family::Reaction], 2);
        assert_eq!(batch.actions[&20_000][&Family::Cast], 1);
    }

    #[test]
    fn rolling_windows_count_each_account_once_and_only_when_read() {
        // Read to 10 minutes into hour 500,000: the windows end as it began.
        let now_hour = 500_000;
        let to = now_hour * 3_600;
        let mut hours = HashMap::new();
        // The hour in progress isn't in a window yet.
        hours.insert(now_hour, Fids::of([9]));
        hours.insert(now_hour - 1, Fids::of([1, 2]));
        hours.insert(now_hour - 24, Fids::of([2, 3]));
        hours.insert(now_hour - 25, Fids::of([4]));
        hours.insert(now_hour - 48, Fids::of([4, 5]));
        hours.insert(now_hour - 49, Fids::of([6]));
        let reached = to + 600;
        assert_eq!(rolling_to(reached), to);
        let f = follow((now_hour - 48) * 3_600, reached);
        assert_eq!(rolling(&f, &hours, to), Some(3));
        assert_eq!(rolling(&f, &hours, to - 86_400), Some(2));
        // Counting began after the earlier window started.
        let late = follow((now_hour - 30) * 3_600, reached);
        assert_eq!(rolling(&late, &hours, to - 86_400), None);
        // A window not read to its end.
        assert_eq!(rolling(&f, &hours, to + 3_600), None);
        // Each of the 48 hours, oldest first; the hour in progress isn't one.
        let each = hourly(&f, &hours, to);
        assert_eq!(each.len(), 48);
        assert_eq!((each[0], each[23], each[24], each[46], each[47]), (Some(2), Some(1), Some(2), Some(0), Some(2)));
        // Hours before counting began weren't read whole; with none read
        // whole, there are none.
        let each = hourly(&late, &hours, to);
        assert_eq!((each.iter().position(Option::is_some), each[47]), (Some(18), Some(2)));
        assert!(hourly(&follow(to, reached), &hours, to).is_empty());
        let meta = crate::api::Meta::default();
        let payload = summary(&f, &hours, meta);
        assert_eq!((payload.rolling_24h, payload.previous_24h, payload.reached), (Some(3), Some(2), Some(reached)));
        assert_eq!(payload.rolling_to, Some(to));
        assert_eq!(payload.hourly, hourly(&f, &hours, to));
        let json = serde_json::to_string(&payload).unwrap();
        assert_eq!(serde_json::from_str::<ActivityPayload>(&json).unwrap(), payload);
    }

    #[test]
    fn keys_added_onchain_carry_their_app() {
        // A Farcaster key (its real KeyRegistry metadata), a key with no
        // signed key request, and a removal, which doesn't count.
        let metadata = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAACAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAjwAAAAAAAAAAAAAAAAALveQ3XmTo1/YR8BT7drpQNBVWWAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAZkXc9QAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABBkm8PZXmsC5a/2h4CHNOJ+1vP3cEQcvCkzn7pGxnuiV8fz0SfwJzkTYNZO3syUdrWKe2rOJmaBvrTewq+CW2LdRwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let event = |key: &str, kind: &str, extra: &str| {
            format!(
                r#"{{"type":"HUB_EVENT_TYPE_MERGE_ON_CHAIN_EVENT","id":1,"mergeOnChainEventBody":{{"onChainEvent":{{"type":"EVENT_TYPE_SIGNER","fid":1096125,"signerEventBody":{{"key":"{key}","keyType":1,"eventType":"SIGNER_EVENT_TYPE_{kind}"{extra}}}}}}}}}"#
            )
        };
        let key = "0xaacf883b539a3e30e0b918c626f9b8b89fbd09feecfba3f8bf27c3ffd178a2ee";
        let page = format!(
            r#"{{"events":[{},{},{}]}}"#,
            event(key, "ADD", &format!(r#","metadata":"{metadata}","metadataType":1"#)),
            event("0x0102030405", "ADD", ""),
            event(key, "REMOVE", ""),
        );
        let page = activity(page.as_bytes()).unwrap();
        let fid = 1_096_125u64;
        assert_eq!(page.keys, [(fid << 32 | 0xaacf883b, 9152), (fid << 32 | 0x01020304, 0)]);
        assert!(page.actions.is_empty());
    }

    #[test]
    fn key_sets_round_trip_and_split_between_accounts() {
        assert_eq!(key_pair(42, "0xaacf883b539a"), Some(42 << 32 | 0xaacf883b));
        assert_eq!(key_pair(42, "0xAACF883B539A"), key_pair(42, "0xaacf883b539a"));
        assert_eq!(key_pair(42, "0xnothex0"), None);
        assert_eq!(key_pair(42, "0x01"), Some(42 << 32 | 1));
        assert_eq!(key_pair(42, "0x"), None);
        assert_eq!(key_pair(1 << 31, "0x01"), None);
        let biggest = ((1u64 << 31) - 1) << 32 | u64::from(u32::MAX);
        assert_eq!(key_pair((1 << 31) - 1, "0xffffffff00"), Some(biggest));
        assert_eq!(pair_fid(biggest), (1 << 31) - 1);
        let keys = Keys::of([biggest, 7 << 32 | 2, 7 << 32 | 1, 9 << 32, 7 << 32 | 2]);
        assert_eq!(keys.len(), 4);
        assert_eq!(Keys::decode(&keys.encode()), Some(keys.clone()));
        assert_eq!(Keys::decode(""), Some(Keys::default()));
        // A varint longer than a u64 holds.
        assert_eq!(Keys::decode("_____________w"), None);
        // An fid set can't hold a key.
        assert_eq!(Fids::decode(&keys.encode()), None);
        let mut more = Keys::of([9 << 32, 10 << 32]);
        more.extend(&keys);
        assert_eq!(more.as_slice(), [7 << 32 | 1, 7 << 32 | 2, 9 << 32, 10 << 32, biggest]);
        // Runs of about 2, never splitting an account's keys.
        let chunks: Vec<Vec<u64>> = more.chunks(1).into_iter().map(<[u64]>::to_vec).collect();
        assert_eq!(chunks, [vec![7 << 32 | 1, 7 << 32 | 2], vec![9 << 32], vec![10 << 32], vec![biggest]]);
        assert_eq!(more.chunks(3).len(), 2);
        assert!(Keys::default().chunks(3).is_empty());
    }

    #[test]
    fn a_batch_keeps_the_keys_that_signed_by_hour() {
        let at = 20_000 * 86_400 + 3_600;
        let actions = [
            Action { fid: 2, signer: "0x0a".into(), at, family: Family::Cast },
            Action { fid: 2, signer: "0x0a".into(), at: at + 1, family: Family::Reaction },
            Action { fid: 3, signer: "0x0b".into(), at: at + 1, family: Family::Reaction },
        ];
        let batch = Batch::of(&actions);
        assert_eq!(batch.keys[&(bucket(at))], Keys::of([2 << 32 | 10]));
        assert_eq!(batch.keys[&(bucket(at) + 1)], Keys::of([2 << 32 | 10, 3 << 32 | 11]));
    }

    #[test]
    fn accounts_by_app_add_up_and_say_how_much_is_known() {
        let counts = [(Some(9152), 3), (Some(0), 1), (None, 2), (Some(309_857), 5), (Some(9152), 4), (None, 1)];
        let window = AppsWindow::of(7_200, 7_300, 50, counts);
        assert_eq!(window.apps, [AppActive { app: 9152, active: 7 }, AppActive { app: 309_857, active: 5 }]);
        assert_eq!((window.none, window.unknown), (1, 3));
        assert_eq!((window.through(9152), window.through(5)), (7, 0));
        assert_eq!(window.known_percent(), 94);
        assert_eq!(AppsWindow::of(0, 0, 0, []).known_percent(), 100);
        assert_eq!(AppsWindow { unknown: 1, ..AppsWindow::of(0, 0, 1_000, []) }.known_percent(), 99);
        let only = window.only(|app| app == 309_857);
        assert_eq!((only.apps.len(), only.active, only.unknown), (1, 50, 3));
        let json = serde_json::to_string(&window).unwrap();
        assert_eq!(serde_json::from_str::<AppsWindow>(&json).unwrap(), window);
    }

    #[test]
    fn accounts_are_counted_by_app_once_an_hour_once_a_window_of_keys_is_kept() {
        let to = 500_000 * 3_600;
        let mut f = follow(to - 3 * 86_400, to + 600);
        // Keys kept from half a day before the window: not a whole window yet.
        f.keys_since = Some(to - 86_400 / 2);
        assert!(!f.apps_due(to, to + 600));
        f.keys_since = Some(to - 86_400);
        assert!(f.apps_due(to, to + 600));
        // Counted for this window: not again until the next, unless more
        // than 1% of accounts' apps were unknown, then every 10 minutes.
        f.apps_24h = Some(AppsWindow::of(to, to + 600, 1_000, [(Some(9152), 995), (None, 5)]));
        assert!(!f.apps_due(to, to + 3_000));
        // The next window, once read to its end.
        assert!(!f.apps_due(to + 3_600, to + 3_660));
        f.reached.iter_mut().for_each(|r| r.at = to + 3_700);
        assert!(f.apps_due(to + 3_600, to + 3_700));
        f.apps_24h = Some(AppsWindow::of(to, to + 600, 1_000, [(Some(9152), 900), (None, 100)]));
        assert!(!f.apps_due(to, to + 1_100));
        assert!(f.apps_due(to, to + 1_200));
    }

    #[test]
    fn weeks_and_a_typical_day_are_of_days_read_whole() {
        let day0 = 20_000;
        // 20 days, the first begun mid-day and the last in progress: 100 a
        // day, then 200 a day for the 7 before the one in progress.
        let days: Vec<DayCount> = (0..20)
            .map(|i| DayCount {
                day: day0 + i,
                active: if i == 0 {
                    5
                } else if i >= 12 {
                    200
                } else {
                    100 + u64::from(i == 5) * 3
                },
                complete: i > 0 && i < 19,
                ..DayCount::default()
            })
            .collect();
        // The newer week ends on the last day read whole, day 18: days 12 to
        // 18; the week before, days 5 to 11 (703 / 7 rounds to 100).
        assert_eq!(weeks(&days), Some(Weeks { to: day0 + 18, average: 200, before: 100 }));
        // Not 14 days read whole: no trend.
        assert_eq!(weeks(&days[6..]), None);
        let mut gap = days.clone();
        gap[8].complete = false;
        assert_eq!(weeks(&gap), None);
        // The median of the 18 days read whole: 11 of 100 (one 103), 7 of 200.
        assert_eq!(median_day(&days), Some(100));
        assert_eq!(median_day(&days[10..]), Some(200));
        // An even count takes the mean of the middle two, rounded up.
        assert_eq!(median_day(&days[8..16]), Some(150));
        assert_eq!(median_day(&days[..7]), None);
    }

    #[test]
    fn days_are_utc() {
        assert_eq!(day(0), 0);
        assert_eq!(day(86_399), 0);
        assert_eq!(day(86_400), 1);
        assert_eq!(day(-1), -1);
        assert_eq!(day_name(0), "1970-01-01");
        assert_eq!(day_name(day(1_609_459_200)), "2021-01-01");
        // Farcaster time 183,000,000 is 2026-10-20T01:20:00Z; that day
        // starts at 1,792,454,400.
        assert_eq!(day_name(day(183_000_000 + FARCASTER_EPOCH)), "2026-10-20");
        assert_eq!(day_name(day(1_792_454_400)), "2026-10-20");
        assert_eq!(day_name(day(1_792_454_399)), "2026-10-19");
        assert_eq!(day_name(day(951_782_400)), "2000-02-29");
    }
    fn confirmation(shard: u32, height: u64, at: i64, total: u32) -> serde_json::Value {
        serde_json::json!({"type":"HUB_EVENT_TYPE_BLOCK_CONFIRMED","id":height << 14,"blockNumber":height,"shardIndex":shard,
            "blockConfirmedBody":{"blockNumber":height,"shardIndex":shard,"timestamp":at-FARCASTER_EPOCH,"totalEvents":total}})
    }

    fn canonical_merge(shard: u32, height: u64, seq: u64, signed_at: i64) -> serde_json::Value {
        serde_json::json!({"type":"HUB_EVENT_TYPE_MERGE_MESSAGE","id":(height << 14)|seq,"blockNumber":height,"shardIndex":shard,
            "mergeMessageBody":{"message":{"signer":format!("0x{:064x}", 1),"data":{"type":"MESSAGE_TYPE_CAST_ADD","fid":7,"timestamp":signed_at-FARCASTER_EPOCH}}}})
    }

    fn canonical_page(events: Vec<serde_json::Value>) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"events":events})).unwrap()
    }

    #[test]
    fn canonical_block_time_survives_page_splits_and_equal_timestamp_blocks() {
        let at = 1_800_000_000;
        let first = verified_activity(
            &canonical_page(vec![confirmation(1, 10, at, 3), canonical_merge(1, 10, 1, at - 90_000)]),
            1,
            (10 << 14) - 1,
            None,
            at + 10,
        )
        .unwrap();
        assert_eq!(first.page.reached, None, "a partial block is not complete coverage");
        assert_eq!(first.page.actions[0].at, at);
        let second = verified_activity(
            &canonical_page(vec![canonical_merge(1, 10, 2, at + 90_000)]),
            1,
            (10 << 14) | 1,
            first.block.as_ref(),
            at + 10,
        )
        .unwrap();
        assert_eq!(second.page.actions[0].at, at, "the user's signed time cannot move activity");
        assert_eq!(second.page.reached, Some(at - 1));
        let equal = verified_activity(
            &canonical_page(vec![confirmation(1, 11, at, 1)]),
            1,
            (10 << 14) | 2,
            second.block.as_ref(),
            at + 10,
        )
        .unwrap();
        assert_eq!(equal.page.reached, Some(at - 1), "later blocks can share a second");
        let later = verified_activity(
            &canonical_page(vec![confirmation(1, 12, at + 1, 1)]),
            1,
            11 << 14,
            equal.block.as_ref(),
            at + 10,
        )
        .unwrap();
        assert_eq!(later.page.reached, Some(at));
    }

    #[test]
    fn canonical_pages_reject_wrong_shards_duplicates_missing_sequences_and_future_blocks() {
        let at = 1_800_000_000;
        for events in [
            vec![confirmation(2, 10, at, 1)],
            vec![confirmation(1, 10, at + 1, 1)],
            vec![confirmation(1, 10, at, 3), canonical_merge(1, 10, 2, at)],
            vec![confirmation(1, 10, at, 3), canonical_merge(1, 10, 1, at), canonical_merge(1, 10, 1, at)],
        ] {
            assert!(verified_activity(&canonical_page(events), 1, (10 << 14) - 1, None, at).is_none());
        }
        let mut wrong_block = confirmation(1, 10, at, 1);
        wrong_block["blockConfirmedBody"]["blockNumber"] = 11.into();
        assert!(verified_activity(&canonical_page(vec![wrong_block]), 1, (10 << 14) - 1, None, at).is_none());
        assert!(
            verified_activity(&canonical_page(vec![canonical_merge(1, 10, 1, at)]), 1, 10 << 14, None, at).is_none(),
            "no canonical block time means no verified count"
        );
    }

    #[test]
    fn retention_jumps_leave_a_gap_including_equal_timestamp_edges() {
        let at = 1_800_000_000;
        let carried = BlockProgress { shard: 1, block: 10, at, total: 2, seen: 1 };
        let next = verified_activity(
            &canonical_page(vec![confirmation(1, 12, at + 60, 1)]),
            1,
            10 << 14,
            Some(&carried),
            at + 60,
        )
        .unwrap();
        assert_eq!(next.gaps, vec![(at, at + 61)]);
    }

    #[test]
    fn coverage_requires_both_shards_and_the_later_shards_start() {
        let mut state = follow(1_000, 10_000);
        state.starts[1].at = 2_000;
        state.since = Some(2_000);
        assert!(!state.covered(1_000, 3_000));
        assert!(state.covered(2_000, 3_000));
        state.reached.pop();
        assert_eq!(state.reached(), None);
        assert!(!state.covered(2_000, 3_000));
    }

    #[test]
    fn exact_signers_preserve_colliding_prefixes_and_all_u32_fids() {
        let a = signer_identity(u64::from(u32::MAX), &format!("0x12345678{:056x}", 1)).unwrap();
        let b = signer_identity(u64::from(u32::MAX), &format!("0x12345678{:056x}", 2)).unwrap();
        let keys = ExactKeys::of([a.clone(), b.clone(), a]);
        assert_eq!(keys.as_slice().len(), 2);
        assert_eq!(ExactKeys::decode(&keys.encode()), Some(keys.clone()));
        assert_eq!(identity_fid(&b), Some(u64::from(u32::MAX)));
        assert_eq!(keys.chunks(1).len(), 1, "an account is never split between count statements");
        assert!(signer_identity(42, "0x01").is_none());
        assert!(ExactKeys::decode(r#"["0000000042:0x01"]"#).is_none());
    }
    #[test]
    fn cached_app_windows_need_the_full_key_verification_marker() {
        let to = hour(1_800_000_000) * 3_600;
        let mut window = AppsWindow::of(to, to + 60, 2, [(Some(9152), 1), (None, 1)]);
        assert!(!window.is_verified());
        window.verification_version = VERIFICATION_VERSION;
        assert!(window.is_verified());
        window.apps.push(AppActive { app: 9152, active: 1 });
        assert!(!window.is_verified(), "duplicate app rows cannot inflate a count");
    }
    #[test]
    fn payload_proof_distinguishes_a_partial_epoch_from_invented_complete_counts() {
        let at = 1_800_000_000;
        let state = Follow { verification_version: VERIFICATION_VERSION, ..Follow::default() };
        let mut payload = summary(
            &state,
            &HashMap::new(),
            crate::api::Meta { fetched_at: at * 1_000, ..crate::api::Meta::default() },
        );
        assert!(payload.is_verified(at), "two explicit waiting shards form a valid incomplete epoch");
        payload.rolling_24h = Some(0);
        assert!(!payload.is_verified(at), "missing coverage cannot become a certified zero");
        payload.rolling_24h = None;
        payload.shards.pop();
        assert!(!payload.is_verified(at));
    }

    #[test]
    fn payload_proof_rejects_unsupported_sources_future_progress_and_false_complete_days() {
        let at = 1_800_000_000;
        let since = at - 3 * 86_400;
        let mut state = follow(since, at - 1);
        state.keys_since = state.since;
        state.blocks =
            SHARDS.into_iter().map(|shard| BlockProgress { shard, block: 10, at, total: 1, seen: 1 }).collect();
        state.cursors = SHARDS.into_iter().map(|shard| Cursor { shard, event_id: 10 << 14 }).collect();
        // The core follow helper's second shard is thirty seconds ahead;
        // this proof has both shards at the same canonical checkpoint.
        state.reached.iter_mut().for_each(|r| r.at = at - 1);
        let payload = summary(
            &state,
            &HashMap::new(),
            crate::api::Meta { fetched_at: at * 1_000, ..crate::api::Meta::default() },
        );
        assert!(payload.is_verified(at));
        for corrupt in 0..5 {
            let mut bad = payload.clone();
            match corrupt {
                0 => bad.source = "https://other.example/v1/events".into(),
                1 => bad.shards[1].shard = 1,
                2 => bad.shards[0].block_at = Some(at + 1),
                3 => bad.days.push(DayCount { day: day(since) - 1, active: 1, complete: true, ..DayCount::default() }),
                _ => bad.meta.fetched_at = (at + 1) * 1_000,
            }
            assert!(!bad.is_verified(at));
        }
    }
    #[test]
    fn recorded_hypersnap_blocks_verify_whole_and_split_at_every_page_boundary() {
        for (shard, body, height, at, actions) in [
            (1, include_bytes!("../tests/fixtures/activity-shard-1.json").as_slice(), 49_751_747, 1_793_097_607, 2),
            (2, include_bytes!("../tests/fixtures/activity-shard-2.json").as_slice(), 49_587_008, 1_793_097_593, 1),
        ] {
            let whole = verified_activity(body, shard, (height << 14) - 1, None, at + 60).unwrap();
            assert_eq!(whole.page.actions.len(), actions);
            assert_eq!(whole.page.reached, Some(at - 1));
            assert!(whole.block.as_ref().unwrap().complete());
            let input: serde_json::Value = serde_json::from_slice(body).unwrap();
            let events = input["events"].as_array().unwrap();
            for cut in 1..events.len() {
                let first = verified_activity(
                    &canonical_page(events[..cut].to_vec()),
                    shard,
                    (height << 14) - 1,
                    None,
                    at + 60,
                )
                .unwrap();
                assert_eq!(first.page.reached, None);
                let second = verified_activity(
                    &canonical_page(events[cut..].to_vec()),
                    shard,
                    first.page.last_id.unwrap(),
                    first.block.as_ref(),
                    at + 60,
                )
                .unwrap();
                let mut split = first.page.actions;
                split.extend(second.page.actions);
                assert_eq!(split, whole.page.actions, "shard{shard} split after event{cut}");
                assert_eq!(second.page.reached, whole.page.reached);
            }
        }
    }
    #[test]
    fn a_gap_on_the_right_edge_invalidates_hours_and_rolling_but_not_the_prior_day() {
        let to = 20_000 * 86_400;
        let mut state = follow(to - 3 * 86_400, to + 60);
        state.gaps.push((to, to + 1));
        assert!(state.covered(to - 86_400, to), "the timestamp belongs to the next calendar day");
        assert!(!state.covered_through(to - 86_400, to));
        assert_eq!(rolling(&state, &HashMap::new(), to), None);
        assert_eq!(hourly(&state, &HashMap::new(), to).last(), Some(&None));
        state.keys_since = state.since;
        assert!(!state.apps_due(to, to + 60));
    }

    #[test]
    fn missing_or_unknown_event_types_and_missing_merge_types_are_not_verified_as_zero() {
        let at = 1_800_000_000;
        for bad_kind in [None, Some("HUB_EVENT_TYPE_UNKNOWN")] {
            let mut message = canonical_merge(1, 10, 1, at);
            match bad_kind {
                None => {
                    message.as_object_mut().unwrap().remove("type");
                }
                Some(kind) => message["type"] = kind.into(),
            }
            assert!(
                verified_activity(
                    &canonical_page(vec![confirmation(1, 10, at, 2), message]),
                    1,
                    (10 << 14) - 1,
                    None,
                    at
                )
                .is_none()
            );
        }
        let mut message = canonical_merge(1, 10, 1, at);
        message["mergeMessageBody"]["message"]["data"].as_object_mut().unwrap().remove("type");
        assert!(
            verified_activity(&canonical_page(vec![confirmation(1, 10, at, 2), message]), 1, (10 << 14) - 1, None, at)
                .is_none()
        );
    }
    #[test]
    fn allocation_free_identity_validation_matches_the_canonical_formatter() {
        for fid in [1, 7, 42, 1 << 31, u64::from(u32::MAX)] {
            for key in [format!("0x{:064x}", 0), format!("0x12345678{:056x}", 1), format!("0x12345678{:056x}", 2)] {
                let identity = signer_identity(fid, &key).unwrap();
                assert_eq!(identity_fid(&identity), Some(fid));
            }
        }
        let valid = signer_identity(42, &format!("0x{:064x}", 1)).unwrap();
        for invalid in [
            valid.replacen("0000000042", "0000000000", 1),
            valid.replacen("0000000042", "4294967296", 1),
            valid.replacen("0000000042", "+000000042", 1),
            valid.replacen("0000000042", "000000004", 1),
            valid.replacen(":0x", ":0X", 1),
            format!("{}A", &valid[..76]),
            format!("{}é", &valid[..76]),
            format!("{} ", &valid[..76]),
        ] {
            assert_eq!(identity_fid(&invalid), None, "{invalid}");
        }
        assert_eq!(exact_signer(&format!("0X{:064X}", 0xABC)), Some(format!("0x{:064x}", 0xabc)));
    }

    #[test]
    fn exact_key_linear_merges_match_an_independent_set_union() {
        use std::collections::BTreeSet;
        let a = signer_identity(7, &format!("0x12345678{:056x}", 1)).unwrap();
        let b = signer_identity(7, &format!("0x12345678{:056x}", 2)).unwrap();
        let c = signer_identity(u64::from(u32::MAX), &format!("0x{:064x}", 3)).unwrap();
        for (left, right) in [
            (vec![], vec![a.clone(), b.clone()]),
            (vec![a.clone(), b.clone()], vec![]),
            (vec![a.clone()], vec![b.clone(), c.clone()]),
            (vec![a.clone(), c.clone()], vec![a.clone(), b.clone()]),
            (vec![a.clone(), b.clone(), c.clone()], vec![a.clone(), b.clone(), c.clone()]),
        ] {
            let expected: BTreeSet<_> = left.iter().chain(right.iter()).cloned().collect();
            let mut got = ExactKeys::of(left);
            got.extend(&ExactKeys::of(right));
            assert_eq!(got.as_slice(), expected.into_iter().collect::<Vec<_>>());
            let chunks = got.chunks(1);
            assert!(
                chunks.windows(2).all(|w| identity_fid(w[0].last().unwrap()) != identity_fid(w[1].first().unwrap()))
            );
        }
    }
}
