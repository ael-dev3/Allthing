-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Allthing's index of notable casts, which ranks Past month, Past year and
-- All time (see crates/allthing-core/src/index.rs and docs/ARCHITECTURE.md).
-- The Worker's cron job fills it. Every statement is idempotent, and the
-- Worker runs them itself before it first writes, so applying this with
-- `wrangler d1 migrations apply` is optional.

-- Casts whose points could rank: hash, Unix seconds, and points
-- (likes + 2 x recasts + replies). Keyed by hash alone, so storing a cast
-- writes one row (and its time index entry when it is new).
CREATE TABLE IF NOT EXISTS casts (
  hash TEXT PRIMARY KEY,
  ts INTEGER NOT NULL,
  points INTEGER NOT NULL
) WITHOUT ROWID;

-- Spans by time: counting a span's casts and finding its oldest.
CREATE INDEX IF NOT EXISTS casts_by_ts ON casts (ts);

-- Each span's ranking ("month", "year", "all"): its top casts as a JSON
-- array of [hash, ts, points], how many casts it was ranked from, the
-- oldest of them, and the last ranked cast's points once the ranking is
-- full (0 before), below which new casts can't rank.
CREATE TABLE IF NOT EXISTS rankings (
  name TEXT PRIMARY KEY,
  built INTEGER NOT NULL,
  total INTEGER NOT NULL,
  oldest INTEGER,
  floor INTEGER NOT NULL,
  body TEXT NOT NULL
);

-- The indexer's state between runs, as JSON.
CREATE TABLE IF NOT EXISTS state (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
