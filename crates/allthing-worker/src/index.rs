// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing's index of notable casts in D1 (the `INDEX` binding): the SQL
//! behind `allthing_data::indexer::Index`. Rankings are computed inside D1,
//! so the Worker never handles more than one page of casts at a time. The
//! same database keeps the daily active accounts' sets
//! (`allthing_data::activity::ActivityStore`), readers' scheduled casts
//! (`allthing_data::scheduled::ScheduledStore`, and `crate::scheduled_casts`),
//! what readers' attachments take (`crate::media`), the votes in
//! Allthing's poll (`crate::poll`), and who is reading now
//! (`crate::presence`).

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use allthing_core::activity::{ActiveWeek, Fids, week_set};
use allthing_core::emoji::ReactionReply;
use allthing_core::index::{Entry, RANKED, Ranking, RankingMeta, Span, sql_statements};
use allthing_core::media::{Usage, WINDOW, day_row};
use allthing_core::scheduled::{self, Cancelled, Checked, ScheduledCast, State};
use allthing_data::activity::{ActivityStore, AppCount, SUMMARY, Saving};
use allthing_data::indexer::{Index, IndexError, Saved, Snapshot};
use allthing_data::scheduled::{Due, Outcome, ScheduledStore, Settled, held_since, kept_since};
use futures::future::{FutureExt, LocalBoxFuture};
use serde::Deserialize;
use worker::wasm_bindgen::JsValue;
use worker::{D1Database, Env};

/// The schema, from the migrations that also create it.
const SCHEMA: [&str; 9] = [
    include_str!("../../../migrations/0001_index.sql"),
    include_str!("../../../migrations/0002_snapshots.sql"),
    include_str!("../../../migrations/0003_activity.sql"),
    include_str!("../../../migrations/0004_signer_apps.sql"),
    include_str!("../../../migrations/0005_scheduled.sql"),
    include_str!("../../../migrations/0006_reactions.sql"),
    include_str!("../../../migrations/0007_media.sql"),
    include_str!("../../../migrations/0008_poll.sql"),
    include_str!("../../../migrations/0009_presence.sql"),
];

/// Store casts given as one JSON array of `[hash, ts, points]`, updating the
/// points of those already indexed (and writing nothing when they are the same).
const STORE: &str = "INSERT INTO casts (hash, ts, points) \
    SELECT json_extract(value, '$[0]'), json_extract(value, '$[1]'), json_extract(value, '$[2]') \
    FROM json_each(?1) WHERE true \
    ON CONFLICT (hash) DO UPDATE SET points = excluded.points WHERE excluded.points <> casts.points";

/// Rank span ?1 (casts from ?2 on, Unix seconds) as of ?3 (Unix ms): its top
/// ?4 casts, how many it has, its oldest, and its floor once full.
const RERANK: &str = "WITH top AS (\
      SELECT hash, ts, points FROM casts WHERE ts >= ?2 ORDER BY points DESC, ts DESC LIMIT ?4) \
    REPLACE INTO rankings (name, built, total, oldest, floor, body) \
    SELECT ?1, ?3, (SELECT COUNT(*) FROM casts WHERE ts >= ?2), (SELECT MIN(ts) FROM casts WHERE ts >= ?2), \
      CASE WHEN COUNT(*) >= ?4 THEN MIN(points) ELSE 0 END, json_group_array(json_array(hash, ts, points)) \
    FROM top";

const RANKING: &str = "SELECT built, total, oldest, body FROM rankings WHERE name = ?1";
const STATE: &str = "SELECT value FROM state WHERE key = 'indexer'";
const METAS: &str = "SELECT name, built, total, floor FROM rankings";
const SAVE_STATE: &str = "REPLACE INTO state (key, value) VALUES ('indexer', ?1)";
const SNAPSHOT: &str = "SELECT built, body FROM snapshots WHERE name = ?1";
const ACTIVITY_STATE: &str = "SELECT value FROM state WHERE key = 'activity'";
const ACTIVITY_SETS: &str = "SELECT name, body FROM activity WHERE name IN (SELECT value FROM json_each(?1))";
/// Store sets given as one JSON array of `[name, body]`.
const STORE_SETS: &str = "INSERT INTO activity (name, body) \
    SELECT json_extract(value, '$[0]'), json_extract(value, '$[1]') FROM json_each(?1) WHERE true \
    ON CONFLICT (name) DO UPDATE SET body = excluded.body";
/// Drop hourly sets (of fids, `h`, and of keys, `k`) before hour ?1 and
/// daily ones (a day's, `d`, and a week's to a day, `w`) before day ?2.
const DROP_SETS: &str = "DELETE FROM activity WHERE \
    (substr(name, 1, 1) IN ('h', 'k') AND CAST(substr(name, 2) AS INTEGER) < ?1) OR \
    (substr(name, 1, 1) IN ('d', 'w') AND CAST(substr(name, 2) AS INTEGER) < ?2)";
/// Keys' apps, given as one JSON array of `[pair, app]`. A key's app never
/// changes, so a key already there is left as it is.
const STORE_KEYS: &str = "INSERT OR IGNORE INTO signer_apps (pair, app) \
    SELECT json_extract(value, '$[0]'), json_extract(value, '$[1]') FROM json_each(?1)";
/// The apps of the keys given as one JSON array. Pairs reach 2^63, past a
/// JavaScript number: they come back as text.
const KEY_APPS: &str = "SELECT CAST(pair AS TEXT) AS pair, app FROM signer_apps \
    WHERE pair IN (SELECT value FROM json_each(?1))";
