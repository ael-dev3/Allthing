-- SPDX-License-Identifier: AGPL-3.0-or-later
-- Exact public signing identities for the verified activity epoch. Legacy
-- prefix-key attribution and activity rows remain intact but are not certified.
CREATE TABLE IF NOT EXISTS verified_signer_apps (
  identity TEXT PRIMARY KEY,
  fid INTEGER NOT NULL CHECK (fid BETWEEN 1 AND 4294967295),
  app INTEGER NOT NULL CHECK (app >= 0),
  CHECK (length(identity) = 77 AND length(CAST(identity AS BLOB)) = 77 AND substr(identity, 1, 10) = printf('%010d', fid)
    AND substr(identity, 11, 3) = ':0x'
    AND substr(identity, 14) NOT GLOB '*[^0-9a-f]*')
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS verified_signer_apps_fid ON verified_signer_apps (fid, app);
