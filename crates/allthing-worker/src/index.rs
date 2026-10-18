// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing's index of notable casts in D1 (the `INDEX` binding): the SQL
//! behind `allthing_data::indexer::Index`. Rankings are computed inside D1,
//! so the Worker never handles more than one page of casts at a time.

use std::cell::Cell;
use std::rc::Rc;

use allthing_core::index::{Entry, RANKED, Ranking, RankingMeta, Span, sql_statements};
use allthing_data::indexer::{Index, IndexError, Saved};
use futures::future::{FutureExt, LocalBoxFuture};
use serde::Deserialize;
use worker::wasm_bindgen::JsValue;
use worker::{D1Database, Env};

/// The schema, from the migration that also creates it.
const SCHEMA: &str = include_str!("../../../migrations/0001_index.sql");

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
struct StateRow {
    value: String,
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

fn number(n: f64) -> JsValue {
    JsValue::from_f64(n)
}

/// Create the tables, once per isolate, before the first write.
async fn ensure_schema(db: &D1Database) -> Result<(), IndexError> {
    if SCHEMA_READY.with(Cell::get) {
        return Ok(());
    }
    let statements = sql_statements(SCHEMA).into_iter().map(|sql| db.prepare(sql)).collect();
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
}