/// Accounts active by app among the keys given as one JSON array, the app
/// NULL for keys not looked up yet, which come back too.
const COUNT_APPS: &str = "SELECT k.app AS app, COUNT(DISTINCT j.value >> 32) AS active, \
      json_group_array(CAST(j.value AS TEXT)) FILTER (WHERE k.pair IS NULL) AS unknown \
    FROM json_each(?1) AS j LEFT JOIN signer_apps AS k ON k.pair = j.value GROUP BY k.app";
/// The apps whose keys the accounts given as one JSON array of fids have
/// signed with: each account's keys are the pairs from `fid << 32` to
/// `fid << 32 | 0xffffffff` (the next fid's start would pass D1's integers
/// for the last fid kept). Fids and apps only: pairs are past a JavaScript
/// number.
const APPS_OF: &str = "SELECT j.value AS fid, s.app AS app FROM json_each(?1) AS j \
    JOIN signer_apps AS s ON s.pair BETWEEN (j.value << 32) AND ((j.value << 32) | 4294967295) \
    GROUP BY j.value, s.app";
/// The later of the week's sets to the ends of days ?1 and ?2 (`w<day>`),
/// if kept: one row.
const LATEST_WEEK: &str = "SELECT name, body FROM activity WHERE name IN (?1, ?2) ORDER BY name DESC LIMIT 1";
const SAVE_ACTIVITY_STATE: &str = "REPLACE INTO state (key, value) VALUES ('activity', ?1)";
/// Emoji reactions, given as one JSON array of `[parent, hash, emoji, fid,
/// ts]`; one already kept stays as it is.
const STORE_REACTIONS: &str = "INSERT OR IGNORE INTO reactions (hash, parent, emoji, fid, ts) \
    SELECT json_extract(value, '$[1]'), json_extract(value, '$[0]'), json_extract(value, '$[2]'), \
      json_extract(value, '$[3]'), json_extract(value, '$[4]') FROM json_each(?1)";
/// Drop the reactions among the casts given as one JSON array: removed,
/// revoked or pruned.
const DROP_REACTIONS: &str = "DELETE FROM reactions WHERE hash IN (SELECT value FROM json_each(?1))";
/// Drop reactions from before ?1, Unix seconds.
const OLD_REACTIONS: &str = "DELETE FROM reactions WHERE ts < ?1";
/// The reactions to the casts given as one JSON array, oldest first.
const REACTIONS: &str = "SELECT parent, hash, emoji, fid, ts FROM reactions \
    WHERE parent IN (SELECT value FROM json_each(?1)) ORDER BY ts, hash LIMIT 2000";
/// A reader's (?1) scheduled casts: waiting (or being given to the node,
/// which to them is waiting), and finished since ?2, soonest first.
const SCHEDULED: &str = "SELECT id, at, hash, state, error, draft FROM scheduled \
    WHERE fid = ?1 AND (state IN ('waiting', 'sending') OR done_at >= ?2) ORDER BY at, id";
/// Schedule a cast, unless its reader has ?6 waiting already: its id.
const SCHEDULE: &str = "INSERT INTO scheduled (fid, at, hash, message, draft) \
    SELECT ?1, ?2, ?3, ?4, ?5 WHERE (SELECT COUNT(*) FROM scheduled WHERE fid = ?1 AND state = 'waiting') < ?6 \
    RETURNING id";
/// Put a cast in the place of one of the reader's that hasn't gone out:
/// one waiting, or one that failed, while they have fewer than ?7 waiting.
const RESCHEDULE: &str = "UPDATE scheduled SET at = ?3, hash = ?4, message = ?5, draft = ?6, state = 'waiting', \
    tries = 0, error = NULL, done_at = NULL \
    WHERE id = ?1 AND fid = ?2 AND (state = 'waiting' OR (state = 'failed' AND \
      (SELECT COUNT(*) FROM scheduled WHERE fid = ?2 AND state = 'waiting') < ?7)) \
    RETURNING id";
/// The cast a cancel names (?1), as it stands: whose it is, where, and its
/// hash.
const KEPT: &str = "SELECT fid, state, hash FROM scheduled WHERE id = ?1";
/// Take one of the reader's (?2) casts off the list: one waiting
/// (cancelled), or one that failed; not one that went out, or that the node
/// is being given (`allthing_core::scheduled::cancelled`).
const CANCEL: &str = "DELETE FROM scheduled WHERE id = ?1 AND fid = ?2 AND state NOT IN ('posted', 'sending')";
/// Take the casts due at ?1 for a run to give the node, at most ?2, the
/// earliest first: those waiting whose time has come, and any a run took
/// before ?3 and never settled. Each is `sending` from ?1 (`done_at` holds
/// when it was taken) until the run settles it.
const CLAIM_DUE: &str = "UPDATE scheduled SET state = 'sending', done_at = ?1 WHERE id IN (\
      SELECT id FROM scheduled WHERE (state = 'waiting' AND at <= ?1) OR (state = 'sending' AND done_at < ?3) \
      ORDER BY at, id LIMIT ?2) \
    RETURNING id, at, hash, message, tries";
