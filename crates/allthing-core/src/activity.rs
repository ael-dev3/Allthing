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
//!   `BLOCK_CONFIRMED` event, which comes first), or the message's signed
//!   time when the block's event is on an earlier page.
//! - **Who:** the message's fid, from 1 to 2³² − 1, and the key that signed
//!   it, which says which app it was sent from (`provenance`).
//!
//! The cron keeps each UTC hour's and each UTC day's active fids as a
//! compact set (`Fids`), so every count is exact: an account is counted once
//! in a window however many hours or shards it was seen in.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::latest::{Cursor, FARCASTER_EPOCH};
use crate::provenance::normalize_key;

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
    #[serde(rename = "blockConfirmedBody")]
    confirmed: Option<Confirmed>,
    #[serde(rename = "mergeMessageBody")]
    merge: Option<Merge>,
}

#[derive(Deserialize)]
struct Confirmed {
    #[serde(rename = "blockNumber")]
    block: Option<u64>,
    timestamp: Option<i64>,
}

#[derive(Deserialize)]
struct Merge {
    message: Option<Message>,
}

/// Only what is counted is read; serde skips the rest of each message
/// (its body, hash and signature) without building it.
#[derive(Deserialize)]
struct Message {
    data: Option<Data>,
    signer: Option<String>,
}

#[derive(Deserialize)]
struct Data {
    #[serde(rename = "type")]
    kind: Option<String>,
    fid: Option<u64>,
    timestamp: Option<i64>,
}

/// Farcaster time (seconds since 2021-01-01), as Unix seconds, if it is one.
fn unix(farcaster: i64) -> Option<i64> {
    u32::try_from(farcaster).ok().filter(|t| *t > 0).map(|t| i64::from(t) + FARCASTER_EPOCH)
}

/// Read a `/v1/events` page: its counted actions, where it ends, and how far
/// in time it reaches. `None` if it isn't an events page.
pub fn activity(json: &[u8]) -> Option<ActivityPage> {
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
                let Some(Message { data: Some(data), signer: Some(signer), .. }) = event.merge.and_then(|m| m.message)
                else {
                    continue;
                };
                let Some(family) = data.kind.as_deref().and_then(Family::of) else { continue };
                let Some(fid) = data.fid.filter(|fid| (1..=u64::from(u32::MAX)).contains(fid)) else { continue };
                let Some(signer) = normalize_key(&signer) else { continue };
                let block_time = event.block.and_then(|b| blocks.get(&b).copied());
                let Some(at) = block_time.or_else(|| data.timestamp.and_then(unix)) else { continue };
                out.actions.push(Action { fid, signer, at, family });
            }
            _ => {}
        }
    }
    Some(out)
}

/// The UTC day of Unix time `at`, as days since 1970-01-01.
pub fn day(at: i64) -> i64 {
    at.div_euclid(86_400)
}

