// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing's index of notable casts in D1 (the `INDEX` binding): the SQL
//! behind `allthing_data::indexer::Index`. Rankings are computed inside D1,
//! so the Worker never handles more than one page of casts at a time. The
//! same database keeps the daily active accounts' sets
//! (`allthing_data::activity::ActivityStore`), and readers' scheduled casts
//! (`allthing_data::scheduled::ScheduledStore`, and `crate::scheduled_casts`).

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use allthing_core::emoji::ReactionReply;
use allthing_core::index::{Entry, RANKED, Ranking, RankingMeta, Span, sql_statements};
use allthing_core::scheduled::{Checked, ScheduledCast, State};
use allthing_data::activity::{ActivityStore, AppCount, SUMMARY, Saving};
use allthing_data::indexer::{Index, IndexError, Saved, Snapshot};
use allthing_data::scheduled::{Due, Outcome, ScheduledStore, Settled, kept_since};
use futures::future::{FutureExt, LocalBoxFuture};
use serde::Deserialize;
use worker::wasm_bindgen::JsValue;
use worker::{D1Database, Env};

/// The schema, from the migrations that also create it.
const SCHEMA: [&str; 6] = [
    include_str!("../../../migrations/0001_index.sql"),
    include_str!("../../../migrations/0002_snapshots.sql"),
    include_str!("../../../migrations/0003_activity.sql"),
    include_str!("../../../migrations/0004_signer_apps.sql"),
    include_str!("../../../migrations/0005_scheduled.sql"),
    include_str!("../../../migrations/0006_reactions.sql"),
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
/// daily ones before day ?2.
const DROP_SETS: &str = "DELETE FROM activity WHERE \
    (substr(name, 1, 1) IN ('h', 'k') AND CAST(substr(name, 2) AS INTEGER) < ?1) OR \
    (substr(name, 1, 1) = 'd' AND CAST(substr(name, 2) AS INTEGER) < ?2)";
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
/// A reader's (?1) scheduled casts: waiting, and finished since ?2, soonest
/// first.
const SCHEDULED: &str = "SELECT id, at, hash, state, error, draft FROM scheduled \
    WHERE fid = ?1 AND (state = 'waiting' OR done_at >= ?2) ORDER BY at, id";
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
/// Take one of the reader's casts off the list: one waiting (cancelled), or
/// one that failed.
const CANCEL: &str = "DELETE FROM scheduled WHERE id = ?1 AND fid = ?2 AND state <> 'posted' RETURNING id";
/// Casts due at ?1, at most ?2, the earliest first.
const DUE: &str = "SELECT id, hash, message, tries FROM scheduled \
    WHERE state = 'waiting' AND at <= ?1 ORDER BY at, id LIMIT ?2";
/// What became of a due cast, if the reader hasn't put another in its
/// place since (?2, its hash).
const POSTED: &str = "UPDATE scheduled SET state = 'posted', error = NULL, done_at = ?3 WHERE id = ?1 AND hash = ?2";
const FAILED: &str = "UPDATE scheduled SET state = 'failed', error = ?3, done_at = ?4 WHERE id = ?1 AND hash = ?2";
const AGAIN: &str = "UPDATE scheduled SET tries = tries + 1 WHERE id = ?1 AND hash = ?2";
/// Drop casts that went out, or didn't, before ?1.
const DROP_DONE: &str = "DELETE FROM scheduled WHERE state <> 'waiting' AND done_at < ?1";
const SAVE_SNAPSHOT: &str = "REPLACE INTO snapshots (name, built, body) VALUES (?1, ?2, ?3)";

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
    hash: String,
    message: String,
    tries: f64,
}

#[derive(Deserialize)]
struct IdRow {
    id: f64,
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
                    state: State::parse(&row.state)?,
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
    /// that failed off the list: whether there was one.
    pub async fn cancel(&self, fid: u64, id: i64) -> Result<bool, IndexError> {
        ensure_schema(&self.db).await?;
        let statement = self.db.prepare(CANCEL).bind(&[number(id as f64), number(fid as f64)]).map_err(failed)?;
        let row: Option<IdRow> = statement.first(None).await.map_err(failed)?;
        Ok(row.is_some())
    }
}

impl ScheduledStore for D1Index {
    fn due(&self, now: i64, limit: u32) -> LocalBoxFuture<'static, Result<Vec<Due>, IndexError>> {
        let db = self.db.clone();
        async move {
            ensure_schema(&db).await?;
            let statement = db.prepare(DUE).bind(&[number(now as f64), number(f64::from(limit))]).map_err(failed)?;
            let rows = statement.all().await.map_err(failed)?.results::<DueRow>().map_err(failed)?;
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
