-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Scheduled casts (allthing_core::scheduled): each reader's casts to go out
-- later, as their browser signed them with their app key. `message` is the
-- signed message in base64, which the cron gives the node at `at` (Unix
-- seconds); `hash` is the cast's hash, its link once it is out; `draft` is
-- the composer's draft, which only the reader's browser reads. `state` is
-- 'waiting', 'posted' or 'failed' (with `error`, and `done_at` for both),
-- and `tries` counts the times a node couldn't take it. Finished ones are
-- dropped after a week. Like 0001, the Worker runs it itself before it
-- first writes.
CREATE TABLE IF NOT EXISTS scheduled (
  id INTEGER PRIMARY KEY,
  fid INTEGER NOT NULL,
  at INTEGER NOT NULL,
  hash TEXT NOT NULL,
  message TEXT NOT NULL,
  draft TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'waiting',
  tries INTEGER NOT NULL DEFAULT 0,
  error TEXT,
  done_at INTEGER
);
CREATE INDEX IF NOT EXISTS scheduled_due ON scheduled (state, at);
CREATE INDEX IF NOT EXISTS scheduled_fid ON scheduled (fid, at);
