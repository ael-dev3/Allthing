// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing's own index of notable casts, for the time ranges Hypersnap's
//! trending feed can't rank: Past month, Past year and All time.
//!
//! A cron job in the Worker reads one page of casts a minute. Every
//! [`TRENDING_EVERY`]th run refreshes a page of the past week's trending
//! casts; the others read a channel's or a notable author's casts further
//! back in time, one page per run, in turn. Each run keeps the casts whose
//! points could rank ([`worth_indexing`]) in D1, and when a ranking is due
//! ([`due`]) recomputes it there: the span's top [`RANKED`] casts by points,
//! stored as one row that the front page reads. Each run's D1 use is counted
//! against a daily share of the free plan's limits.
//!
//! The logic is here; `allthing-data` does the reading and the Worker the SQL.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::model::Cast;
use crate::normalize::{self, RawCast};
use crate::ranking::{RECAST_WEIGHT, TrendingWindow, may_ingest_timestamp, points};
use crate::time::parse_rfc3339;

const DAY: i64 = 86_400;

/// Casts each span's ranking keeps: ten pages of 100, as trending has.
pub const RANKED: usize = 1000;
/// Casts with fewer points are never indexed.
pub const MIN_POINTS: u64 = 20;
/// An author with a cast this good is worth reading back in time.
pub const NOTABLE_POINTS: u64 = 250;
/// Feeds read back in turn, at most; notable authors queue for a free place.
pub const MAX_FEEDS: usize = 120;
/// Feeds read to their start, remembered so they are not read again.
pub const MAX_FINISHED: usize = 2000;
/// One run in this many refreshes a trending page; the others read back.
pub const TRENDING_EVERY: u64 = 4;
/// The last trending page (the list ends at 1,000 casts).
pub const LAST_TRENDING_OFFSET: u32 = 900;
/// The indexer's daily share of D1's free limits (100,000 rows written and
/// 5,000,000 read a day), leaving the rest to the front page's reads.
pub const DAILY_WRITES: u64 = 50_000;
pub const DAILY_READS: u64 = 2_000_000;
/// The channels read back from the start.
pub const SEED_CHANNELS: &[&str] = &["farcaster", "base", "ethereum", "degen", "memes", "founders", "dev"];

/// The ranges ranked from the index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Span {
    Month,
    Year,
    All,
}

impl Span {
    pub const ALL: [Span; 3] = [Span::Month, Span::Year, Span::All];

    /// Its ranking's name in the index.
    pub fn name(self) -> &'static str {
        match self {
            Span::Month => "month",
            Span::Year => "year",
            Span::All => "all",
        }
    }

    pub fn parse(name: &str) -> Option<Span> {
        Span::ALL.into_iter().find(|s| s.name() == name)
    }

    pub fn of(window: TrendingWindow) -> Option<Span> {
        match window {
            TrendingWindow::Month => Some(Span::Month),
            TrendingWindow::Year => Some(Span::Year),
            TrendingWindow::All => Some(Span::All),
            TrendingWindow::Hour
            | TrendingWindow::SixHours
            | TrendingWindow::TwelveHours
            | TrendingWindow::Day
            | TrendingWindow::Week => None,
        }
    }

    /// The oldest cast time in the span at `now` (both Unix seconds).
    pub fn cutoff(self, now: i64) -> i64 {
        match self {
            Span::Month => now - 30 * DAY,
            Span::Year => now - 365 * DAY,
            Span::All => 0,
        }
    }

    /// The narrowest span a cast from `ts` is in.
    pub fn narrowest(ts: i64, now: i64) -> Span {
        Span::ALL.into_iter().find(|s| ts >= s.cutoff(now)).unwrap_or(Span::All)
    }
}

/// An indexed cast: its hash, time (Unix seconds) and points, stored as the
/// JSON array `[hash, ts, points]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry(pub String, pub i64, pub u64);