/// What became of a due cast, if the reader hasn't put another in its
/// place since (?2, its hash).
const POSTED: &str = "UPDATE scheduled SET state = 'posted', error = NULL, done_at = ?3 WHERE id = ?1 AND hash = ?2";
const FAILED: &str = "UPDATE scheduled SET state = 'failed', error = ?3, done_at = ?4 WHERE id = ?1 AND hash = ?2";
/// Waiting again, for the next minute, with a try counted.
const AGAIN: &str = "UPDATE scheduled SET state = 'waiting', tries = tries + 1, done_at = NULL \
    WHERE id = ?1 AND hash = ?2";
/// Not given to the node this run: waiting again, as it was.
const UNSENT: &str = "UPDATE scheduled SET state = 'waiting', done_at = NULL WHERE id = ?1 AND hash = ?2";
/// Drop casts that went out, or didn't, before ?1.
const DROP_DONE: &str = "DELETE FROM scheduled WHERE state IN ('posted', 'failed') AND done_at < ?1";
const SAVE_SNAPSHOT: &str = "REPLACE INTO snapshots (name, built, body) VALUES (?1, ?2, ?3)";
/// What reader ?1 has uploaded since ?2 (Unix seconds): how many files, how
/// many bytes and when the oldest came; everyone's uploads on the UTC day
/// whose row is ?3; and the bytes stored in all. Reads the reader's index
/// entries in the window, and two rows of totals.
const MEDIA_USAGE: &str = "SELECT COUNT(*) AS files, COALESCE(SUM(bytes), 0) AS bytes, MIN(created_at) AS oldest, \
      COALESCE((SELECT files FROM media_total WHERE name = ?3), 0) AS today, \
      COALESCE((SELECT bytes FROM media_total WHERE name = 'all'), 0) AS stored \
    FROM media WHERE fid = ?1 AND created_at > ?2";
/// An attachment R2 has kept; then it is added to the total (`all`) and to
/// its day's row.
const RECORD_MEDIA: &str = "INSERT INTO media (key, fid, bytes, type, created_at) VALUES (?1, ?2, ?3, ?4, ?5)";
const ADD_MEDIA: &str = "INSERT INTO media_total (name, bytes, files) VALUES (?1, ?2, 1) \
    ON CONFLICT (name) DO UPDATE SET bytes = bytes + excluded.bytes, files = files + 1";
/// Count voter ?2's vote ?3 in poll ?1, unless that voter has voted in it:
/// run before the vote is kept, in the same transaction, so a vote counts
/// once. Reads one vote's row by its key, not every vote.
const COUNT_VOTE: &str = "INSERT INTO poll_counts (poll, choice, n) \
    SELECT ?1, ?3, 1 WHERE NOT EXISTS (SELECT 1 FROM poll_votes WHERE poll = ?1 AND voter = ?2) \
    ON CONFLICT (poll, choice) DO UPDATE SET n = n + 1";
/// Keep voter ?2's vote ?3 in poll ?1, unless they have one: a row back if
/// it was kept.
const KEEP_VOTE: &str = "INSERT OR IGNORE INTO poll_votes (poll, voter, choice) VALUES (?1, ?2, ?3) RETURNING choice";
/// How many voted each way in poll ?1: two rows.
const POLL_COUNTS: &str = "SELECT choice, n FROM poll_counts WHERE poll = ?1";

/// A visitor's beat: their row, written at most once a minute (or when
/// they show or hide themselves).
const PRESENCE_BEAT: &str = "INSERT INTO presence (visitor, fid, seen) VALUES (?1, ?2, ?3) \
    ON CONFLICT(visitor) DO UPDATE SET fid = excluded.fid, seen = excluded.seen \
    WHERE presence.seen < excluded.seen - 60 OR presence.fid IS NOT excluded.fid";
/// A reader's row by name, when they hide themselves.
const PRESENCE_FORGET: &str = "DELETE FROM presence WHERE visitor = ?1";
/// Cached names must still be public in D1, including when another edge
/// location processed the reader's hide request. At most twelve key reads.
const PRESENCE_PUBLIC: &str = "SELECT fid, 0 AS online FROM presence \
    WHERE visitor IN (SELECT 'f:' || value FROM json_each(?1)) AND seen >= ?2";
/// Rows past keeping (seen before ?1).
const PRESENCE_PRUNE: &str = "DELETE FROM presence WHERE seen < ?1";
/// Who was seen since ?1, in one pass over the table: how many (`online`,
/// on each row), and ?2 rows at most, those shown by name first, most
/// recent first. A count and a list apart would read the table twice.
const PRESENCE_SEEN: &str = "SELECT fid, COUNT(*) OVER () AS online FROM presence WHERE seen >= ?1 \
    ORDER BY fid IS NULL, seen DESC LIMIT ?2";

thread_local! {
    /// Whether this isolate has made sure the tables exist.
    static SCHEMA_READY: Cell<bool> = const { Cell::new(false) };
}

#[derive(Deserialize)]
struct RankingRow {
    built: f64,
    total: f64,
    oldest: Option<f64>,
    body: String,
}

#[derive(Deserialize)]
struct SnapshotRow {
    built: f64,
    body: String,
}

#[derive(Deserialize)]
struct StateRow {
    value: String,
}

#[derive(Deserialize)]
struct SetRow {
    name: String,
    body: String,
}

#[derive(Deserialize)]
struct ReactionRow {
    parent: String,
    hash: String,
    emoji: String,
    fid: f64,
    ts: f64,
}

#[derive(Deserialize)]
struct KeyRow {
    pair: String,
    app: f64,
}

