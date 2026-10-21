-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Emoji reactions (allthing_core::emoji::reaction): replies that are one
-- Pepe and nothing else, as the active-accounts follower reads them from
-- the hub's events, kept by the cast they reply to (`parent`) for the cards
-- of listings. A reply that is removed, revoked or pruned is deleted, and
-- one older than 400 days dropped. Like 0001, the Worker runs it itself
-- before it first writes.
CREATE TABLE IF NOT EXISTS reactions (
  hash TEXT PRIMARY KEY,
  parent TEXT NOT NULL,
  emoji TEXT NOT NULL,
  fid INTEGER NOT NULL,
  ts INTEGER NOT NULL
) WITHOUT ROWID;

-- A listing's casts' reactions, oldest first; and the old ones to drop.
CREATE INDEX IF NOT EXISTS reactions_by_parent ON reactions (parent, ts);
CREATE INDEX IF NOT EXISTS reactions_by_ts ON reactions (ts);
