// SPDX-License-Identifier: AGPL-3.0-or-later
// Fixture casts are posted "now": the front page keeps only the casts inside
// its time range (24 hours by default), so fixed past times would age out.

/** Now, in Unix seconds, when the spec file loads. */
export const NOW = Math.floor(Date.now() / 1000);

/** A time `secondsAgo` before now, as Hypersnap writes cast times. */
export const isoAgo = (secondsAgo = 0) => new Date((NOW - secondsAgo) * 1000).toISOString();