#[derive(Deserialize)]
struct AppRow {
    app: Option<f64>,
    active: f64,
    unknown: String,
}

#[derive(Deserialize)]
struct FidAppRow {
    fid: f64,
    app: f64,
}

#[derive(Deserialize)]
struct ScheduledRow {
    id: f64,
    at: f64,
    hash: String,
    state: String,
    error: Option<String>,
    draft: String,
}

#[derive(Deserialize)]
struct DueRow {
    id: f64,
    at: f64,
    hash: String,
    message: String,
    tries: f64,
}

#[derive(Deserialize)]
struct KeptRow {
    fid: f64,
    state: String,
    hash: String,
}

#[derive(Deserialize)]
struct IdRow {
    id: f64,
}

#[derive(Deserialize)]
struct UsageRow {
    files: f64,
    bytes: f64,
    oldest: Option<f64>,
    today: f64,
    stored: f64,
}

#[derive(Deserialize)]
struct CountRow {
    choice: String,
    n: f64,
}

#[derive(Deserialize)]
struct SeenRow {
    fid: Option<f64>,
    online: f64,
}

#[derive(Deserialize)]
struct MetaRow {
    name: String,
    built: f64,
    total: f64,
    floor: f64,
}

pub struct D1Index {
    db: Rc<D1Database>,
}

impl D1Index {
    /// The `INDEX` binding, if this Worker has one.
    pub fn new(env: &Env) -> Option<D1Index> {
        env.d1("INDEX").ok().map(|db| D1Index { db: Rc::new(db) })
    }
}

fn failed(error: worker::Error) -> IndexError {
    IndexError(error.to_string())
}

fn to_json(value: &impl serde::Serialize) -> Result<String, IndexError> {
    serde_json::to_string(value).map_err(|e| IndexError(e.to_string()))
}

fn number(n: f64) -> JsValue {
    JsValue::from_f64(n)
}

/// How many voted each way, from a poll's rows of counts.
fn counts(rows: Vec<CountRow>) -> allthing_core::poll::Counts {
    use allthing_core::poll::{Choice, Counts};
    let mut counts = Counts::default();
    for row in rows {
        match Choice::parse(&row.choice) {
            Some(Choice::Yes) => counts.yes = row.n as u64,
            Some(Choice::No) => counts.no = row.n as u64,
            None => {}
        }
    }
    counts
}

/// Create the tables, once per isolate, before the first write.
async fn ensure_schema(db: &D1Database) -> Result<(), IndexError> {
    if SCHEMA_READY.with(Cell::get) {
        return Ok(());
    }
    let statements = SCHEMA.iter().flat_map(|script| sql_statements(script)).map(|sql| db.prepare(sql)).collect();
    db.batch(statements).await.map_err(failed)?;
    SCHEMA_READY.with(|ready| ready.set(true));
    Ok(())
}

