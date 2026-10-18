// SPDX-License-Identifier: AGPL-3.0-or-later
//! The newest casts, read from Snapchain's event stream. A page of a shard's
//! hub events says which top-level casts were added and which casts were
//! removed; the front page's "new" list is a rolling buffer of those casts,
//! kept fresh by reading only the events after the last ones seen.

use std::collections::HashSet;

use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

use crate::model::Cast;
use crate::provenance::base64_decode;

/// The shards that hold users' messages (shard 0 holds none).
pub const SHARDS: [u32; 2] = [1, 2];

/// Seconds from the Unix epoch to the Farcaster epoch (2021-01-01).
pub const FARCASTER_EPOCH: i64 = 1_609_459_200;

/// Where reading left off in one shard's event stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub shard: u32,
    /// The newest event read.
    pub event_id: u64,
}

/// A cast added at the top of a conversation, in a channel or not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Added {
    pub hash: String,
    pub fid: u64,
    /// Unix seconds.
    pub ts: i64,
}

/// What one page of a shard's events says about casts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CastEvents {
    pub added: Vec<Added>,
    /// Hashes of removed casts, as `0x…` hex.
    pub removed: Vec<String>,
    /// The newest event on the page.
    pub last_id: Option<u64>,
    /// How many events the page held, of every kind.
    pub count: usize,
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    events: Vec<Event>,
}

#[derive(Deserialize)]
struct Event {
    id: u64,
    #[serde(rename = "mergeMessageBody")]
    merge: Option<Merge>,
}

#[derive(Deserialize)]
struct Merge {
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    hash: Option<String>,
    data: Option<Data>,
}

#[derive(Deserialize)]
struct Data {
    #[serde(rename = "type")]
    kind: Option<String>,
    fid: Option<u64>,
    timestamp: Option<i64>,
    #[serde(rename = "castAddBody")]
    add: Option<AddBody>,
    #[serde(rename = "castRemoveBody")]
    remove: Option<RemoveBody>,
}

#[derive(Deserialize)]
struct AddBody {
    /// Set on replies; null or absent on top-level casts.
    #[serde(rename = "parentCastId")]
    parent: Option<IgnoredAny>,
}

#[derive(Deserialize)]
struct RemoveBody {
    #[serde(rename = "targetHash")]
    target: Option<String>,
}