/// A span's stored ranking.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ranking {
    /// When it was computed, Unix milliseconds.
    pub built: i64,
    /// Casts indexed in the span.
    pub total: u64,
    /// The oldest of them, Unix seconds.
    pub oldest: Option<i64>,
    /// Its top casts, most points first.
    pub entries: Vec<Entry>,
}

impl Ranking {
    /// From its stored row, whose body is a JSON array of entries; `None` if
    /// the body is not one.
    pub fn from_row(built: i64, total: u64, oldest: Option<i64>, body: &str) -> Option<Ranking> {
        let mut entries: Vec<Entry> = serde_json::from_str(body).ok()?;
        entries.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)).then_with(|| a.0.cmp(&b.0)));
        Some(Ranking { built, total, oldest, entries })
    }

    /// The hashes of the `size` casts from `offset`.
    pub fn page(&self, offset: usize, size: usize) -> Vec<String> {
        self.entries.iter().skip(offset).take(size).map(|e| e.0.clone()).collect()
    }
}

/// What the indexer needs to know of a stored ranking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RankingMeta {
    pub span: Span,
    /// When it was computed, Unix milliseconds.
    pub built: i64,
    /// Casts indexed in the span.
    pub total: u64,
    /// The last ranked cast's points once the ranking is full, else 0.
    pub floor: u64,
}

fn meta(metas: &[RankingMeta], span: Span) -> Option<&RankingMeta> {
    metas.iter().find(|m| m.span == span)
}

/// The points a cast from `ts` needs to be worth indexing: enough to make
/// its narrowest span's ranking once that is full (a wider span's ranking
/// needs at least as many), and [`MIN_POINTS`] at least.
pub fn floor(metas: &[RankingMeta], ts: i64, now: i64) -> u64 {
    meta(metas, Span::narrowest(ts, now)).map_or(0, |m| m.floor).max(MIN_POINTS)
}

/// The top-level casts of a raw page that could rank or make their author
/// notable, normalized. Normalizing (text, embeds, spam rules) costs more
/// CPU than parsing, so the rest are left as they are.
pub fn candidates(raw: &[RawCast], metas: &[RankingMeta], now: i64) -> Vec<Cast> {
    raw.iter()
        .filter(|r| r.parent_hash.as_deref().is_none_or(str::is_empty))
        .filter(|r| {
            let count = |n: Option<u64>| n.unwrap_or(0);
            let reactions = r.reactions.as_ref();
            let points = count(reactions.and_then(|x| x.likes_count))
                + RECAST_WEIGHT * count(reactions.and_then(|x| x.recasts_count))
                + count(r.replies.as_ref().and_then(|x| x.count));
            let ts = r.timestamp.as_deref().and_then(parse_rfc3339).unwrap_or(0);
            may_ingest_timestamp(ts, now) && points >= floor(metas, ts, now).min(NOTABLE_POINTS)
        })
        .map(normalize::cast)
        .collect()
}

/// The casts of a page worth keeping: top-level, not spam, and with points
/// that could rank. Mini app casts are kept, for readers who show them; the
/// front page marks them and the browser leaves them out for the others.
pub fn worth_indexing(casts: &[Cast], metas: &[RankingMeta], now: i64) -> Vec<Entry> {
    let mut seen = HashSet::new();
    casts
        .iter()
        .filter(|c| !c.is_reply() && !c.spam && may_ingest_timestamp(c.ts, now) && points(c) >= floor(metas, c.ts, now))
        .filter(|c| seen.insert(c.hash.as_str()))
        .map(|c| Entry(c.hash.clone(), c.ts, points(c)))
        .collect()
}

/// The authors of a page's notable casts, best first.
pub fn notable_authors(casts: &[Cast]) -> Vec<u64> {
    let mut notable: Vec<&Cast> = casts.iter().filter(|c| !c.hidden() && points(c) >= NOTABLE_POINTS).collect();
    notable.sort_by_key(|c| std::cmp::Reverse(points(c)));
    let mut seen = HashSet::new();
    notable.into_iter().map(|c| c.author.fid).filter(|fid| *fid > 0 && seen.insert(*fid)).collect()
}