impl Index for D1Index {
    fn ranking(&self, span: Span) -> LocalBoxFuture<'static, Result<Option<Ranking>, IndexError>> {
        let db = self.db.clone();
        async move {
            let statement = db.prepare(RANKING).bind(&[span.name().into()]).map_err(failed)?;
            let row: Option<RankingRow> = statement.first(None).await.map_err(failed)?;
            Ok(row.and_then(|r| Ranking::from_row(r.built as i64, r.total as u64, r.oldest.map(|o| o as i64), &r.body)))
        }
        .boxed_local()
    }

    fn load(&self) -> LocalBoxFuture<'static, Result<Saved, IndexError>> {
        let db = self.db.clone();
        async move {
            ensure_schema(&db).await?;
            let results = db.batch(vec![db.prepare(STATE), db.prepare(METAS)]).await.map_err(failed)?;
            let mut results = results.into_iter();
            let state = match results.next() {
                Some(result) => result.results::<StateRow>().map_err(failed)?.into_iter().next().map(|row| row.value),
                None => None,
            };
            let metas = match results.next() {
                Some(result) => result.results::<MetaRow>().map_err(failed)?,
                None => Vec::new(),
            };
            let rankings = metas
                .into_iter()
                .filter_map(|m| {
                    Some(RankingMeta {
                        span: Span::parse(&m.name)?,
                        built: m.built as i64,
                        total: m.total as u64,
                        floor: m.floor as u64,
                    })
                })
                .collect();
            Ok(Saved { state, rankings })
        }
        .boxed_local()
    }

    fn save(
        &self,
        casts: Vec<Entry>,
        rerank: Option<Span>,
        state: String,
        now_ms: f64,
    ) -> LocalBoxFuture<'static, Result<(), IndexError>> {
        let db = self.db.clone();
        async move {
            let mut statements = Vec::new();
            if !casts.is_empty() {
                let rows = serde_json::to_string(&casts).map_err(|e| IndexError(e.to_string()))?;
                statements.push(db.prepare(STORE).bind(&[rows.into()]).map_err(failed)?);
            }
            if let Some(span) = rerank {
                let cutoff = span.cutoff((now_ms / 1000.0) as i64);
                let args = [span.name().into(), number(cutoff as f64), number(now_ms.floor()), number(RANKED as f64)];
                statements.push(db.prepare(RERANK).bind(&args).map_err(failed)?);
            }
            statements.push(db.prepare(SAVE_STATE).bind(&[state.into()]).map_err(failed)?);
            db.batch(statements).await.map_err(failed)?;
            Ok(())
        }
        .boxed_local()
    }

    /// Before the cron first stores one, there is no table yet: that reads
    /// as no copy.
    fn snapshot(&self, name: &'static str) -> LocalBoxFuture<'static, Result<Option<Snapshot>, IndexError>> {
        let db = self.db.clone();
        async move {
            let statement = db.prepare(SNAPSHOT).bind(&[name.into()]).map_err(failed)?;
            let row: Option<SnapshotRow> = statement.first(None).await.map_err(failed)?;
            Ok(row.map(|r| Snapshot { built: r.built, body: r.body }))
        }
        .boxed_local()
    }

    fn save_snapshot(&self, name: &'static str, snapshot: Snapshot) -> LocalBoxFuture<'static, Result<(), IndexError>> {
        let db = self.db.clone();
        async move {
            ensure_schema(&db).await?;
            let args = [name.into(), number(snapshot.built.floor()), snapshot.body.into()];
            db.prepare(SAVE_SNAPSHOT).bind(&args).map_err(failed)?.run().await.map_err(failed)?;
            Ok(())
        }
        .boxed_local()
    }

    /// Fids of 2^31 or more have no keys kept (`activity::key_pair`), and
    /// would take the pairs' range past D1's integers: they are left out.
    fn apps_of(&self, fids: &[u64]) -> LocalBoxFuture<'static, Result<Vec<(u64, u64)>, IndexError>> {
        let db = self.db.clone();
        let fids: Vec<u64> = fids.iter().copied().filter(|&fid| fid < 1 << 31).collect();
        async move {
            if fids.is_empty() {
                return Ok(Vec::new());
            }
            let statement = db.prepare(APPS_OF).bind(&[to_json(&fids)?.into()]).map_err(failed)?;
            let rows = statement.all().await.map_err(failed)?.results::<FidAppRow>().map_err(failed)?;
            Ok(rows.into_iter().map(|row| (row.fid as u64, row.app as u64)).collect())
        }
        .boxed_local()
    }

    /// Yesterday's week, or until the cron's daily pass has kept it, the
    /// day before's: one row of about 2 bytes an account, decoded once and
    /// asked about each of `fids` (`Fids::contains`).
    fn active_week(&self, fids: &[u64], today: i64) -> LocalBoxFuture<'static, Result<Option<ActiveWeek>, IndexError>> {
        let db = self.db.clone();
        let fids = fids.to_vec();
        async move {
            let names: [JsValue; 2] = [week_set(today - 1).into(), week_set(today - 2).into()];
            let statement = db.prepare(LATEST_WEEK).bind(&names).map_err(failed)?;
            let Some(row) = statement.first::<SetRow>(None).await.map_err(failed)? else { return Ok(None) };
            let day = row.name.strip_prefix('w').and_then(|day| day.parse().ok());
            let week = Fids::decode(&row.body);
            match (day, week) {
                (Some(day), Some(week)) => Ok(Some(ActiveWeek::among(day, &week, &fids))),
                _ => Err(IndexError(format!("{} isn't a week's set", row.name))),
            }
        }
        .boxed_local()
    }

    /// Before the follower first keeps one, there is no table yet: the read
    /// fails, and the listing keeps the reactions it had.
    fn reactions(
        &self,
        casts: Vec<String>,
    ) -> LocalBoxFuture<'static, Result<Vec<(String, ReactionReply)>, IndexError>> {
        let db = self.db.clone();
        async move {
            let casts = serde_json::to_string(&casts).map_err(|e| IndexError(e.to_string()))?;
            let statement = db.prepare(REACTIONS).bind(&[casts.into()]).map_err(failed)?;
            let rows = statement.all().await.map_err(failed)?.results::<ReactionRow>().map_err(failed)?;
            Ok(rows
                .into_iter()
                .map(|r| (r.parent, ReactionReply { hash: r.hash, emoji: r.emoji, fid: r.fid as u64, ts: r.ts as i64 }))
                .collect())
        }
        .boxed_local()
    }

    /// Where no write has made the poll's tables yet, the read fails, and
    /// the front page goes without the counts, as it does before anyone
    /// has voted.
    fn poll(&self, poll: &'static str) -> LocalBoxFuture<'static, Result<allthing_core::poll::Counts, IndexError>> {
        let db = self.db.clone();
        async move {
            let statement = db.prepare(POLL_COUNTS).bind(&[poll.into()]).map_err(failed)?;
            Ok(counts(statement.all().await.map_err(failed)?.results::<CountRow>().map_err(failed)?))
        }
        .boxed_local()
    }
}

/// The votes in Allthing's poll, for `crate::poll`.
impl D1Index {
    /// Count and keep `voter`'s vote `choice` in `poll`, unless that voter
    /// has voted in it, in one transaction: whether it was counted, and how
    /// many voted each way.
    pub async fn vote(
        &self,
        poll: &str,
        voter: &str,
        choice: allthing_core::poll::Choice,
    ) -> Result<(bool, allthing_core::poll::Counts), IndexError> {
        ensure_schema(&self.db).await?;
        let args: [JsValue; 3] = [poll.into(), voter.into(), choice.as_str().into()];
        let count = self.db.prepare(COUNT_VOTE).bind(&args).map_err(failed)?;
        let keep = self.db.prepare(KEEP_VOTE).bind(&args).map_err(failed)?;
        let read = self.db.prepare(POLL_COUNTS).bind(&[poll.into()]).map_err(failed)?;
        let mut results = self.db.batch(vec![count, keep, read]).await.map_err(failed)?.into_iter().skip(1);
        let kept = match results.next() {
            Some(kept) => !kept.results::<serde_json::Value>().map_err(failed)?.is_empty(),
            None => false,
        };
        let rows = match results.next() {
            Some(read) => read.results::<CountRow>().map_err(failed)?,
            None => Vec::new(),
        };
        Ok((kept, counts(rows)))
    }
}

