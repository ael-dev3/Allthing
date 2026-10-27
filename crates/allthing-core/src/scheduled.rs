// SPDX-License-Identifier: AGPL-3.0-or-later
//! Scheduled casts. A signed-in reader writes a cast and picks when it goes
//! out; their browser signs it at once with their app key, with that time as
//! its timestamp, and hands the signed message to the Worker. The Worker
//! keeps it in D1 and gives it to the node at its time
//! (`allthing_data::scheduled`), as the browser would have.
//!
//! The Worker never has the key and signs nothing: it checks that the
//! message is a cast by the signed-in reader, for a time ahead, and keeps
//! its bytes. The network refuses a message timestamped more than ten
//! minutes ahead, so nothing can post one early; at its time it is a cast
//! like any other. Editing one signs it again, in its place; cancelling
//! deletes the stored message. From the moment the cron takes a cast to give
//! the node until it has said what became of it, the cast is `sending`: too
//! late to change or cancel, and a cancel says so ([`cancelled`]).

use serde::{Deserialize, Serialize};

use crate::message::{EPOCH, message_hash, open_envelope, read_data};
use crate::provenance::{base64_decode, hex_encode};

/// The reader's scheduled casts: listed by GET, scheduled (or one put in
/// another's place) by POST.
pub const SCHEDULED_PATH: &str = "/api/v1/scheduled";

/// Where one is cancelled, by POST.
pub fn cancel_path() -> String {
    format!("{SCHEDULED_PATH}/cancel")
}

/// How far ahead a cast may be scheduled: a minute at least, 30 days at
/// most. The Worker takes one a little short of a minute ahead, for the
/// time the request took.
pub const MIN_AHEAD: i64 = 60;
pub const MAX_AHEAD: i64 = 30 * 86_400;
const SLACK: i64 = 30;
/// The most casts a reader may have waiting.
pub const MAX_WAITING: u32 = 25;
/// The largest signed message kept: a cast of 1,024 bytes with two long
/// links, its mentions and its signature come to well under this.
pub const MAX_MESSAGE_BYTES: usize = 2048;
/// The largest draft kept with it, for the reader's list and for editing.
pub const MAX_DRAFT_BYTES: usize = 6 * 1024;
/// How long a cast that went out, or didn't, stays in the reader's list.
pub const KEEP_DONE: i64 = 7 * 86_400;
/// How many times one is offered to a node that can't take it just now,
/// a minute apart, before it is given up.
pub const MAX_TRIES: u32 = 30;

/// A cast to schedule: its signed message (an envelope, in base64) and the
/// draft as the composer had it, which only the reader's browser reads. With
/// `replaces`, it takes the place of a cast that hasn't gone out yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub message: String,
    pub draft: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<i64>,
}

/// A scheduled cast to cancel, or one that didn't go out to take off the
/// list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cancel {
    pub id: i64,
}

/// What a cancel did: the Worker's answer to one of the reader's. A cancel
/// of someone else's cast is refused instead (403).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "lowercase")]
pub enum Cancelled {
    /// Off the list: a cast waiting won't go out, and one that didn't is
    /// gone. A cancel asked again finds it gone, and says so too.
    Cancelled,
    /// Too late: it went out, as `hash`, before the cancel came.
    Posted { hash: String },
    /// Too late: the node was being given it, as `hash`, as the cancel came.
    Sending { hash: String },
}

impl Cancelled {
    /// The Worker's answer, as JSON, to a cancel of the cast the list had
    /// as `hash`. One that says nothing of it (`{}`, as the Worker answered
    /// before it said) took the cast off. Read by hand, as the browser reads
    /// it: serde's way with a tagged enum would bring it several kilobytes
    /// of code.
    pub fn read(answer: &serde_json::Value, hash: &str) -> Cancelled {
        let field = |name| answer.get(name).and_then(serde_json::Value::as_str);
        let hash = field("hash").unwrap_or(hash).to_owned();
        match field("outcome") {
            Some("posted") => Cancelled::Posted { hash },
            Some("sending") => Cancelled::Sending { hash },
            _ => Cancelled::Cancelled,
        }
    }
}