/// The UTC hour of Unix time `at`, as hours since 1970-01-01T00:00Z.
pub fn hour(at: i64) -> i64 {
    at.div_euclid(3_600)
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

/// A set of fids, sorted. Stored as text: each fid's gap from the one
/// before as a LEB128 varint, in unpadded URL-safe base64; a day's 6,000
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

    /// Add `other`'s fids.
    pub fn extend(&mut self, other: &Fids) {
        let mut all = Vec::with_capacity(self.0.len() + other.0.len());
        let (mut a, mut b) = (self.0.iter().peekable(), other.0.iter().peekable());
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
        self.0 = all;
    }

    /// As stored.
    pub fn encode(&self) -> String {
        let mut bytes = Vec::with_capacity(self.0.len() * 2);
        let mut previous = 0u32;
        for &fid in &self.0 {
            let mut gap = fid - previous;
            previous = fid;
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

    /// From storage; `None` if `text` isn't a stored set.
    pub fn decode(text: &str) -> Option<Fids> {
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
        let mut fids = Vec::with_capacity(bytes.len() / 2);
        let (mut previous, mut gap, mut shift) = (0u32, 0u32, 0);
        for b in bytes {
            if shift > 28 {
                return None;
            }
            gap |= u32::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                previous = previous.checked_add(gap)?;
                fids.push(previous);
                (gap, shift) = (0, 0);
            } else {
                shift += 7;
            }
        }
        // Every fid after the first is a positive gap away, so the set is sorted.
        (shift == 0 && fids.windows(2).all(|w| w[0] < w[1])).then_some(Fids(fids))
    }

    /// How many distinct fids `sets` hold between them.
    pub fn union_len<'a>(sets: impl IntoIterator<Item = &'a Fids>) -> usize {
        let sets: Vec<&Fids> = sets.into_iter().collect();
        let Some(max) = sets.iter().filter_map(|s| s.0.last()).max() else { return 0 };
        // A bitmap: about 425 KB for today's 3.4 million fids, and linear.
        let mut seen = vec![0u64; *max as usize / 64 + 1];
        for fid in sets.iter().flat_map(|s| s.0.iter()) {
            seen[*fid as usize / 64] |= 1 << (fid % 64);
        }
        seen.iter().map(|w| w.count_ones() as usize).sum()
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

/// The name a set is stored under: `h<hour>` or `d<day>`.
pub fn hour_set(hour: i64) -> String {
    format!("h{hour}")
}

pub fn day_set(day: i64) -> String {
    format!("d{day}")
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
pub struct Follow {
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
}

/// A count over the days to the end of `day`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub day: i64,
    pub active: u64,
    pub complete: bool,
}

impl Follow {
    /// How far every shard has been read, Unix seconds: the least of them.
    pub fn reached(&self) -> Option<i64> {
        self.reached.iter().map(|r| r.at).min()
    }

    /// Whether `[from, to)` was read throughout: after counting began, with
    /// no gap in it, and every shard past its end.
    pub fn covered(&self, from: i64, to: i64) -> bool {
        self.since.is_some_and(|since| since <= from)
            && self.reached().is_some_and(|reached| reached >= to)
            && !self.gaps.iter().any(|&(a, b)| a < to && b > from)
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
}

impl Batch {
    pub fn of(actions: &[Action]) -> Batch {
        let mut hours: HashMap<i64, Vec<u64>> = HashMap::new();
        let mut days: HashMap<i64, Vec<u64>> = HashMap::new();
        let mut counts: HashMap<i64, HashMap<Family, u64>> = HashMap::new();
        for a in actions {
            hours.entry(hour(a.at)).or_default().push(a.fid);
            days.entry(day(a.at)).or_default().push(a.fid);
            *counts.entry(day(a.at)).or_default().entry(a.family).or_default() += 1;
        }
        Batch {
            hours: hours.into_iter().map(|(h, f)| (h, Fids::of(f))).collect(),
            days: days.into_iter().map(|(d, f)| (d, Fids::of(f))).collect(),
            actions: counts,
        }
    }
}

/// What `/api/v1/activity` answers: Farcaster's active accounts, as the
/// cron last counted them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityPayload {
    /// How far the events had been read, Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reached: Option<i64>,
    /// Accounts active in the 24 whole hours to the hour in progress, and
    /// in the 24 before them; none while those hours weren't all read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rolling_24h: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_24h: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_30d: Option<Window>,
    /// The latest 30 UTC days, oldest first; the last is in progress.
    #[serde(default)]
    pub days: Vec<DayCount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<(i64, i64)>,
    pub meta: crate::api::Meta,
}

/// Accounts active in the 24 hours `last - 23` to `last` (the hour in
/// progress, read up to where reading has reached), if all of them were
/// read. `hours` holds their sets; an hour with no set had no activity.
pub fn rolling(follow: &Follow, hours: &HashMap<i64, Fids>, last: i64) -> Option<u64> {
    let reached = follow.reached()?;
    let (from, to) = ((last - 23) * 3_600, ((last + 1) * 3_600).min(reached));
    follow.covered(from, to).then(|| Fids::union_len(((last - 23)..=last).filter_map(|h| hours.get(&h))) as u64)
}

/// The payload for `follow`'s counts, with the hourly sets of the past 48
/// hours.
pub fn summary(follow: &Follow, hours: &HashMap<i64, Fids>, meta: crate::api::Meta) -> ActivityPayload {
    let reached = follow.reached();
    let last = reached.map(hour);
    ActivityPayload {
        reached,
        rolling_24h: last.and_then(|h| rolling(follow, hours, h)),
        previous_24h: last.and_then(|h| rolling(follow, hours, h - 24)),
        active_30d: follow.active_30d,
        days: follow.days.iter().rev().take(KEEP_DAYS as usize).rev().cloned().collect(),
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

    fn follow(since: i64, reached: i64) -> Follow {
        Follow {
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
        assert_eq!(batch.hours[&hour(at)], Fids::of([1, 2]));
        assert_eq!(batch.hours[&(hour(at) + 1)], Fids::of([2]));
        assert_eq!(batch.days[&20_000], Fids::of([1, 2]));
        assert_eq!(batch.actions[&20_000][&Family::Reaction], 2);
        assert_eq!(batch.actions[&20_000][&Family::Cast], 1);
    }

    #[test]
    fn rolling_windows_count_each_account_once_and_only_when_read() {
        let now_hour = 500_000;
        let mut hours = HashMap::new();
        hours.insert(now_hour, Fids::of([1, 2]));
        hours.insert(now_hour - 23, Fids::of([2, 3]));
        hours.insert(now_hour - 24, Fids::of([4]));
        hours.insert(now_hour - 47, Fids::of([4, 5]));
        let reached = now_hour * 3_600 + 600;
        let f = follow((now_hour - 47) * 3_600, reached);
        assert_eq!(rolling(&f, &hours, now_hour), Some(3));
        assert_eq!(rolling(&f, &hours, now_hour - 24), Some(2));
        // Counting began after the earlier window started.
        let late = follow((now_hour - 30) * 3_600, reached);
        assert_eq!(rolling(&late, &hours, now_hour - 24), None);
        let meta = crate::api::Meta::default();
        let payload = summary(&f, &hours, meta);
        assert_eq!((payload.rolling_24h, payload.previous_24h, payload.reached), (Some(3), Some(2), Some(reached)));
        let json = serde_json::to_string(&payload).unwrap();
        assert_eq!(serde_json::from_str::<ActivityPayload>(&json).unwrap(), payload);
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
}