/// Hub event JSON gives removed casts' hashes in base64.
fn base64_to_hex(value: &str) -> Option<String> {
    let bytes = base64_decode(value).filter(|b| b.len() == 20)?;
    Some(format!("0x{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()))
}

/// Read a `/v1/events` page: the top-level casts it adds, the casts it
/// removes, and where it ends. `None` if it isn't an events page.
pub fn cast_events(json: &[u8]) -> Option<CastEvents> {
    let page: Page = serde_json::from_slice(json).ok()?;
    let mut out = CastEvents { count: page.events.len(), ..Default::default() };
    for event in page.events {
        out.last_id = Some(out.last_id.map_or(event.id, |id| id.max(event.id)));
        let Some(Message { hash, data: Some(data) }) = event.merge.and_then(|m| m.message) else { continue };
        match data.kind.as_deref() {
            Some("MESSAGE_TYPE_CAST_ADD") if data.add.is_some_and(|a| a.parent.is_none()) => {
                if let (Some(hash), Some(fid), Some(ts)) = (hash, data.fid, data.timestamp) {
                    out.added.push(Added { hash: hash.to_ascii_lowercase(), fid, ts: ts + FARCASTER_EPOCH });
                }
            }
            Some("MESSAGE_TYPE_CAST_REMOVE") => {
                if let Some(hex) = data.remove.and_then(|r| r.target).and_then(|t| base64_to_hex(&t)) {
                    out.removed.push(hex);
                }
            }
            _ => {}
        }
    }
    Some(out)
}

/// The hashes to read in one bulk call of at most `limit`: the added casts,
/// newest first, then the newest casts already listed, whose counts that
/// refreshes.
pub fn to_hydrate(added: &[Added], listed: &[Cast], removed: &[String], limit: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut added: Vec<&Added> = added.iter().collect();
    added.sort_by(|a, b| b.ts.cmp(&a.ts).then_with(|| a.hash.cmp(&b.hash)));
    added
        .into_iter()
        .map(|a| &a.hash)
        .chain(listed.iter().map(|c| &c.hash))
        .filter(|hash| !removed.contains(hash))
        .filter(|hash| seen.insert(hash.as_str()))
        .take(limit)
        .cloned()
        .collect()
}

/// The newest `keep` casts of `listed` and `fresh` (newly read, so they
/// replace listed copies), without `removed` ones.
pub fn newest(listed: Vec<Cast>, fresh: Vec<Cast>, removed: &[String], keep: usize) -> Vec<Cast> {
    let fresh_hashes: HashSet<String> = fresh.iter().map(|c| c.hash.clone()).collect();
    let mut all: Vec<Cast> = fresh
        .into_iter()
        .chain(listed.into_iter().filter(|c| !fresh_hashes.contains(&c.hash)))
        .filter(|c| !removed.contains(&c.hash))
        .collect();
    all.sort_by(|a, b| b.ts.cmp(&a.ts).then_with(|| a.hash.cmp(&b.hash)));
    all.truncate(keep);
    all
}

/// How fresh a cached list is, for `/status`: its newest cast's age and its
/// own, in seconds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Freshness {
    pub newest_cast_age_s: i64,
    pub built_age_s: i64,
    pub casts: usize,
}

/// The freshness of a list payload (anything with a `pool` of casts) built
/// at `fetched_at`, as of `now` (both Unix milliseconds).
pub fn freshness(body: &[u8], fetched_at: f64, now: f64) -> Option<Freshness> {
    #[derive(Deserialize)]
    struct Listed {
        ts: i64,
    }
    #[derive(Deserialize)]
    struct List {
        pool: Vec<Listed>,
    }
    let list: List = serde_json::from_slice(body).ok()?;
    let newest = list.pool.iter().map(|c| c.ts).max()?;
    Some(Freshness {
        newest_cast_age_s: ((now / 1000.0) as i64 - newest).max(0),
        built_age_s: ((now - fetched_at) / 1000.0).round().max(0.0) as i64,
        casts: list.pool.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::UserSummary;

    /// A page as Snapchain's HTTP API serves it: a block confirmation, a
    /// top-level cast, a channel cast, a reply, a removal and a reaction.
    const PAGE: &str = r#"{"events":[
      {"type":"HUB_EVENT_TYPE_BLOCK_CONFIRMED","id":802629369856,"blockConfirmedBody":{"blockNumber":48988609,"shardIndex":1}},
      {"type":"HUB_EVENT_TYPE_MERGE_MESSAGE","id":802629386240,"mergeMessageBody":{"message":{
        "hash":"0xF39C5F04A57B2ACAACB5983BDBF44F08DB58C216","signer":"0x2afe",
        "data":{"type":"MESSAGE_TYPE_CAST_ADD","fid":1019597,"timestamp":182866299,
          "castAddBody":{"parentCastId":null,"parentUrl":null,"text":"gm","embeds":[]}}},"deletedMessages":[]}},
      {"type":"HUB_EVENT_TYPE_MERGE_MESSAGE","id":802629386241,"mergeMessageBody":{"message":{
        "hash":"0x1111111111111111111111111111111111111111",
        "data":{"type":"MESSAGE_TYPE_CAST_ADD","fid":3,"timestamp":182866300,
          "castAddBody":{"parentUrl":"https://farcaster.xyz/~/channel/dev","text":"in a channel"}}}}},
      {"type":"HUB_EVENT_TYPE_MERGE_MESSAGE","id":802629386242,"mergeMessageBody":{"message":{
        "hash":"0x2222222222222222222222222222222222222222",
        "data":{"type":"MESSAGE_TYPE_CAST_ADD","fid":5,"timestamp":182866301,
          "castAddBody":{"parentCastId":{"fid":3,"hash":"0x1111111111111111111111111111111111111111"},"text":"a reply"}}}}},
      {"type":"HUB_EVENT_TYPE_MERGE_MESSAGE","id":802629386243,"mergeMessageBody":{"message":{
        "hash":"0x3333333333333333333333333333333333333333",
        "data":{"type":"MESSAGE_TYPE_CAST_REMOVE","fid":478804,"timestamp":182865772,
          "castRemoveBody":{"targetHash":"MwDCsNIIAE0e6G2gW0WL+TQUCqQ="}}}}},
      {"type":"HUB_EVENT_TYPE_MERGE_MESSAGE","id":802629386244,"mergeMessageBody":{"message":{
        "hash":"0x4444444444444444444444444444444444444444",
        "data":{"type":"MESSAGE_TYPE_REACTION_ADD","fid":9,"timestamp":182866302,"reactionBody":{"type":"REACTION_TYPE_LIKE"}}}}}
    ]}"#;

    #[test]
    fn reads_top_level_casts_and_removals_from_an_events_page() {
        let events = cast_events(PAGE.as_bytes()).unwrap();
        assert_eq!(events.count, 6);
        assert_eq!(events.last_id, Some(802629386244));
        assert_eq!(
            events.added,
            vec![
                Added {
                    hash: "0xf39c5f04a57b2acaacb5983bdbf44f08db58c216".into(),
                    fid: 1019597,
                    ts: 182866299 + FARCASTER_EPOCH
                },
                Added {
                    hash: "0x1111111111111111111111111111111111111111".into(),
                    fid: 3,
                    ts: 182866300 + FARCASTER_EPOCH
                },
            ]
        );
        // The removal's base64 target, as the hex hash casts are listed by.
        assert_eq!(events.removed, ["0x3300c2b0d208004d1ee86da05b458bf934140aa4"]);
        assert_eq!(cast_events(b"{\"events\":[]}").unwrap().last_id, None);
        assert_eq!(cast_events(b"not json"), None);
    }

    fn cast(hash: &str, ts: i64, likes: u64) -> Cast {
        Cast {
            hash: hash.into(),
            author: UserSummary { fid: 1, username: "a".into(), display_name: String::new(), pfp_url: None },
            text: String::new(),
            mentions: vec![],
            ts,
            parent_hash: None,
            parent_url: None,
            channel: None,
            embeds: vec![],
            likes,
            recasts: 0,
            replies: 0,
            mini_app: false,
            spam: false,
        }
    }

    #[test]
    fn hydrates_new_casts_first_then_refreshes_the_newest_listed() {
        let added = vec![Added { hash: "0xa".into(), fid: 1, ts: 10 }, Added { hash: "0xb".into(), fid: 1, ts: 20 }];
        let listed = vec![cast("0xc", 5, 0), cast("0xa", 10, 0), cast("0xd", 4, 0)];
        assert_eq!(to_hydrate(&added, &listed, &[], 10), ["0xb", "0xa", "0xc", "0xd"]);
        assert_eq!(to_hydrate(&added, &listed, &["0xc".into()], 3), ["0xb", "0xa", "0xd"]);
    }

    #[test]
    fn reports_how_old_a_list_and_its_newest_cast_are() {
        let body = br#"{"pool":[{"ts":1000,"hash":"0x1"},{"ts":1090}],"meta":{"fetched_at":1}}"#;
        let fresh = freshness(body, 1_095_000.0, 1_100_000.0).unwrap();
        assert_eq!(fresh, Freshness { newest_cast_age_s: 10, built_age_s: 5, casts: 2 });
        assert_eq!(freshness(br#"{"pool":[]}"#, 0.0, 0.0), None);
    }

    #[test]
    fn keeps_the_newest_casts_with_fresh_counts_and_without_removed_ones() {
        let listed = vec![cast("0xa", 30, 1), cast("0xb", 20, 1), cast("0xc", 10, 1)];
        let fresh = vec![cast("0xd", 40, 0), cast("0xb", 20, 7)];
        let kept = newest(listed, fresh, &["0xa".into()], 3);
        let summary: Vec<(&str, u64)> = kept.iter().map(|c| (c.hash.as_str(), c.likes)).collect();
        assert_eq!(summary, [("0xd", 0), ("0xb", 7), ("0xc", 1)]);
    }
}
