// SPDX-License-Identifier: AGPL-3.0-or-later
//! Who is reading Allthing now: the small "● 12" in the header.
//!
//! A browser tells the Worker it is here with a beat (`POST
//! /api/v1/presence`), never as part of a page view: one tab beats for the
//! whole browser, only while it is visible, online and in use, every
//! [`next_interval`] seconds, a figure the Worker sets from how many are
//! here, so the beats stay few however many come (docs/BUDGET.md). The
//! Worker keeps one row per visitor for the last few minutes: a signed-in
//! reader who shows themselves by their account, anyone else by a keyed
//! hash of a random id their browser makes afresh each day, and never an IP.
//! Readers not shown by name are counted, and nothing else is said of them.

use serde::{Deserialize, Serialize};

use crate::format::short_number;
use crate::model::UserSummary;

/// The beat's endpoint.
pub const PRESENCE_PATH: &str = "/api/v1/presence";

/// The fewest and most seconds between a browser's beats.
pub const MIN_NEXT: u32 = 240;
pub const MAX_NEXT: u32 = 900;

/// Seconds past a browser's interval during which it still counts as here:
/// its next beat may come a little late.
pub const GRACE: u32 = 60;

/// How long a reader's tab goes without input (a pointer, a key, a scroll,
/// a touch or a playing video) before it stops beating, in milliseconds.
pub const IDLE_MS: f64 = 5.0 * 60.0 * 1000.0;

/// How far ahead of this browser's clock the last beat may seem before the
/// clock is taken to have been set back, in milliseconds: clocks step back
/// a little now and then, and a beat at once each time would be one too
/// many.
pub const CLOCK_SLACK_MS: f64 = 60_000.0;

/// Readers shown by name, at most, most recent first.
pub const SHOWN: usize = 12;

/// A row is written at most this often per visitor, in seconds: a beat
/// sooner than that changes nothing (`presence.seen` stays).
pub const WRITE_EVERY: i64 = 60;

/// Rows older than this many seconds are deleted: a little over the
/// longest window ([`MAX_NEXT`] + [`GRACE`]).
pub const KEEP: i64 = 1000;

/// What the hash of an anonymous visitor's id is keyed for, so it can't be
/// mistaken for any other of the Worker's MACs.
pub const ID_PURPOSE: &str = "allthing presence v1";

/// A beat, as the browser sends it: its random id for the day, and whether
/// a signed-in reader asked not to be shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Beat {
    pub id: String,
    #[serde(default)]
    pub hidden: bool,
}

/// What the Worker answers a beat with: how many are here, those shown by
/// name (at most [`SHOWN`], most recent first), and when to beat again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub online: u64,
    #[serde(default)]
    pub readers: Vec<UserSummary>,
    pub next: u32,
}

/// A browser's random id for the day: 16 random bytes as 32 lowercase hex
/// characters, and nothing else.
pub fn is_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Seconds between a browser's beats while `online` are here: ten per
/// reader, within [`MIN_NEXT`]..=[`MAX_NEXT`]. Up to about 90 readers at once
/// that keeps all the beats of a day under 8,640; past them, each reader
/// adds about 96 (docs/BUDGET.md).
pub fn next_interval(online: u64) -> u32 {
    (online.saturating_mul(10)).clamp(u64::from(MIN_NEXT), u64::from(MAX_NEXT)) as u32
}

/// Seconds since a visitor's last beat during which they count as here,
/// for browsers beating every `next` seconds.
pub fn online_window(next: u32) -> u32 {
    next.clamp(MIN_NEXT, MAX_NEXT) + GRACE
}

/// Whether a tab should beat now: it is visible, the browser is online,
/// the reader has done something in the last [`IDLE_MS`], and no tab of
/// this browser beat less than `next_ms` ago (`last_beat_ms`, the latest of
/// any tab's, kept in localStorage), unless that seems to be well in the
/// future: the clock was set back. Times are Unix milliseconds.
pub fn should_beat(
    now_ms: f64,
    last_beat_ms: Option<f64>,
    last_input_ms: f64,
    visible: bool,
    online: bool,
    next_ms: f64,
) -> bool {
    visible
        && online
        && now_ms - last_input_ms <= IDLE_MS
        && last_beat_ms.is_none_or(|at| now_ms - at >= next_ms || at - now_ms > CLOCK_SLACK_MS)
}

/// The UTC day of `now_ms` (Unix milliseconds), as days since 1970.
pub fn utc_day(now_ms: f64) -> i64 {
    (now_ms / 86_400_000.0).floor() as i64
}

/// The id a browser beats with today: its stored one if it is well formed
/// and was made today, else none (the caller makes a fresh one), so a
/// browser's beats can be told apart within a day and never across days.
pub fn todays_id(stored: Option<(&str, i64)>, today: i64) -> Option<String> {
    stored.filter(|(id, day)| *day == today && is_id(id)).map(|(id, _)| id.to_owned())
}

