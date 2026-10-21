-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Allthing's own poll (allthing_core::poll): one vote per network.
-- `poll_votes` keeps each network's vote: `voter` is the hex HMAC-SHA256 of
-- the poll and the network (an IPv4 address, or an IPv6 address's /64),
-- keyed with the Worker's SESSION_SECRET, so the table alone can't be
-- turned back into addresses, and a network's votes in two polls can't be
-- linked; never an address, a time or a browser. `poll_counts` holds how
-- many voted each way, kept with each vote, so reading the results reads
-- two rows rather than every vote.
-- Like 0001, the Worker runs it itself before it first writes.
CREATE TABLE IF NOT EXISTS poll_votes (
  poll TEXT NOT NULL,
  voter TEXT NOT NULL,
  choice TEXT NOT NULL CHECK (choice IN ('yes', 'no')),
  PRIMARY KEY (poll, voter)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS poll_counts (
  poll TEXT NOT NULL,
  choice TEXT NOT NULL CHECK (choice IN ('yes', 'no')),
  n INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (poll, choice)
) WITHOUT ROWID;
