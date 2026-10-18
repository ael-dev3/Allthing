-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Payloads the cron keeps fresh for every data center: the newest casts
-- ("latest"), as served, rebuilt every minute. A data center with no
-- recent copy of its own serves this one instead of waiting for a build
-- (see crates/allthing-data/src/pages.rs and docs/ARCHITECTURE.md). Like
-- 0001, the Worker runs it itself before it first writes.
CREATE TABLE IF NOT EXISTS snapshots (
  name TEXT PRIMARY KEY,
  built INTEGER NOT NULL,
  body TEXT NOT NULL
);
