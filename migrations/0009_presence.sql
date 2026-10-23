-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Who is reading Allthing now (allthing_core::presence): one row per
-- visitor, for the few minutes since their browser's last beat. `visitor`
-- is 'f:<fid>' for a signed-in reader shown by name (their account, from
-- the sealed session cookie), else 'a:' and the hex of an HMAC of their
-- browser's random id for the day; never an IP, a user agent or the raw
-- id. `fid` is set only for a reader shown by name, and `seen` is the
-- latest beat's Unix seconds, written at most once a minute. Rows older
-- than KEEP (1,000 s) are deleted as the count is recomputed, so the table
-- holds the last quarter of an hour at most; with no index on `seen`, a
-- beat writes one row.
-- Like 0001, the Worker runs it itself before it first writes.
CREATE TABLE IF NOT EXISTS presence (
  visitor TEXT PRIMARY KEY,
  fid INTEGER,
  seen INTEGER NOT NULL
) WITHOUT ROWID;