/// What the chip shows after its dot: the count, "12"; at least 1 while
/// this reader is here, and 1.2k past a thousand.
pub fn count_label(online: u64) -> String {
    short_number(online.max(1))
}

/// The chip's name for screen readers, which starts with the count it
/// shows (as WCAG's label in name asks): "12 readers online".
pub fn chip_name(online: u64) -> String {
    let online = online.max(1);
    format!("{} {} online", short_number(online), if online == 1 { "reader" } else { "readers" })
}

/// The readers to show and how many are here, from `answer` and `me`, the
/// signed-in reader if they show themselves: they lead the list if a
/// cached answer doesn't have them yet, and they are counted. No more than
/// [`SHOWN`] by name, and the count is never below those named, or 1.
pub fn with_me(answer: &Answer, me: Option<&UserSummary>) -> (Vec<UserSummary>, u64) {
    let mut readers = answer.readers.clone();
    let mut online = answer.online;
    if let Some(me) = me
        && !readers.iter().any(|r| r.fid == me.fid)
    {
        readers.insert(0, me.clone());
        online += 1;
    }
    readers.truncate(SHOWN);
    let online = online.max(readers.len() as u64).max(1);
    (readers, online)
}

/// The visitor key of a reader shown by name.
pub fn named_visitor(fid: u64) -> String {
    format!("f:{fid}")
}

/// The visitor key of anyone else: the hex of their id's keyed hash.
pub fn anonymous_visitor(mac_hex: &str) -> String {
    format!("a:{mac_hex}")
}

