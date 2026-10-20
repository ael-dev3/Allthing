-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Daily active accounts (allthing_core::activity): each UTC hour's
-- ("h<hours since 1970>") and day's ("d<days since 1970>") active fids, as
-- compact sets, which the cron adds to every two minutes and keeps for 49
-- hours and 30 days. Its state is in `state` ('activity') and the payload it
-- serves in `snapshots` ('activity'). Like 0001, the Worker runs it itself
-- before it first writes.
CREATE TABLE IF NOT EXISTS activity (
  name TEXT PRIMARY KEY,
  body TEXT NOT NULL
) WITHOUT ROWID;
