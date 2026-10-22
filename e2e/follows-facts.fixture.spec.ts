// SPDX-License-Identifier: AGPL-3.0-or-later
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";

/*
 * What a Followers or Following page will read of the active-accounts
 * cron's D1 for its people (no page asks yet): the apps each signed with,
 * from the keys the cron learned (`signer_apps`), and whether each was
 * active in the past week, from the union of its days the cron keeps once
 * a day (`w<day>` in `activity`), kept as the days are. This runs the
 * Worker's own statements, read from crates/allthing-worker/src/index.rs,
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

/** `sql` with its parameters (?1, ?2) as literals, as D1 binds them. */
const bind = (sql: string, ...values: (string | bigint)[]) =>
  values.reduce<string>((bound, value, i) => bound.replaceAll(`?${i + 1}`, typeof value === "bigint" ? String(value) : `'${value.replace(/'/g, "''")}'`), sql);

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

/** A key as the cron keeps it: its account's fid in the high 32 bits, the key's first 4 bytes in the low. */
const pair = (fid: number, head: number) => (BigInt(fid) << 32n) | BigInt(head);

test.beforeAll(() => {
  test.setTimeout(120_000);
  state = mkdtempSync(path.join(tmpdir(), "allthing-follows-facts-"));
  for (const migration of ["0003_activity.sql", "0004_signer_apps.sql"]) {
    execFileSync("npx", ["wrangler", "d1", "execute", "allthing-index", "--local", "--persist-to", state, "--file", path.join(ROOT, "migrations", migration)], {
      cwd: ROOT,
      stdio: "ignore",
    });
  }
});

test.afterAll(() => {
  if (state) rmSync(state, { recursive: true, force: true });
});

test("each account's apps come from its keys' range, by fid, never as a key", () => {
  const last = 2 ** 31 - 1;
  const keys: [bigint, number][] = [
    // Two keys from the Farcaster app and one from Allthing.
    [pair(1, 0x0a0b0c0d), 9152],
    [pair(1, 0x00000001), 9152],
    [pair(1, 0xffffffff), 3356318],
    // A key no app requested, its account's first.
    [pair(2, 0), 0],
    // The next account's first key isn't the one before's.
    [pair(3, 0), 309857],
    // The highest fid a key is kept for.
    [pair(last, 5), 9152],
  ];
  d1(`INSERT INTO signer_apps (pair, app) VALUES ${keys.map(([p, app]) => `(${p}, ${app})`).join(", ")}`);
  const rows = d1(bind(statement("APPS_OF"), JSON.stringify([1, 2, 4, last])));
  // Fids and apps only, each pair of them once.
  for (const row of rows) expect(Object.keys(row).sort()).toEqual(["app", "fid"]);
  const found = rows.map((row) => [Number(row.fid), Number(row.app)]).sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  expect(found).toEqual([
    [1, 9152],
    [1, 3356318],
    [2, 0],
    [last, 9152],
  ]);
  expect(d1(bind(statement("APPS_OF"), "[]"))).toEqual([]);
});

test("a page reads the latest week kept, yesterday's or the day before's, in one row; old weeks go as old days do", () => {
  const today = 20_386;
  d1(`INSERT INTO activity (name, body) VALUES ('w${today - 3}', 'AQEB'), ('w${today - 2}', 'AQID'), ('d${today - 1}', 'AQ')`);
  const latest = () => d1(bind(statement("LATEST_WEEK"), `w${today - 1}`, `w${today - 2}`));
  // Before the cron's daily pass: the day before's.
  expect(latest()).toEqual([{ name: `w${today - 2}`, body: "AQID" }]);
  // After it: yesterday's.
  d1(`INSERT INTO activity (name, body) VALUES ('w${today - 1}', 'AgIC')`);
  expect(latest()).toEqual([{ name: `w${today - 1}`, body: "AgIC" }]);
  // Weeks are dropped with the days, before the 30 kept.
  d1(`INSERT INTO activity (name, body) VALUES ('w${today - 40}', 'AQ'), ('d${today - 40}', 'AQ'), ('h489000', 'AQ')`);
  d1(bind(statement("DROP_SETS"), BigInt(today * 24 - 48), BigInt(today - 30)));
  const names = d1("SELECT name FROM activity ORDER BY name").map((row) => row.name);
  expect(names).toEqual([`d${today - 1}`, `w${today - 3}`, `w${today - 2}`, `w${today - 1}`]);
});