/// Whether `visitor` is a key the Worker stores: `f:<fid>`, or `a:` and
/// 64 lowercase hex characters (an HMAC-SHA256), so a row can hold nothing
/// else (no IP, no raw id).
pub fn is_visitor(visitor: &str) -> bool {
    if let Some(fid) = visitor.strip_prefix("f:") {
        return !fid.is_empty() && fid.bytes().all(|b| b.is_ascii_digit()) && fid.parse::<u64>().is_ok_and(|f| f > 0);
    }
    visitor
        .strip_prefix("a:")
        .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader(fid: u64, name: &str) -> UserSummary {
        UserSummary { fid, username: name.into(), display_name: String::new(), pfp_url: None }
    }

    #[test]
    fn an_id_is_32_lowercase_hex_characters_only() {
        assert!(is_id("0123456789abcdef0123456789abcdef"));
        assert!(!is_id("0123456789ABCDEF0123456789abcdef"));
        assert!(!is_id("0123456789abcdef0123456789abcde"));
        assert!(!is_id("0123456789abcdef0123456789abcdef0"));
        assert!(!is_id("0123456789abcdeg0123456789abcdef"));
        assert!(!is_id(""));
        assert!(!is_id("0x23456789abcdef0123456789abcdef"));
        // Multibyte characters aren't hex, whatever their length in bytes.
        assert!(!is_id("ééééééééééééééé0"));
    }

    #[test]
    fn the_interval_is_ten_seconds_a_reader_within_four_and_fifteen_minutes() {
        assert_eq!(next_interval(0), 240);
        assert_eq!(next_interval(1), 240);
        assert_eq!(next_interval(24), 240);
        assert_eq!(next_interval(25), 250);
        assert_eq!(next_interval(60), 600);
        assert_eq!(next_interval(90), 900);
        assert_eq!(next_interval(91), 900);
        assert_eq!(next_interval(u64::MAX), 900);
        // A reader counts for a minute past their interval.
        assert_eq!(online_window(240), 300);
        assert_eq!(online_window(900), 960);
        assert_eq!(online_window(0), 300);
        assert_eq!(online_window(5000), 960);
        // Rows outlive the longest window.
        assert!(KEEP > i64::from(online_window(MAX_NEXT)));
    }

    #[test]
    fn up_to_ninety_readers_beat_under_8640_times_a_day() {
        // Each reader's tab beats once per interval while it is in use.
        let day = 86_400u64;
        let beats = |online: u64| online * day / u64::from(next_interval(online));
        assert_eq!(beats(1), 360);
        assert_eq!(beats(24), 8640);
        assert_eq!(beats(90), 8640);
        // Past that, about 96 a day for each reader more.
        assert_eq!(beats(91) - beats(90), 96);
        assert!(beats(60) <= 8640);
    }

    #[test]
    fn a_tab_beats_only_when_visible_online_in_use_and_due() {
        let next = 240_000.0;
        let now = 1_000_000_000_000.0;
        // Due: no beat yet, or the last is `next` old.
        assert!(should_beat(now, None, now, true, true, next));
        assert!(should_beat(now, Some(now - next), now - 1000.0, true, true, next));
        // Not due: a tab of this browser beat less than `next` ago.
        assert!(!should_beat(now, Some(now - next + 1.0), now, true, true, next));
        // Hidden, offline or idle for more than five minutes: never.
        assert!(!should_beat(now, None, now, false, true, next));
        assert!(!should_beat(now, None, now, true, false, next));
        assert!(!should_beat(now, None, now - IDLE_MS - 1.0, true, true, next));
        assert!(should_beat(now, None, now - IDLE_MS, true, true, next));
        // A last beat well in the future (a clock set back) doesn't stop
        // beats; one a moment ahead (a clock stepped back a little) does.
        assert!(should_beat(now, Some(now + CLOCK_SLACK_MS + 1.0), now, true, true, next));
        assert!(!should_beat(now, Some(now + 250.0), now, true, true, next));
    }

    #[test]
    fn the_id_is_made_afresh_each_utc_day() {
        let id = "0123456789abcdef0123456789abcdef";
        let today = utc_day(1_792_540_800_000.0);
        assert_eq!(today, 20_747);
        assert_eq!(utc_day(1_792_540_800_000.0 - 1.0), 20_746);
        assert_eq!(todays_id(Some((id, today)), today), Some(id.to_owned()));
        assert_eq!(todays_id(Some((id, today - 1)), today), None);
        assert_eq!(todays_id(Some(("not hex", today)), today), None);
        assert_eq!(todays_id(None, today), None);
    }

    #[test]
    fn the_label_is_the_count_and_the_name_says_readers_online() {
        assert_eq!(count_label(12), "12");
        // Never below 1 while this reader is here.
        assert_eq!(count_label(0), "1");
        assert_eq!(count_label(1234), "1.2k");
        assert_eq!(count_label(999), "999");
        assert_eq!(chip_name(12), "12 readers online");
        assert_eq!(chip_name(0), "1 reader online");
        assert_eq!(chip_name(1), "1 reader online");
        assert_eq!(chip_name(1234), "1.2k readers online");
        // What the chip shows is in its name, wide or narrow.
        for online in [0, 7, 12, 999, 1234, 56_789, 3_000_000] {
            assert!(chip_name(online).starts_with(&format!("{} ", count_label(online))));
        }
    }

    #[test]
    fn the_signed_in_reader_leads_a_cached_answer_that_lacks_them() {
        let answer = Answer { online: 3, readers: vec![reader(2, "b"), reader(3, "c")], next: 240 };
        let me = reader(1, "me");
        let (readers, online) = with_me(&answer, Some(&me));
        assert_eq!(readers.iter().map(|r| r.fid).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(online, 4);
        // Already there: as it is.
        let (readers, online) = with_me(&answer, Some(&reader(3, "c")));
        assert_eq!(readers.iter().map(|r| r.fid).collect::<Vec<_>>(), [2, 3]);
        assert_eq!(online, 3);
        // Hidden or signed out: as it is, and at least 1.
        let (readers, online) = with_me(&Answer { online: 0, readers: vec![], next: 240 }, None);
        assert!(readers.is_empty());
        assert_eq!(online, 1);
        // No more than twelve by name.
        let full = Answer { online: 40, readers: (2..14).map(|f| reader(f, "x")).collect(), next: 400 };
        let (readers, online) = with_me(&full, Some(&me));
        assert_eq!(readers.len(), SHOWN);
        assert_eq!(readers[0].fid, 1);
        assert_eq!(online, 41);
    }

    #[test]
    fn a_visitor_is_an_account_or_a_hash_and_nothing_else() {
        assert_eq!(named_visitor(42), "f:42");
        assert!(is_visitor("f:42"));
        assert!(is_visitor(&anonymous_visitor(&"ab".repeat(32))));
        assert!(!is_visitor("f:0"));
        assert!(!is_visitor("f:"));
        assert!(!is_visitor("f:4x"));
        assert!(!is_visitor(&anonymous_visitor(&"ab".repeat(31))));
        assert!(!is_visitor(&anonymous_visitor(&"AB".repeat(32))));
        assert!(!is_visitor("203.0.113.7"));
        assert!(!is_visitor("a:0123456789abcdef0123456789abcdef"));
    }

    #[test]
    fn a_beat_and_an_answer_read_as_the_worker_and_the_browser_send_them() {
        let beat: Beat = serde_json::from_str(r#"{"id":"0123456789abcdef0123456789abcdef","hidden":false}"#).unwrap();
        assert!(!beat.hidden);
        let quiet: Beat = serde_json::from_str(r#"{"id":"0123456789abcdef0123456789abcdef"}"#).unwrap();
        assert!(!quiet.hidden);
        let answer: Answer = serde_json::from_str(
            r#"{"online":2,"readers":[{"fid":3,"username":"dwr","display_name":"Dan","pfp_url":"https://example.com/p.png"}],"next":240}"#,
        )
        .unwrap();
        assert_eq!(answer.online, 2);
        assert_eq!(answer.readers[0].username, "dwr");
        assert_eq!(answer.next, 240);
    }
}
