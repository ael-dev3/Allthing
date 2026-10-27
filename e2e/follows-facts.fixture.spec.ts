// SPDX-License-Identifier: AGPL-3.0-or-later
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";

/*
 * What a Followers or Following page will read of the active-accounts
 * cron's D1 for its people (no page asks yet): the apps each signed with,
 * from the full keys the verified cron learned (`verified_signer_apps`),
 * and whether each was active in the past week, from the union of its days
 * (`v2w<day>` in `activity`). Legacy rows remain outside this verified epoch.
 * This runs the Worker's own statements, read from index.rs,
 * on a local D1 made from the migrations, as wrangler runs D1.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no D1");
test.skip(Boolean(process.env.E2E_BASE_URL), "runs a local D1 of its own");

const ROOT = path.join(__dirname, "..");
const INDEX = readFileSync(path.join(ROOT, "crates/allthing-worker/src/index.rs"), "utf8");

/** The Worker's `const NAME: &str = "…";`, as the string it is. */
function statement(name: string): string {
  const found = INDEX.match(new RegExp(`const ${name}: &str = "((?:[^"\\\\]|\\\\.)*)";`, "s"));
  if (!found) throw new Error(`no ${name} in index.rs`);
  // A backslash at a line's end goes on past the next line's indentation.
  return found[1].replace(/\\\n\s*/g, "").replace(/\\"/g, '"');
}

/** `sql` with all numbered parameters as literals, including a SQL NULL. */
function bind(sql: string, ...values: (string | bigint | null)[]): string {
  return sql.replace(/\?(\d+)/g, (_, parameter: string) => {
    const index = Number(parameter) - 1;
    if (index < 0 || index >= values.length) throw new Error(`unbound SQL parameter ?${parameter}`);
    const value = values[index];
    return value === null ? "NULL" : typeof value === "bigint" ? String(value) : `'${value.replace(/'/g, "''")}'`;
  });
}

let state = "";

/** The rows of the last of `sql`'s statements, on the spec's local D1. */
function d1(sql: string): Record<string, unknown>[] {
  const out = execFileSync("npx", ["wrangler", "d1", "execute", "allthing-index", "--local", "--persist-to", state, "--json", "--command", sql], {
    cwd: ROOT,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  });
  const results = JSON.parse(out);
  return results[results.length - 1].results;
}

/** Every FID and public-key byte, in the verified counter's canonical format. */
const identity = (fid: number, head: number, tail = 0) =>
  `${String(fid).padStart(10, "0")}:0x${head.toString(16).padStart(8, "0")}${tail.toString(16).padStart(56, "0")}`;

test.beforeAll(() => {
  test.setTimeout(120_000);
  state = mkdtempSync(path.join(tmpdir(), "allthing-follows-facts-"));
  for (const migration of ["0001_index.sql", "0003_activity.sql", "0004_signer_apps.sql", "0011_verified_activity.sql"]) {
    execFileSync("npx", ["wrangler", "d1", "execute", "allthing-index", "--local", "--persist-to", state, "--file", path.join(ROOT, "migrations", migration)], {
      cwd: ROOT,
      stdio: "ignore",
    });
  }
});

test.afterAll(() => {
  if (state) rmSync(state, { recursive: true, force: true });
});

test("each account's apps come from its full signer keys, by fid, never as a key", () => {
  const aboveOldLimit = 2 ** 31;
  const last = 2 ** 32 - 1;
  const keys: [string, number][] = [
    // Full keys sharing the same first four bytes remain distinct; two
    // belong to Farcaster and the third to Allthing for this account.
    [identity(1, 0x0a0b0c0d, 1), 9152],
    [identity(1, 0x0a0b0c0d, 2), 9152],
    [identity(1, 0x0a0b0c0d, 3), 3356318],
    // Explicitly verified as no requesting app.
    [identity(2, 0, 1), 0],
    // The same public key under another FID is a separate identity.
    [identity(3, 0, 1), 309857],
    [identity(aboveOldLimit, 5), 309857],
    [identity(last, 5), 9152],
  ];
  d1(bind(statement("STORE_KEYS"), JSON.stringify(keys)));
  expect(d1("SELECT COUNT(*) AS total FROM verified_signer_apps")).toEqual([{ total: keys.length }]);
  // Legacy prefix attribution is preserved but cannot enter verified facts.
  const legacyPair = (1n << 32n) | 0x0a0b0c0dn;
  d1(`INSERT INTO signer_apps (pair, app) VALUES (${legacyPair}, 9999)`);
  const rows = d1(bind(statement("APPS_OF"), JSON.stringify([1, 2, 4, aboveOldLimit, last])));
  // Fids and apps only, each pair of them once; never expose signing keys.
  for (const row of rows) expect(Object.keys(row).sort()).toEqual(["app", "fid"]);
  const found = rows.map((row) => [Number(row.fid), Number(row.app)]).sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  expect(found).toEqual([
    [1, 9152],
    [1, 3356318],
    [2, 0],
    [aboveOldLimit, 309857],
    [last, 9152],
  ]);
  expect(d1(bind(statement("APPS_OF"), "[]"))).toEqual([]);
  expect(d1(`SELECT pair, app FROM signer_apps WHERE pair = ${legacyPair}`)).toEqual([{ pair: Number(legacyPair), app: 9999 }]);
});

test("a page reads the latest verified week; fenced expiry removes old verified sets and preserves legacy history", () => {
  const today = 20_386;
  const hourCutoff = today * 24 - 49;
  const dayCutoff = today - 30;
  d1(`INSERT INTO activity (name, body) VALUES ('v2w${today - 3}', 'AQEB'), ('v2w${today - 2}', 'AQID'), ('v2d${today - 1}', 'AQ'), ('w${today - 1}', 'AQ')`);
  const latest = () => d1(bind(statement("LATEST_WEEK"), `v2w${today - 1}`, `v2w${today - 2}`));
  // Before the daily pass, the verified day-before row wins even though a
  // newer legacy week exists. Afterwards the verified yesterday row wins.
  expect(latest()).toEqual([{ name: `v2w${today - 2}`, body: "AQID" }]);
  d1(`INSERT INTO activity (name, body) VALUES ('v2w${today - 1}', 'AgIC')`);
  expect(latest()).toEqual([{ name: `v2w${today - 1}`, body: "AgIC" }]);
  d1(`INSERT INTO activity (name, body) VALUES
    ('v2w${today - 40}', 'AQ'), ('v2d${today - 40}', 'AQ'),
    ('v2h${hourCutoff - 1}', 'AQ'), ('v2k${hourCutoff - 1}', '[]'),
    ('v2w${dayCutoff}', 'AQ'), ('v2d${dayCutoff}', 'AQ'),
    ('v2h${hourCutoff}', 'AQ'), ('v2k${hourCutoff}', '[]'),
    ('w${today - 40}', 'AQ'), ('d${today - 40}', 'AQ'), ('h489000', 'AQ'), ('k489000', 'AQ')`);
  const winning = "follows-facts-winning-fence";
  const losing = "follows-facts-losing-fence";
  expect(d1(bind(statement("CLAIM_ACTIVITY"), null, winning))).toEqual([{ value: winning }]);
  const checkpoint = JSON.stringify({ verification_version: 2, revision: 1, cursors: [], reached: [], starts: [], blocks: [], days: [] });
  d1(bind(statement("SAVE_ACTIVITY_STATE"), checkpoint, winning));
  const before = d1("SELECT name, body FROM activity ORDER BY name");
  // A stale run cannot acquire the fence or expire any rows.
  expect(d1(bind(statement("CLAIM_ACTIVITY"), null, losing))).toEqual([]);
  d1(bind(statement("DROP_SETS"), BigInt(hourCutoff), BigInt(dayCutoff), losing));
  expect(d1("SELECT name, body FROM activity ORDER BY name")).toEqual(before);
  expect(d1("SELECT value FROM state WHERE key = 'activity_v2_fence'")).toEqual([{ value: winning }]);
  // The winning run expires only older verified sets, keeping both exact
  // cutoff rows and every legacy row.
  d1(bind(statement("DROP_SETS"), BigInt(hourCutoff), BigInt(dayCutoff), winning));
  const names = d1("SELECT name FROM activity ORDER BY name").map((row) => row.name);
  expect(names).toEqual([
    `v2d${today - 1}`, `v2d${dayCutoff}`, `v2w${today - 3}`, `v2w${today - 2}`, `v2w${today - 1}`, `v2w${dayCutoff}`,
    `v2h${hourCutoff}`, `v2k${hourCutoff}`, `w${today - 1}`, `w${today - 40}`, `d${today - 40}`, "h489000", "k489000",
  ].sort());
  expect(latest()).toEqual([{ name: `v2w${today - 1}`, body: "AgIC" }]);
});