/// Each recompute reads every indexed cast in its span two or three times
/// (to rank, count and date them), so the bigger the index, the longer a
/// ranking waits: a minute per 50 indexed casts, from 5 minutes to 6 hours.
/// That keeps recomputes near 650,000 rows read a day however big it grows.
pub fn recompute_interval_ms(indexed: u64) -> f64 {
    (indexed / 50).clamp(5, 360) as f64 * 60_000.0
}

/// The ranking to recompute now, if one is due: the stalest.
pub fn due(metas: &[RankingMeta], now_ms: f64) -> Option<Span> {
    let indexed = meta(metas, Span::All).map_or(0, |m| m.total);
    let interval = recompute_interval_ms(indexed);
    Span::ALL
        .into_iter()
        .map(|span| (span, meta(metas, span).map_or(0, |m| m.built)))
        .filter(|(_, built)| now_ms - *built as f64 >= interval)
        .min_by_key(|(_, built)| *built)
        .map(|(span, _)| span)
}

/// Rows a recompute of `span` reads, about: its casts, counted and ranked.
pub fn recompute_reads(metas: &[RankingMeta], span: Span) -> u64 {
    let indexed = meta(metas, span).or_else(|| meta(metas, Span::All)).map_or(0, |m| m.total);
    3 * indexed + 1
}

/// A feed of casts read back in time.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A channel, by id.
    Channel(String),
    /// An author's casts, by FID.
    Author(u64),
}