/// How D1 keeps a cast from when the cron takes it to give the node until it
/// has said what became of it: listed as waiting ([`State::listed`]), but too
/// late to change or cancel.
pub const SENDING: &str = "sending";

/// What a cancel by `fid` does to the cast it names, as D1 kept it just
/// before: `Some((owner, state, hash))`, or `None` once it's gone. `None` when
/// it is someone else's; otherwise the answer. A cast waiting, or one that
/// failed, is taken off, as `allthing-worker`'s `CANCEL` deletes everything
/// but those that went out or are going.
pub fn cancelled(fid: u64, kept: Option<(u64, &str, &str)>) -> Option<Cancelled> {
    match kept {
        // Taken off already (a cancel sent twice), or never kept.
        None => Some(Cancelled::Cancelled),
        Some((owner, ..)) if owner != fid => None,
        Some((_, state, hash)) => Some(match state {
            "posted" => Cancelled::Posted { hash: hash.to_owned() },
            SENDING => Cancelled::Sending { hash: hash.to_owned() },
            _ => Cancelled::Cancelled,
        }),
    }
}

/// Where a scheduled cast stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Waiting,
    Posted,
    Failed,
}

impl State {
    /// As D1 keeps it.
    pub fn as_str(self) -> &'static str {
        match self {
            State::Waiting => "waiting",
            State::Posted => "posted",
            State::Failed => "failed",
        }
    }

    pub fn parse(text: &str) -> Option<State> {
        match text {
            "waiting" => Some(State::Waiting),
            "posted" => Some(State::Posted),
            "failed" => Some(State::Failed),
            _ => None,
        }
    }

    /// Where a cast D1 keeps as `text` stands for its reader's list: one the
    /// node is being given ([`SENDING`]) is still waiting, to them.
    pub fn listed(text: &str) -> Option<State> {
        if text == SENDING { Some(State::Waiting) } else { State::parse(text) }
    }
}

/// One of the reader's scheduled casts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledCast {
    pub id: i64,
    /// When it goes out, or went: Unix seconds.
    pub at: i64,
    /// The cast's hash, `0x…`, which is its link once it is out.
    pub hash: String,
    pub state: State,
    /// Why it didn't go out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub draft: String,
}

/// The reader's scheduled casts: those waiting, and those of the past week
/// that went out or didn't, soonest first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledList {
    pub casts: Vec<ScheduledCast>,
}

/// What a browser knows of its reader's casts waiting, for the account
/// menu's count without asking the Worker: their ids and times, from their
/// list when it was last read there, and from what was scheduled, changed or
/// cancelled there since. A browser keeps it (`localStorage`); what another
/// device did shows once the list is read again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Known {
    pub fid: u64,
    /// `(id, at)`: each cast's id, and when it goes out (Unix seconds).
    pub waiting: Vec<(i64, i64)>,
}

impl Known {
    /// As kept; anything unreadable knows nothing.
    pub fn parse(text: &str) -> Known {
        serde_json::from_str(text).unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// `fid`'s list as read: the casts waiting in it.
    pub fn listed(fid: u64, casts: &[ScheduledCast]) -> Known {
        let waiting = casts.iter().filter(|c| c.state == State::Waiting).map(|c| (c.id, c.at)).collect();
        Known { fid, waiting }
    }

    /// How many of `fid`'s casts are still to go out at `now`.
    pub fn count(&self, fid: u64, now: i64) -> usize {
        if self.fid == fid { self.waiting.iter().filter(|(_, at)| *at > now).count() } else { 0 }
    }

    /// `fid` scheduled `cast`, or put it in the place of `replaces`.
    pub fn kept(&mut self, fid: u64, cast: &ScheduledCast, replaces: Option<i64>) {
        if self.fid != fid {
            *self = Known { fid, waiting: Vec::new() };
        }
        self.waiting.retain(|(id, _)| *id != cast.id && Some(*id) != replaces);
        self.waiting.push((cast.id, cast.at));
    }

    /// One of `fid`'s casts is off the list.
    pub fn gone(&mut self, fid: u64, id: i64) {
        if self.fid == fid {
            self.waiting.retain(|(kept, _)| *kept != id);
        }
    }
}

/// What the composer had, kept with a scheduled cast as its `draft`: for
/// the reader's list, and to edit it. The Worker keeps it as it is given.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedDraft {
    /// As written, "@name"s and all.
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gif: Option<SavedGif>,
    /// The images and videos it carries, uploaded, in order: their URLs,
    /// which the cast embeds, and their types (`allthing_core::media`).
    /// Drafts kept before attachments have none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media: Vec<SavedMedia>,
    /// The cast it replies to, or quotes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<SavedCast>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<SavedCast>,
    /// The channel it goes to, and the parent URL that puts it there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_url: Option<String>,
}

