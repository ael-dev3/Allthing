-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Readers' attachments (allthing_core::media): each image or video a
-- signed-in reader uploaded for a cast, which the Worker keeps in R2 (the
-- MEDIA binding) under `key`, `<fid>/<id>.<ext>`, and serves at /m/<key>:
-- whose it is, its size in bytes, the type it is served as, and when it
-- came (Unix seconds). Before each upload the Worker reads the reader's
-- past 24 hours here, their quota; once R2 has the file, it records it.
-- `media_total` holds the bytes and files stored in all, in the row 'all',
-- which the cap on storage is checked against without reading every row;
-- and those of each UTC day, in rows 'd<days since 1970>', which the cap
-- on a day's uploads is checked against.
-- Like 0001, the Worker runs it itself before it first writes.
CREATE TABLE IF NOT EXISTS media (
  key TEXT PRIMARY KEY,
  fid INTEGER NOT NULL,
  bytes INTEGER NOT NULL,
  type TEXT NOT NULL,
  created_at INTEGER NOT NULL
) WITHOUT ROWID;

-- A reader's uploads in the past 24 hours, with their sizes, so a quota
-- check reads only that reader's index entries.
CREATE INDEX IF NOT EXISTS media_by_fid ON media (fid, created_at, bytes);

CREATE TABLE IF NOT EXISTS media_total (
  name TEXT PRIMARY KEY,
  bytes INTEGER NOT NULL,
  files INTEGER NOT NULL
);