impl Source {
    pub fn key(&self) -> String {
        match self {
            Source::Channel(id) => format!("c:{id}"),
            Source::Author(fid) => format!("a:{fid}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feed {
    pub source: Source,
    /// Where its next, older page starts; `None` before the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// What a run reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// The past week's trending casts, from this offset.
    Trending(u32),
    /// The next page of this feed.
    Backfill(usize),
    /// Nothing: today's share of D1 is used up.
    Rest,
}

/// Where the indexer is, kept as JSON in the index between runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexerState {
    #[serde(default)]
    pub runs: u64,
    #[serde(default)]
    pub trending_offset: u32,
    #[serde(default)]
    pub feeds: Vec<Feed>,
    /// The feed whose turn it is.
    #[serde(default)]
    pub turn: usize,
    /// Keys of the feeds read to their start, oldest first.
    #[serde(default)]
    pub finished: Vec<String>,
    /// The UTC day (days since the epoch) that `read` and `written` count.
    #[serde(default)]
    pub day: i64,
    /// D1 rows read and written today, as estimated from each run.
    #[serde(default)]
    pub read: u64,
    #[serde(default)]
    pub written: u64,
}

impl Default for IndexerState {
    /// A new index starts with the seed channels and finds authors as it goes.
    fn default() -> IndexerState {
        IndexerState {
            runs: 0,
            trending_offset: 0,
            feeds: SEED_CHANNELS
                .iter()
                .map(|id| Feed { source: Source::Channel((*id).to_owned()), cursor: None })
                .collect(),
            turn: 0,
            finished: Vec::new(),
            day: 0,
            read: 0,
            written: 0,
        }
    }
}

impl IndexerState {
    /// The saved state, or a new one if there is none or it can't be read.
    pub fn load(saved: Option<&str>) -> IndexerState {
        saved.and_then(|json| serde_json::from_str(json).ok()).unwrap_or_default()
    }

    /// Count `read` and `written` rows against the UTC day of `now_ms`,
    /// starting over when the day changes.
    pub fn count(&mut self, now_ms: f64, read: u64, written: u64) {
        let day = (now_ms / 86_400_000.0).floor() as i64;
        if day != self.day {
            self.day = day;
            self.read = 0;
            self.written = 0;
        }
        self.read += read;
        self.written += written;
    }

    /// Whether today's reads leave room for `rows` more.
    pub fn can_read(&self, rows: u64) -> bool {
        self.read + rows <= DAILY_READS
    }

    pub fn plan(&self) -> Step {
        if self.written >= DAILY_WRITES || self.read >= DAILY_READS {
            Step::Rest
        } else if self.feeds.is_empty() || self.runs.is_multiple_of(TRENDING_EVERY) {
            Step::Trending(self.trending_offset)
        } else {
            Step::Backfill(self.turn % self.feeds.len())
        }
    }

    /// Move on after a run that read `step`: to the next trending page, or
    /// to `next`, the feed's following page; a feed with none (or the same
    /// one again) is finished. A failed read (`Err`) moves on, and the feed
    /// tries the same page again on its next turn.
    pub fn advance(&mut self, step: &Step, next: Result<Option<String>, ()>) {
        self.runs += 1;
        match *step {
            Step::Rest => {}
            // A failed trending page waits for its next turn, 40 minutes on.
            Step::Trending(offset) => {
                self.trending_offset = if offset >= LAST_TRENDING_OFFSET { 0 } else { offset + 100 };
            }
            Step::Backfill(i) if i < self.feeds.len() => match next {
                Ok(Some(cursor)) if !cursor.is_empty() && self.feeds[i].cursor.as_ref() != Some(&cursor) => {
                    self.feeds[i].cursor = Some(cursor);
                    self.turn = i + 1;
                }
                Ok(_) => {
                    let feed = self.feeds.remove(i);
                    self.finished.push(feed.source.key());
                    let over = self.finished.len().saturating_sub(MAX_FINISHED);
                    self.finished.drain(..over);
                    self.turn = i;
                }
                Err(()) => self.turn = i + 1,
            },
            Step::Backfill(_) => self.turn = 0,
        }
    }

    /// Queue `authors` to be read back, unless already queued or finished,
    /// while there is room.
    pub fn discover(&mut self, authors: &[u64]) {
        let finished: HashSet<&str> = self.finished.iter().map(String::as_str).collect();
        let mut added = Vec::new();
        for &fid in authors {
            if self.feeds.len() + added.len() >= MAX_FEEDS {
                break;
            }
            let source = Source::Author(fid);
            if finished.contains(source.key().as_str())
                || self.feeds.iter().chain(&added).any(|f: &Feed| f.source == source)
            {
                continue;
            }
            added.push(Feed { source, cursor: None });
        }
        self.feeds.extend(added);
    }
}

/// The statements of a SQL script, without its `--` comments. D1's `exec`
/// wants one statement per line, so the Worker prepares each in turn.
pub fn sql_statements(script: &str) -> Vec<String> {
    let code: String = script.lines().map(|line| line.split("--").next().unwrap_or("")).collect::<Vec<_>>().join("\n");
    code.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::UserSummary;

    const NOW: i64 = 1_792_000_000;

    fn cast(hash: &str, fid: u64, ts: i64, likes: u64) -> Cast {
        Cast {
            hash: hash.into(),
            author: UserSummary { fid, username: format!("u{fid}"), display_name: String::new(), pfp_url: None },
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

    fn metas(month: u64, year: u64, all: u64) -> Vec<RankingMeta> {
        [(Span::Month, month), (Span::Year, year), (Span::All, all)]
            .into_iter()
            .map(|(span, floor)| RankingMeta { span, built: 0, total: 5000, floor })
            .collect()
    }

    #[test]
    fn spans_reach_back_30_days_a_year_and_forever() {
        assert_eq!(Span::of(TrendingWindow::Month), Some(Span::Month));
        assert_eq!(Span::of(TrendingWindow::Week), None);
        for span in Span::ALL {
            assert_eq!(Span::parse(span.name()), Some(span));
        }
        assert_eq!(Span::narrowest(NOW - 3 * DAY, NOW), Span::Month);
        assert_eq!(Span::narrowest(NOW - 30 * DAY, NOW), Span::Month);
        assert_eq!(Span::narrowest(NOW - 31 * DAY, NOW), Span::Year);
        assert_eq!(Span::narrowest(NOW - 400 * DAY, NOW), Span::All);
        // Clock skew: a cast from the near future is this month's.
        assert_eq!(Span::narrowest(NOW + 60, NOW), Span::Month);
    }

    #[test]
    fn a_ranking_reads_its_body_most_points_first_and_pages_through_it() {
        let body = r#"[["0xb",100,40],["0xa",200,90],["0xc",300,40]]"#;
        let ranking = Ranking::from_row(5, 3, Some(100), body).unwrap();
        assert_eq!(ranking.page(0, 2), ["0xa", "0xc"]);
        assert_eq!(ranking.page(2, 100), ["0xb"]);
        assert!(ranking.page(3, 100).is_empty());
        assert_eq!(Ranking::from_row(5, 0, None, "[]").unwrap().entries, []);
        assert_eq!(Ranking::from_row(5, 0, None, "{}"), None);
    }

    #[test]
    fn only_casts_that_could_rank_are_kept() {
        let mut reply = cast("0x2", 1, NOW, 500);
        reply.parent_hash = Some("0x1".into());
        let mut spam = cast("0x3", 1, NOW, 500);
        spam.spam = true;
        let mut app = cast("0x4", 1, NOW, 500);
        app.mini_app = true;
        let casts = [
            cast("0x1", 1, NOW - DAY, 150),
            reply,
            spam,
            app,
            cast("0x5", 1, NOW - DAY, 10),
            cast("0x6", 1, NOW - 100 * DAY, 150),
            cast("0x7", 1, NOW - 100 * DAY, 300),
            cast("0x8", 1, NOW - 900 * DAY, 300),
            cast("0x9", 1, NOW - 900 * DAY, 600),
            cast("0x1", 1, NOW - DAY, 150),
        ];
        // Empty rankings: anything with MIN_POINTS.
        let kept: Vec<String> = worth_indexing(&casts, &[], NOW).into_iter().map(|e| e.0).collect();
        assert_eq!(kept, ["0x1", "0x4", "0x6", "0x7", "0x8", "0x9"]);
        // Full rankings: each cast against its narrowest span's floor.
        let kept: Vec<String> = worth_indexing(&casts, &metas(100, 200, 500), NOW).into_iter().map(|e| e.0).collect();
        assert_eq!(kept, ["0x1", "0x4", "0x7", "0x9"]);
        assert_eq!(worth_indexing(&casts[..1], &[], NOW), [Entry("0x1".into(), NOW - DAY, 150)]);
    }

    #[test]
    fn only_casts_that_could_rank_or_make_an_author_notable_are_normalized() {
        let raw: Vec<RawCast> = serde_json::from_str(
            r#"[
                {"hash":"0x01","timestamp":"2026-10-01T00:00:00Z","reactions":{"likes_count":30,"recasts_count":1},"replies":{"count":0}},
                {"hash":"0x02","timestamp":"2026-10-01T00:00:00Z","reactions":{"likes_count":5}},
                {"hash":"0x03","parent_hash":"0x01","timestamp":"2026-10-01T00:00:00Z","reactions":{"likes_count":900}},
                {"hash":"0x04","parent_hash":"","timestamp":"2026-10-01T00:00:00Z","reactions":{"likes_count":260}},
                {"hash":"0x05","timestamp":"2026-10-01T00:00:00Z","reactions":{"likes_count":100}},
                {"hash":"0xfuture","timestamp":"2061-09-12T00:03:58Z","reactions":{"likes_count":1000000}}
            ]"#,
        )
        .unwrap();
        let now = parse_rfc3339("2026-10-02T00:00:00Z").unwrap();
        let hashes = |casts: Vec<Cast>| casts.into_iter().map(|c| c.hash).collect::<Vec<_>>();
        assert_eq!(hashes(candidates(&raw, &[], now)), ["0x01", "0x04", "0x05"]);
        // A full month: only casts over its floor, or notable ones.
        assert_eq!(hashes(candidates(&raw, &metas(150, 300, 600), now)), ["0x04"]);
        assert_eq!(hashes(candidates(&raw, &metas(400, 400, 600), now)), ["0x04"]);
    }

    #[test]
    fn only_small_clock_skew_is_worth_indexing_not_implausible_future_dates() {
        let casts = vec![
            cast("0xskew", 1, NOW + 5, 30),
            cast("0xfuture", 1, NOW + 6, 1_000_000),
            cast("0xfar", 1, 2_893_718_638, 1_000_000),
            cast("0xnow", 1, NOW, 30),
        ];
        assert_eq!(
            worth_indexing(&casts, &[], NOW),
            [Entry("0xskew".into(), NOW + 5, 30), Entry("0xnow".into(), NOW, 30)]
        );
    }

    #[test]
    fn only_small_clock_skew_is_normalized_as_an_index_candidate() {
        let raw: Vec<RawCast> = serde_json::from_str(
            r#"[
            {"hash":"0xskew","timestamp":"2026-10-02T00:00:05Z","reactions":{"likes_count":30}},
            {"hash":"0xfuture","timestamp":"2026-10-02T00:00:06Z","reactions":{"likes_count":1000000}},
            {"hash":"0xfar","timestamp":"2061-09-12T00:03:58Z","reactions":{"likes_count":1000000}}
        ]"#,
        )
        .unwrap();
        let now = parse_rfc3339("2026-10-02T00:00:00Z").unwrap();
        assert_eq!(candidates(&raw, &[], now).iter().map(|cast| cast.hash.as_str()).collect::<Vec<_>>(), ["0xskew"]);
    }

    #[test]
    fn notable_authors_come_best_first_once_each() {
        let casts =
            [cast("0x1", 7, NOW, 300), cast("0x2", 8, NOW, 900), cast("0x3", 7, NOW, 1000), cast("0x4", 9, NOW, 20)];
        assert_eq!(notable_authors(&casts), [7, 8]);
    }

    #[test]
    fn recomputes_wait_longer_as_the_index_grows() {
        assert_eq!(recompute_interval_ms(0), 300_000.0);
        assert_eq!(recompute_interval_ms(2_000), 2_400_000.0);
        assert_eq!(recompute_interval_ms(10_000_000), 21_600_000.0);
        // None stored yet: all due, month first.
        assert_eq!(due(&[], 1.0e12), Some(Span::Month));
        let at = |built: [i64; 3]| -> Vec<RankingMeta> {
            Span::ALL
                .into_iter()
                .zip(built)
                .map(|(span, built)| RankingMeta { span, built, total: 0, floor: 0 })
                .collect()
        };
        let now = 1.0e12;
        let fresh = now as i64 - 60_000;
        let stale = now as i64 - 600_000;
        assert_eq!(due(&at([fresh, fresh, fresh]), now), None);
        assert_eq!(due(&at([fresh, stale, stale - 1]), now), Some(Span::All));
        assert_eq!(due(&at([stale, fresh, fresh]), now), Some(Span::Month));
        assert_eq!(recompute_reads(&metas(0, 0, 0), Span::Year), 15_001);
    }

    #[test]
    fn runs_take_turns_between_trending_and_feeds() {
        let mut state = IndexerState::default();
        assert_eq!(state.feeds.len(), SEED_CHANNELS.len());
        assert_eq!(state.plan(), Step::Trending(0));
        state.advance(&Step::Trending(0), Ok(None));
        assert_eq!(state.trending_offset, 100);
        assert_eq!(state.plan(), Step::Backfill(0));
        state.advance(&Step::Backfill(0), Ok(Some("c1".into())));
        assert_eq!(state.feeds[0].cursor.as_deref(), Some("c1"));
        assert_eq!(state.plan(), Step::Backfill(1));
        // A failed read moves on, and the feed keeps its place.
        state.advance(&Step::Backfill(1), Err(()));
        assert_eq!(state.feeds[1].cursor, None);
        assert_eq!(state.plan(), Step::Backfill(2));
        state.advance(&Step::Backfill(2), Ok(Some("c2".into())));
        assert_eq!(state.plan(), Step::Trending(100));
        state.advance(&Step::Trending(100), Err(()));
        assert_eq!(state.trending_offset, 200);
        state.trending_offset = LAST_TRENDING_OFFSET;
        state.advance(&Step::Trending(LAST_TRENDING_OFFSET), Ok(None));
        assert_eq!(state.trending_offset, 0);
    }

    #[test]
    fn a_feed_with_no_further_page_is_finished_and_not_queued_again() {
        let mut state = IndexerState { feeds: vec![], ..IndexerState::default() };
        state.discover(&[7, 8, 7]);
        assert_eq!(
            state.feeds.iter().map(|f| f.source.clone()).collect::<Vec<_>>(),
            [Source::Author(7), Source::Author(8)]
        );
        state.advance(&Step::Backfill(0), Ok(Some("c1".into())));
        // The same cursor again would loop: finished.
        state.advance(&Step::Backfill(0), Ok(Some("c1".into())));
        assert_eq!(state.finished, ["a:7"]);
        assert_eq!(state.turn, 0);
        assert_eq!(state.plan(), Step::Backfill(0));
        state.advance(&Step::Backfill(0), Ok(None));
        assert_eq!(state.finished, ["a:7", "a:8"]);
        assert!(state.feeds.is_empty());
        state.discover(&[7, 9]);
        assert_eq!(state.feeds, [Feed { source: Source::Author(9), cursor: None }]);
        // No feeds: every run refreshes trending.
        state.feeds.clear();
        state.runs = 1;
        assert_eq!(state.plan(), Step::Trending(0));
    }

    #[test]
    fn the_queue_and_the_finished_list_are_bounded() {
        let mut state = IndexerState::default();
        let authors: Vec<u64> = (1..=500).collect();
        state.discover(&authors);
        assert_eq!(state.feeds.len(), MAX_FEEDS);
        state.finished = (0..MAX_FINISHED).map(|i| format!("a:{i}")).collect();
        let last = state.feeds.len() - 1;
        state.advance(&Step::Backfill(last), Ok(None));
        assert_eq!(state.finished.len(), MAX_FINISHED);
        assert_eq!(state.finished.first().map(String::as_str), Some("a:1"));
    }

    #[test]
    fn usage_is_counted_per_utc_day_and_rest_comes_at_the_limit() {
        let mut state = IndexerState::default();
        let day = 86_400_000.0;
        state.count(20_000.0 * day + 5.0, 10, DAILY_WRITES - 1);
        assert_ne!(state.plan(), Step::Rest);
        state.count(20_000.0 * day + 6.0, 0, 1);
        assert_eq!(state.plan(), Step::Rest);
        assert!(state.can_read(DAILY_READS - 10) && !state.can_read(DAILY_READS));
        state.count(20_001.0 * day, 3, 0);
        assert_eq!((state.read, state.written), (3, 0));
        assert_ne!(state.plan(), Step::Rest);
    }

    #[test]
    fn saved_state_round_trips_and_bad_state_starts_over() {
        let mut state = IndexerState::default();
        state.discover(&[3]);
        state.advance(&Step::Backfill(0), Ok(Some("next".into())));
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains(r#"{"source":{"channel":"farcaster"},"cursor":"next"}"#), "{json}");
        assert!(json.contains(r#"{"source":{"author":3}}"#), "{json}");
        assert_eq!(IndexerState::load(Some(&json)), state);
        assert_eq!(IndexerState::load(Some("nope")), IndexerState::default());
        assert_eq!(IndexerState::load(None), IndexerState::default());
    }

    #[test]
    fn sql_scripts_split_into_statements() {
        let script =
            "-- a comment; with a semicolon\nCREATE TABLE a (\n  x INTEGER -- note\n);\n\nCREATE INDEX b ON a (x);\n";
        assert_eq!(sql_statements(script), ["CREATE TABLE a (\n  x INTEGER \n)", "CREATE INDEX b ON a (x)"]);
    }
}