/// A GIF or a sticker: what the cast embeds, and the picture the composer
/// shows for it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedGif {
    pub title: String,
    pub embed: String,
    pub tile: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
}

/// An image or a video a draft carries, uploaded.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedMedia {
    pub url: String,
    #[serde(rename = "type")]
    pub media_type: String,
}

/// The cast a draft replies to or quotes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedCast {
    pub hash: String,
    pub fid: u64,
    pub username: String,
    #[serde(default)]
    pub excerpt: String,
    /// Full account provenance survives excerpt truncation and draft reloads.
    #[serde(default, skip_serializing_if = "crate::moderation::References::is_empty")]
    pub references: crate::moderation::References,
}

impl SavedDraft {
    /// A kept draft, or an empty one if it can't be read.
    pub fn parse(draft: &str) -> SavedDraft {
        serde_json::from_str(draft).unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// What it answers, quotes or goes to, for the list: "Reply to @alice",
    /// "Quoting @bob", "In /farcaster".
    pub fn context(&self) -> Option<String> {
        if let Some(to) = &self.reply {
            Some(format!("Reply to @{}", to.username))
        } else if let Some(of) = &self.quote {
            Some(format!("Quoting @{}", of.username))
        } else {
            self.channel.as_ref().map(|id| format!("In /{id}"))
        }
    }
}

#[cfg(test)]
mod reference_tests {
    use super::*;