impl ActivityStore for D1Index {
    fn activity_state(&self) -> LocalBoxFuture<'static, Result<Option<String>, IndexError>> {
        let db = self.db.clone();
        async move {
            ensure_schema(&db).await?;
            let row: Option<StateRow> = db.prepare(ACTIVITY_STATE).first(None).await.map_err(failed)?;
            Ok(row.map(|r| r.value))
        }
        .boxed_local()
    }

    fn activity_sets(
        &self,
        names: Vec<String>,
    ) -> LocalBoxFuture<'static, Result<HashMap<String, String>, IndexError>> {
        let db = self.db.clone();
        async move {
            let names = serde_json::to_string(&names).map_err(|e| IndexError(e.to_string()))?;
            let statement = db.prepare(ACTIVITY_SETS).bind(&[names.into()]).map_err(failed)?;
            let rows = statement.all().await.map_err(failed)?.results::<SetRow>().map_err(failed)?;
            Ok(rows.into_iter().map(|r| (r.name, r.body)).collect())
        }
        .boxed_local()
    }

    fn key_apps(&self, pairs: Vec<u64>) -> LocalBoxFuture<'static, Result<HashMap<u64, u64>, IndexError>> {
        let db = self.db.clone();
        async move {
            let pairs = serde_json::to_string(&pairs).map_err(|e| IndexError(e.to_string()))?;
            let statement = db.prepare(KEY_APPS).bind(&[pairs.into()]).map_err(failed)?;
            let rows = statement.all().await.map_err(failed)?.results::<KeyRow>().map_err(failed)?;
            Ok(rows.into_iter().filter_map(|r| Some((r.pair.parse().ok()?, r.app as u64))).collect())
        }
        .boxed_local()
    }

    fn count_apps(
        &self,
        keys: Vec<(u64, u64)>,
        chunks: Vec<Vec<u64>>,
    ) -> LocalBoxFuture<'static, Result<Vec<AppCount>, IndexError>> {
        let db = self.db.clone();
        async move {
            let mut statements = Vec::new();
            if !keys.is_empty() {
                statements.push(db.prepare(STORE_KEYS).bind(&[to_json(&keys)?.into()]).map_err(failed)?);
            }
            let stored = statements.len();
            for chunk in &chunks {
                statements.push(db.prepare(COUNT_APPS).bind(&[to_json(chunk)?.into()]).map_err(failed)?);
            }
            let mut out = Vec::new();
            if statements.is_empty() {
                return Ok(out);
            }
            for result in db.batch(statements).await.map_err(failed)?.into_iter().skip(stored) {
                for row in result.results::<AppRow>().map_err(failed)? {
                    let unknown: Vec<String> = serde_json::from_str(&row.unknown).unwrap_or_default();
                    let unknown = unknown.iter().filter_map(|p| p.parse().ok()).collect();
                    out.push((row.app.map(|app| app as u64), row.active as u64, unknown));
                }
            }
            Ok(out)
        }
        .boxed_local()
    }

    fn save_activity(&self, saving: Saving) -> LocalBoxFuture<'static, Result<(), IndexError>> {
        let db = self.db.clone();
        async move {
            let Saving { sets, keep_from, keys, reactions, removed, reactions_from, state, summary } = saving;
            let mut statements = Vec::new();
            if !sets.is_empty() {
                let rows = serde_json::to_string(&sets).map_err(|e| IndexError(e.to_string()))?;
                statements.push(db.prepare(STORE_SETS).bind(&[rows.into()]).map_err(failed)?);
            }
            if !keys.is_empty() {
                let rows = serde_json::to_string(&keys).map_err(|e| IndexError(e.to_string()))?;
                statements.push(db.prepare(STORE_KEYS).bind(&[rows.into()]).map_err(failed)?);
            }
            let keep = [number(keep_from.0 as f64), number(keep_from.1 as f64)];
            statements.push(db.prepare(DROP_SETS).bind(&keep).map_err(failed)?);
            if !reactions.is_empty() {
                let rows: Vec<_> =
                    reactions.into_iter().map(|(cast, r)| (cast, r.hash, r.emoji, r.fid, r.ts)).collect();
                let rows = serde_json::to_string(&rows).map_err(|e| IndexError(e.to_string()))?;
                statements.push(db.prepare(STORE_REACTIONS).bind(&[rows.into()]).map_err(failed)?);
            }
            if !removed.is_empty() {
                let hashes = serde_json::to_string(&removed).map_err(|e| IndexError(e.to_string()))?;
                statements.push(db.prepare(DROP_REACTIONS).bind(&[hashes.into()]).map_err(failed)?);
            }
            statements.push(db.prepare(OLD_REACTIONS).bind(&[number(reactions_from as f64)]).map_err(failed)?);
            statements.push(db.prepare(SAVE_ACTIVITY_STATE).bind(&[state.into()]).map_err(failed)?);
            let args = [SUMMARY.into(), number(summary.built.floor()), summary.body.into()];
            statements.push(db.prepare(SAVE_SNAPSHOT).bind(&args).map_err(failed)?);
            db.batch(statements).await.map_err(failed)?;
            Ok(())
        }
        .boxed_local()
    }
}

