-- SPDX-License-Identifier: AGPL-3.0-or-later
-- Incomplete files reserve their full eventual size before R2 accepts bytes.
-- Expiry only makes a row eligible for cleanup: it NEVER releases storage.
CREATE TABLE IF NOT EXISTS media_uploads (
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
CREATE INDEX IF NOT EXISTS media_uploads_by_reader ON media_uploads (fid, state, created_at);
CREATE INDEX IF NOT EXISTS media_uploads_expiry ON media_uploads (state, expires_at, lock_until);
-- Charged before every R2 Class A attempt, even one that fails or is retried.
CREATE TABLE IF NOT EXISTS media_operations (
  day INTEGER PRIMARY KEY,
  operations INTEGER NOT NULL
);
-- A removal that beats its create request must still cancel that request.
CREATE TABLE IF NOT EXISTS media_upload_cancellations (
  id TEXT NOT NULL,
  fid INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  PRIMARY KEY (id, fid)
) WITHOUT ROWID;