    #[test]
    fn saved_target_provenance_survives_excerpt_truncation_and_legacy_drafts_parse() {
        let legacy =
            SavedDraft::parse(r#"{"text":"","reply":{"hash":"0x01","fid":42,"username":"alice","excerpt":"short"}}"#);
        assert!(legacy.reply.unwrap().references.is_empty());
        let mut references = crate::moderation::References::default();
        references.add_fid(42);
        references.add_fid(99);
        let draft = SavedDraft {
            text: "My editable draft".into(),
            reply: Some(SavedCast {
                hash: "0x01".into(),
                fid: 42,
                username: "alice".into(),
                excerpt: "short".into(),
                references,
            }),
            ..SavedDraft::default()
        };
        let restored = SavedDraft::parse(&draft.to_json());
        assert_eq!(restored, draft);
        assert_eq!(restored.reply.unwrap().references.fids, [42, 99]);
    }
}

/// How long until a scheduled cast goes out, `seconds` from now: "in 5
/// min", "in 3 h", "in 2 days"; or "now" once its time has come.
pub fn until(seconds: i64) -> String {
    match seconds {
        ..=59 => "now".to_owned(),
        60..3_600 => format!("in {} min", seconds / 60),
        3_600..86_400 => format!("in {} h", seconds / 3_600),
        86_400..172_800 => "in 1 day".to_owned(),
        _ => format!("in {} days", seconds / 86_400),
    }
}

/// A signed cast, checked for scheduling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    /// When it goes out: its timestamp, in Unix seconds.
    pub at: i64,
    /// Its hash, `0x…`.
    pub hash: String,
}

/// Whether `message` (base64) is a cast `fid` signed for a time ahead of
/// `now`, and when it goes out; or why not, for the reader. Its signature is
/// the node's to check when it is given the cast.
pub fn check(message: &str, fid: u64, now: i64) -> Result<Checked, &'static str> {
    const NOT_SIGNED: &str = "That isn't a signed cast.";
    let bytes = base64_decode(message).ok_or(NOT_SIGNED)?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err("That cast is too large to schedule.");
    }
    let opened = open_envelope(&bytes).ok_or(NOT_SIGNED)?;
    let well_formed = opened.hash.len() == 20
        && opened.hash_scheme == 1
        && opened.signature.len() == 64
        && opened.signature_scheme == 1
        && opened.signer.len() == 32;
    if !well_formed || message_hash(opened.data)[..] != *opened.hash {
        return Err(NOT_SIGNED);
    }
    let data = read_data(opened.data).ok_or(NOT_SIGNED)?;
    if data.message_type != 1 || data.body_field != 5 || data.network != 1 {
        return Err("Only casts can be scheduled.");
    }
    if data.fid != fid {
        return Err("That cast is someone else's.");
    }
    let at = i64::try_from(data.timestamp).map_err(|_| NOT_SIGNED)? + EPOCH;
    if at < now + MIN_AHEAD - SLACK {
        return Err("That time has passed: pick one at least a minute from now.");
    }
    if at > now + MAX_AHEAD + SLACK {
        return Err("Pick a time within 30 days.");
    }
    Ok(Checked { at, hash: hex_encode(opened.hash) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Body, CastId, MessageData, NewCast, Reaction, envelope};
    use crate::provenance::base64_encode;

    const NOW: i64 = 1_800_000_000;

    fn signed(fid: u64, at: i64, body: Body) -> String {
        let data = MessageData::new(fid, at, body).encode();
        let hash = message_hash(&data);
        base64_encode(&envelope(&data, &hash, &[7; 64], &[9; 32]))
    }

    fn cast(text: &str) -> Body {
        Body::CastAdd(NewCast { text: text.into(), ..NewCast::default() })
    }

    #[test]
    fn a_cast_signed_for_later_is_taken_with_its_time_and_hash() {
        let message = signed(3, NOW + 3_600, cast("In an hour"));
        let checked = check(&message, 3, NOW).unwrap();
        assert_eq!(checked.at, NOW + 3_600);
        let bytes = base64_decode(&message).unwrap();
        assert_eq!(checked.hash, hex_encode(open_envelope(&bytes).unwrap().hash));
        // A minute ahead, and 30 days, are within reach; a request that took
        // a few seconds still makes the minute.
        assert!(check(&signed(3, NOW + 60, cast("Soon")), 3, NOW + 5).is_ok());
        assert!(check(&signed(3, NOW + MAX_AHEAD, cast("Far")), 3, NOW).is_ok());
    }

    #[test]
    fn anything_else_is_refused_with_why() {
        let later = NOW + 3_600;
        assert_eq!(check(&signed(4, later, cast("Mine?")), 3, NOW), Err("That cast is someone else's."));
        let like =
            Body::ReactionAdd(Reaction::Like, CastId::new(2, "0x0102030405060708090a0b0c0d0e0f1011121314").unwrap());
        assert_eq!(check(&signed(3, later, like), 3, NOW), Err("Only casts can be scheduled."));
        assert_eq!(
            check(&signed(3, NOW + 10, cast("Now")), 3, NOW),
            Err("That time has passed: pick one at least a minute from now.")
        );
        assert_eq!(
            check(&signed(3, NOW - 600, cast("Before")), 3, NOW).unwrap_err(),
            "That time has passed: pick one at least a minute from now."
        );
        assert_eq!(
            check(&signed(3, NOW + MAX_AHEAD + 3_600, cast("Too far")), 3, NOW),
            Err("Pick a time within 30 days.")
        );
        assert_eq!(check("not base64!", 3, NOW), Err("That isn't a signed cast."));
        assert_eq!(check(&base64_encode(b"plain words"), 3, NOW), Err("That isn't a signed cast."));
        assert_eq!(check(&base64_encode(&[0u8; 3000]), 3, NOW), Err("That cast is too large to schedule."));
        // Its data changed after it was signed: the hash no longer matches.
        let mut bytes = base64_decode(&signed(3, later, cast("Original"))).unwrap();
        let at = bytes.windows(8).position(|w| w == b"Original").unwrap();
        bytes[at] = b'X';
        assert_eq!(check(&base64_encode(&bytes), 3, NOW), Err("That isn't a signed cast."));
    }

    #[test]
    fn a_draft_is_kept_as_written_and_read_back() {
        let draft = SavedDraft {
            text: "Hi @alice :pepe_heart_hug:".into(),
            gif: Some(SavedGif {
                title: "Wave".into(),
                embed: "https://x.test/w.gif".into(),
                tile: "https://x.test/w.webp".into(),
                width: 200,
                height: 150,
            }),
            reply: Some(SavedCast {
                hash: "0x01".into(),
                fid: 42,
                username: "alice".into(),
                excerpt: "Pick".into(),
                ..SavedCast::default()
            }),
            ..SavedDraft::default()
        };
        assert_eq!(SavedDraft::parse(&draft.to_json()), draft);
        assert_eq!(draft.context().as_deref(), Some("Reply to @alice"));
        let quote = SavedDraft {
            quote: Some(SavedCast { username: "bob".into(), ..SavedCast::default() }),
            ..SavedDraft::default()
        };
        assert_eq!(quote.context().as_deref(), Some("Quoting @bob"));
        let channel = SavedDraft { channel: Some("farcaster".into()), ..SavedDraft::default() };
        assert_eq!(channel.context().as_deref(), Some("In /farcaster"));
        // Its images and videos, in order; a draft kept before them has none.
        let attached = SavedDraft {
            text: "two".into(),
            media: vec![
                SavedMedia { url: "https://allthing.social/m/1/a.png".into(), media_type: "image/png".into() },
                SavedMedia { url: "https://allthing.social/m/1/b.mp4".into(), media_type: "video/mp4".into() },
            ],
            ..SavedDraft::default()
        };
        assert_eq!(
            attached.to_json(),
            r#"{"text":"two","media":[{"url":"https://allthing.social/m/1/a.png","type":"image/png"},{"url":"https://allthing.social/m/1/b.mp4","type":"video/mp4"}]}"#
        );
        assert_eq!(SavedDraft::parse(&attached.to_json()), attached);
        assert_eq!(SavedDraft::parse(r#"{"text":"before","gif":null}"#).media, Vec::<SavedMedia>::new());
        // Empty fields are left out, and anything unreadable reads as empty.
        assert_eq!(SavedDraft { text: "x".into(), ..SavedDraft::default() }.to_json(), r#"{"text":"x"}"#);
        assert_eq!(SavedDraft::parse("not json"), SavedDraft::default());
        assert_eq!(SavedDraft::default().context(), None);
    }

    #[test]
    fn how_long_until_it_goes_out() {
        assert_eq!(until(-30), "now");
        assert_eq!(until(59), "now");
        assert_eq!(until(60), "in 1 min");
        assert_eq!(until(3_599), "in 59 min");
        assert_eq!(until(3_600), "in 1 h");
        assert_eq!(until(86_399), "in 23 h");
        assert_eq!(until(86_400), "in 1 day");
        assert_eq!(until(30 * 86_400), "in 30 days");
    }

    #[test]
    fn states_are_kept_as_words() {
        for state in [State::Waiting, State::Posted, State::Failed] {
            assert_eq!(State::parse(state.as_str()), Some(state));
            assert_eq!(serde_json::to_string(&state).unwrap(), format!("\"{}\"", state.as_str()));
        }
        assert_eq!(State::parse("sent"), None);
        let schedule: Schedule = serde_json::from_str(r#"{"message":"AA==","draft":"{}"}"#).unwrap();
        assert_eq!(schedule.replaces, None);
        assert_eq!(cancel_path(), "/api/v1/scheduled/cancel");
        // One the node is being given is still waiting, to its reader.
        assert_eq!(State::parse(SENDING), None);
        assert_eq!(State::listed(SENDING), Some(State::Waiting));
        for state in [State::Waiting, State::Posted, State::Failed] {
            assert_eq!(State::listed(state.as_str()), Some(state));
        }
        assert_eq!(State::listed("sent"), None);
    }

    #[test]
    fn a_cancel_takes_off_the_readers_own_and_says_when_its_too_late() {
        let hash = "0x0102030405060708090a0b0c0d0e0f1011121314";
        // Waiting, or failed: taken off.
        assert_eq!(cancelled(3, Some((3, "waiting", hash))), Some(Cancelled::Cancelled));
        assert_eq!(cancelled(3, Some((3, "failed", hash))), Some(Cancelled::Cancelled));
        // Gone already: a cancel sent twice says the same as the first.
        assert_eq!(cancelled(3, None), Some(Cancelled::Cancelled));
        // The cron got there first: out, or on its way, with its link.
        assert_eq!(cancelled(3, Some((3, "posted", hash))), Some(Cancelled::Posted { hash: hash.into() }));
        assert_eq!(cancelled(3, Some((3, SENDING, hash))), Some(Cancelled::Sending { hash: hash.into() }));
        // Someone else's, whatever it is.
        for state in ["waiting", "failed", "posted", SENDING] {
            assert_eq!(cancelled(4, Some((3, state, hash))), None);
        }
    }

    #[test]
    fn a_browser_counts_the_casts_it_knows_are_waiting() {
        let cast = |id: i64, at: i64, state: State| ScheduledCast {
            id,
            at,
            hash: format!("0x{id:02x}"),
            state,
            error: None,
            draft: "{}".into(),
        };
        let read = [
            cast(1, NOW + 60, State::Waiting),
            cast(2, NOW - 600, State::Posted),
            cast(3, NOW + 7_200, State::Waiting),
        ];
        let mut known = Known::listed(3, &read);
        assert_eq!(known.waiting, [(1, NOW + 60), (3, NOW + 7_200)]);
        assert_eq!(known.count(3, NOW), 2);
        // Once a cast's time has come, it isn't waiting any more.
        assert_eq!(known.count(3, NOW + 60), 1);
        // Another account's browser knows nothing of these.
        assert_eq!(known.count(4, NOW), 0);
        // Scheduled here, changed (in its own place, or another's), cancelled.
        known.kept(3, &cast(9, NOW + 900, State::Waiting), None);
        assert_eq!(known.count(3, NOW), 3);
        known.kept(3, &cast(9, NOW + 1_800, State::Waiting), Some(9));
        assert_eq!(known.waiting, [(1, NOW + 60), (3, NOW + 7_200), (9, NOW + 1_800)]);
        known.kept(3, &cast(1, NOW + 120, State::Waiting), Some(1));
        assert_eq!(known.count(3, NOW), 3);
        known.gone(3, 3);
        known.gone(4, 9);
        assert_eq!(known.waiting, [(9, NOW + 1_800), (1, NOW + 120)]);
        // Kept as JSON; another account's scheduling starts afresh.
        assert_eq!(Known::parse(&known.to_json()), known);
        assert_eq!(Known::parse("not json"), Known::default());
        known.kept(4, &cast(20, NOW + 60, State::Waiting), None);
        assert_eq!(known, Known { fid: 4, waiting: vec![(20, NOW + 60)] });
    }

    #[test]
    fn a_cancels_answer_says_what_it_did() {
        let posted = Cancelled::Posted { hash: "0xab".into() };
        assert_eq!(serde_json::to_string(&Cancelled::Cancelled).unwrap(), r#"{"outcome":"cancelled"}"#);
        assert_eq!(serde_json::to_string(&posted).unwrap(), r#"{"outcome":"posted","hash":"0xab"}"#);
        let sending = serde_json::json!({"outcome": "sending", "hash": "0xcd"});
        assert_eq!(Cancelled::read(&sending, "0x01"), Cancelled::Sending { hash: "0xcd".into() });
        assert_eq!(Cancelled::read(&serde_json::to_value(&posted).unwrap(), "0x01"), posted);
        // The Worker's answer before it said: off the list.
        assert_eq!(Cancelled::read(&serde_json::json!({}), "0x01"), Cancelled::Cancelled);
        assert_eq!(Cancelled::read(&serde_json::json!({"outcome": "cancelled"}), "0x01"), Cancelled::Cancelled);
        // Too late, with no hash said: the cast's, as the list had it.
        let bare = serde_json::json!({"outcome": "posted"});
        assert_eq!(Cancelled::read(&bare, "0x01"), Cancelled::Posted { hash: "0x01".into() });
    }
}
