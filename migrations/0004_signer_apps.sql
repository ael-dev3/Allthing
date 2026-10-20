-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- The apps behind daily active accounts (allthing_core::activity): the app
-- that requested each signing key active accounts signed with, so the cron
-- can count them by app. A key is `pair`, its account's fid times 2^32 plus
-- the key's first four bytes, as each hour's set of active keys
-- ("k<hours since 1970>" in `activity`) holds it. `app` is the requesting
-- app's fid, or 0 when no app requested the key; a key's app never changes,
-- so a row is written once. Like 0001, the Worker runs it itself before it
-- first writes.
CREATE TABLE IF NOT EXISTS signer_apps (
  pair INTEGER PRIMARY KEY,
  app INTEGER NOT NULL
);