/// A signed-in reader's scheduled casts, for `crate::scheduled_casts`.
impl D1Index {
    /// The reader's casts waiting, and those finished in the past week.
    pub async fn scheduled(&self, fid: u64, now: i64) -> Result<Vec<ScheduledCast>, IndexError> {
        ensure_schema(&self.db).await?;
        let args = [number(fid as f64), number(kept_since(now) as f64)];
        let statement = self.db.prepare(SCHEDULED).bind(&args).map_err(failed)?;
        let rows = statement.all().await.map_err(failed)?.results::<ScheduledRow>().map_err(failed)?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                Some(ScheduledCast {
                    id: row.id as i64,
                    at: row.at as i64,
                    hash: row.hash,
                    state: State::listed(&row.state)?,
                    error: row.error,
                    draft: row.draft,
                })
            })
            .collect())
    }

    /// Keep a checked cast for the reader, or put it in the place of one of
    /// theirs (`replaces`): its id, or `None` when they have `max` waiting
    /// already, or the one to replace has gone out or isn't theirs.
    pub async fn schedule(
        &self,
        fid: u64,
        checked: &Checked,
        message: &str,
        draft: &str,
        replaces: Option<i64>,
        max: u32,
    ) -> Result<Option<i64>, IndexError> {
        ensure_schema(&self.db).await?;
        let (fid, at, max) = (number(fid as f64), number(checked.at as f64), number(f64::from(max)));
        let (hash, message, draft) = (checked.hash.as_str().into(), message.into(), draft.into());
        let statement = match replaces {
            Some(id) => self.db.prepare(RESCHEDULE).bind(&[number(id as f64), fid, at, hash, message, draft, max]),
            None => self.db.prepare(SCHEDULE).bind(&[fid, at, hash, message, draft, max]),
        }
        .map_err(failed)?;
        let row: Option<IdRow> = statement.first(None).await.map_err(failed)?;
        Ok(row.map(|row| row.id as i64))
    }

    /// Cancel one of the reader's casts that hasn't gone out, or take one
    /// that failed off the list: what became of it, or `None` when it is
    /// someone else's. What it reads and what it deletes are one batch, a
    /// transaction, so the answer is what the delete found.
    pub async fn cancel(&self, fid: u64, id: i64) -> Result<Option<Cancelled>, IndexError> {
        ensure_schema(&self.db).await?;
        let read = self.db.prepare(KEPT).bind(&[number(id as f64)]).map_err(failed)?;
        let delete = self.db.prepare(CANCEL).bind(&[number(id as f64), number(fid as f64)]).map_err(failed)?;
        let results = self.db.batch(vec![read, delete]).await.map_err(failed)?;
        let kept = match results.into_iter().next() {
            Some(result) => result.results::<KeptRow>().map_err(failed)?.into_iter().next(),
            None => None,
        };
        Ok(scheduled::cancelled(fid, kept.as_ref().map(|row| (row.fid as u64, row.state.as_str(), row.hash.as_str()))))
    }
}

/// What readers' attachments take, for `crate::media`.
impl D1Index {
    /// What `fid` has uploaded in the 24 hours to `now` (Unix seconds),
    /// what everyone has today, and what is stored in all.
    pub async fn media_usage(&self, fid: u64, now: i64) -> Result<Usage, IndexError> {
        ensure_schema(&self.db).await?;
        let args = [number(fid as f64), number((now - WINDOW) as f64), day_row(now).into()];
        let statement = self.db.prepare(MEDIA_USAGE).bind(&args).map_err(failed)?;
        let row: Option<UsageRow> = statement.first(None).await.map_err(failed)?;
        Ok(row.map_or_else(Usage::default, |row| Usage {
            files: row.files as u32,
            bytes: row.bytes as u64,
            oldest: row.oldest.map(|at| at as i64),
            today: row.today as u32,
            stored: row.stored as u64,
        }))
    }

    /// Record an attachment that R2 has kept, and add it to the total and
    /// to its day's count, in one transaction.
    pub async fn record_media(&self, key: &str, fid: u64, bytes: u64, mime: &str, now: i64) -> Result<(), IndexError> {
        let size = number(bytes as f64);
        let args = [key.into(), number(fid as f64), size.clone(), mime.into(), number(now as f64)];
        let record = self.db.prepare(RECORD_MEDIA).bind(&args).map_err(failed)?;
        let total = self.db.prepare(ADD_MEDIA).bind(&["all".into(), size.clone()]).map_err(failed)?;
        let day = self.db.prepare(ADD_MEDIA).bind(&[day_row(now).into(), size]).map_err(failed)?;
        self.db.batch(vec![record, total, day]).await.map_err(failed)?;
        Ok(())
    }
}

