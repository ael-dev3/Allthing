-- D1 allthing-index schema as of 2026-10-28 (sqlite_master dump)
CREATE TABLE casts (
  hash TEXT PRIMARY KEY,
  ts INTEGER NOT NULL,
  points INTEGER NOT NULL
) WITHOUT ROWID;

CREATE INDEX casts_by_ts ON casts (ts);

CREATE TABLE rankings (
  name TEXT PRIMARY KEY,
  built INTEGER NOT NULL,
  total INTEGER NOT NULL,
  oldest INTEGER,
  floor INTEGER NOT NULL,
  body TEXT NOT NULL
);

CREATE TABLE state (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE snapshots (
  name TEXT PRIMARY KEY,
  built INTEGER NOT NULL,
  body TEXT NOT NULL
);

CREATE TABLE activity (
  name TEXT PRIMARY KEY,
  body TEXT NOT NULL
) WITHOUT ROWID;

CREATE TABLE signer_apps (
  pair INTEGER PRIMARY KEY,
  app INTEGER NOT NULL
);

CREATE TABLE scheduled (
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

CREATE INDEX scheduled_due ON scheduled (state, at);

CREATE INDEX scheduled_fid ON scheduled (fid, at);

CREATE TABLE reactions (
  hash TEXT PRIMARY KEY,
  parent TEXT NOT NULL,
  emoji TEXT NOT NULL,
  fid INTEGER NOT NULL,
  ts INTEGER NOT NULL
) WITHOUT ROWID;

CREATE INDEX reactions_by_parent ON reactions (parent, ts);

CREATE INDEX reactions_by_ts ON reactions (ts);

CREATE TABLE media (
  key TEXT PRIMARY KEY,
  fid INTEGER NOT NULL,
  bytes INTEGER NOT NULL,
  type TEXT NOT NULL,
  created_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE INDEX media_by_fid ON media (fid, created_at, bytes);

CREATE TABLE media_total (
  name TEXT PRIMARY KEY,
  bytes INTEGER NOT NULL,
  files INTEGER NOT NULL
);

CREATE TABLE poll_votes (
  poll TEXT NOT NULL,
  voter TEXT NOT NULL,
  choice TEXT NOT NULL CHECK (choice IN ('yes', 'no')),
  PRIMARY KEY (poll, voter)
) WITHOUT ROWID;

CREATE TABLE poll_counts (
  poll TEXT NOT NULL,
  choice TEXT NOT NULL CHECK (choice IN ('yes', 'no')),
  n INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (poll, choice)
) WITHOUT ROWID;

CREATE TABLE presence (
  visitor TEXT PRIMARY KEY,
  fid INTEGER,
  seen INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TABLE media_uploads (
  id TEXT PRIMARY KEY,
  key TEXT NOT NULL UNIQUE,
  fid INTEGER NOT NULL,
  bytes INTEGER NOT NULL,
  type TEXT NOT NULL,
  hls TEXT,
  upload_id TEXT,
  mode TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'active',
  parts TEXT NOT NULL DEFAULT '[]',
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  lock_id TEXT NOT NULL DEFAULT '',
  lock_until INTEGER NOT NULL DEFAULT 0,
  cancel_requested INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;

CREATE INDEX media_uploads_by_reader ON media_uploads (fid, state, created_at);

CREATE INDEX media_uploads_expiry ON media_uploads (state, expires_at, lock_until);

CREATE TABLE media_operations (
  day INTEGER PRIMARY KEY,
  operations INTEGER NOT NULL
);

CREATE TABLE media_upload_cancellations (
  id TEXT NOT NULL,
  fid INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  PRIMARY KEY (id, fid)
) WITHOUT ROWID;

CREATE TABLE verified_signer_apps (
  identity TEXT PRIMARY KEY,
  fid INTEGER NOT NULL CHECK (fid BETWEEN 1 AND 4294967295),
  app INTEGER NOT NULL CHECK (app >= 0),
  CHECK (length(identity) = 77 AND length(CAST(identity AS BLOB)) = 77 AND substr(identity, 1, 10) = printf('%010d', fid)
    AND substr(identity, 11, 3) = ':0x'
    AND substr(identity, 14) NOT GLOB '*[^0-9a-f]*')
) WITHOUT ROWID;

CREATE INDEX verified_signer_apps_fid ON verified_signer_apps (fid, app);