/// Who is reading now, for `crate::presence`.
impl D1Index {
    /// Record `visitor`'s beat at `now` (Unix seconds), as `fid` when they
    /// are shown by name, in one batch, which first drops `forget`, the
    /// previous named or anonymous row when visibility changes. With `recount`, the
    /// window's start and the oldest second kept, the same batch also drops
    /// rows past keeping and counts who was seen in the window, with the
    /// readers shown by name (`SHOWN` at most, most recent first): two
    /// passes over the table, which has no index on `seen`.
    pub async fn presence(
        &self,
        visitor: &str,
        fid: Option<u64>,
        forget: Option<&str>,
        now: i64,
        recount: Option<(i64, i64)>,
        cached_fids: &[u64],
    ) -> Result<Option<(u64, Vec<u64>)>, IndexError> {
        ensure_schema(&self.db).await?;
        let mut statements = Vec::with_capacity(5);
        if let Some(forget) = forget {
            statements.push(self.db.prepare(PRESENCE_FORGET).bind(&[forget.into()]).map_err(failed)?);
        }
        let fid = fid.map_or(JsValue::NULL, |fid| number(fid as f64));
        statements
            .push(self.db.prepare(PRESENCE_BEAT).bind(&[visitor.into(), fid, number(now as f64)]).map_err(failed)?);
        let Some((since, kept)) = recount else {
            let position = statements.len();
            statements.push(
                self.db
                    .prepare(PRESENCE_PUBLIC)
                    .bind(&[
                        serde_json::to_string(cached_fids).map_err(|err| IndexError(err.to_string()))?.into(),
                        number(
                            (now - i64::from(allthing_core::presence::online_window(allthing_core::presence::MAX_NEXT)))
                                as f64,
                        ),
                    ])
                    .map_err(failed)?,
            );
            let results = self.db.batch(statements).await.map_err(failed)?;
            let public = results[position].results::<SeenRow>().map_err(failed)?;
            return Ok(Some((0, public.into_iter().filter_map(|row| row.fid).map(|fid| fid as u64).collect())));
        };
        let written = statements.len() + 1;
        statements.push(self.db.prepare(PRESENCE_PRUNE).bind(&[number(kept as f64)]).map_err(failed)?);
        statements.push(
            self.db
                .prepare(PRESENCE_SEEN)
                .bind(&[number(since as f64), number(allthing_core::presence::SHOWN as f64)])
                .map_err(failed)?,
        );
        let seen = match self.db.batch(statements).await.map_err(failed)?.into_iter().nth(written) {
            Some(result) => result.results::<SeenRow>().map_err(failed)?,
            None => Vec::new(),
        };
        let online = seen.first().map_or(0, |row| row.online as u64);
        let fids = seen.iter().filter_map(|row| row.fid).map(|fid| fid as u64).collect();
        Ok(Some((online, fids)))
    }

    /// Sign-out still has the authenticated session, so it can remove the
    /// named row before clearing that session. Cached responses recheck it.
    pub async fn forget_presence(&self, fid: u64) -> Result<(), IndexError> {
        ensure_schema(&self.db).await?;
        self.db
            .prepare(PRESENCE_FORGET)
            .bind(&[allthing_core::presence::named_visitor(fid).into()])
            .map_err(failed)?
            .run()
            .await
            .map_err(failed)?;
        Ok(())
    }
}

impl ScheduledStore for D1Index {
    fn claim_due(&self, now: i64, limit: u32) -> LocalBoxFuture<'static, Result<Vec<Due>, IndexError>> {
        let db = self.db.clone();
        async move {
            ensure_schema(&db).await?;
            let args = [number(now as f64), number(f64::from(limit)), number(held_since(now) as f64)];
            let statement = db.prepare(CLAIM_DUE).bind(&args).map_err(failed)?;
            let mut rows = statement.all().await.map_err(failed)?.results::<DueRow>().map_err(failed)?;
            // RETURNING keeps no order: the earliest first, as they were picked.
            rows.sort_by(|a, b| a.at.total_cmp(&b.at).then(a.id.total_cmp(&b.id)));
            Ok(rows
                .into_iter()
                .map(|row| Due { id: row.id as i64, hash: row.hash, message: row.message, tries: row.tries as u32 })
                .collect())
        }
        .boxed_local()
    }

    fn settle(&self, settled: Vec<Settled>, now: i64) -> LocalBoxFuture<'static, Result<(), IndexError>> {
        let db = self.db.clone();
        async move {
            let mut statements = Vec::new();
            for Settled { id, hash, outcome } in settled {
                let (id, hash) = (number(id as f64), JsValue::from(hash));
                let statement = match outcome {
                    Outcome::Posted => db.prepare(POSTED).bind(&[id, hash, number(now as f64)]),
                    Outcome::Failed(why) => db.prepare(FAILED).bind(&[id, hash, why.into(), number(now as f64)]),
                    Outcome::Again => db.prepare(AGAIN).bind(&[id, hash]),
                    Outcome::Unsent => db.prepare(UNSENT).bind(&[id, hash]),
                };
                statements.push(statement.map_err(failed)?);
            }
            statements.push(db.prepare(DROP_DONE).bind(&[number(kept_since(now) as f64)]).map_err(failed)?);
            db.batch(statements).await.map_err(failed)?;
            Ok(())
        }
        .boxed_local()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schema_is_every_migration_in_order() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.ends_with(".sql"))
            .collect();
        names.sort();
        assert_eq!(names.len(), SCHEMA.len(), "{names:?}");
        for (name, script) in names.iter().zip(SCHEMA) {
            assert_eq!(std::fs::read_to_string(format!("{dir}/{name}")).unwrap(), script, "{name}");
            // Each isolate runs them all again before it first writes.
            for statement in sql_statements(script) {
                let again = ["CREATE TABLE IF NOT EXISTS ", "CREATE INDEX IF NOT EXISTS "];
                assert!(again.iter().any(|start| statement.starts_with(start)), "{name}: {statement}");
            }
        }
    }
}
