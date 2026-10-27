// SPDX-License-Identifier: AGPL-3.0-or-later
import test from "node:test";
import assert from "node:assert/strict";
import { buildSync } from "esbuild";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createRequire } from "node:module";

// Bundle the actual adapter and generated SDK boundary. The export exists
// only in this test build; application exports and production code stay intact.
const temp = mkdtempSync(join(tmpdir(), "allthing-stdb-bridge-"));
const previous = globalThis.allthingStdb;
buildSync({
  stdin: {
    contents: `${readFileSync(resolve("web/stdb/bridge.ts"), "utf8")}\nexport { Bridge };\n`,
    resolveDir: resolve("web/stdb"), sourcefile: "bridge-test.ts", loader: "ts",
  },
  bundle: true, platform: "node", format: "cjs", outfile: join(temp, "bridge.cjs"), logLevel: "silent",
});
const { Bridge } = createRequire(import.meta.url)(join(temp, "bridge.cjs"));
test.after(() => {
  if (previous === undefined) delete globalThis.allthingStdb;
  else globalThis.allthingStdb = previous;
  rmSync(temp, { recursive: true, force: true });
});

function fixture() {
  const subscriptions = [], events = [];
  let rows = [], inserted;
  const conn = {
    db: { myLibraryMembership: {
      iter: () => rows.values(), onInsert: (callback) => { inserted = callback; }, onDelete() {},
    } },
    subscriptionBuilder() {
      const subscription = { unsubscribeCount: 0, apply() { this.applied(); }, fail(error) { this.error({}, error); } };
      const builder = {
        onApplied(callback) { subscription.applied = callback; return this; },
        onError(callback) { subscription.error = callback; return this; },
        subscribe(queries) {
          subscription.queries = queries;
          subscriptions.push(subscription);
          return { unsubscribe() { subscription.unsubscribeCount++; } };
        },
      };
      return builder;
    },
    disconnectCount: 0, disconnect() { this.disconnectCount++; },
  };
  const bridge = new Bridge(conn, (event) => events.push(JSON.parse(event)));
  return { bridge, conn, subscriptions, events, setRows(value) { rows = value; }, insert(row) { inserted({}, row); } };
}
const query = (value) => JSON.stringify([`SELECT * FROM my_library_membership('${value}')`]);

test("a replaced snapshot's finally cannot unsubscribe its successor", async () => {
  const f = fixture();
  const first = f.bridge.snapshot("page", "myLibraryMembership", query("first"));
  const firstClosed = assert.rejects(first, /Subscription closed/);
  const second = f.bridge.snapshot("page", "myLibraryMembership", query("second"));
  await firstClosed;
  assert.equal(f.subscriptions[0].unsubscribeCount, 1);
  assert.equal(f.subscriptions[1].unsubscribeCount, 0);
  assert.deepEqual(f.subscriptions[1].queries, JSON.parse(query("second")));
  f.setRows([{ fid: 7n, seenRevision: 2n, seen: true }]);
  f.subscriptions[1].apply();
  assert.deepEqual(JSON.parse(await second), [{ fid: 7, seen_revision: 2, seen: true }]);
  assert.equal(f.subscriptions[1].unsubscribeCount, 1);
  // A late old onApplied callback cannot retire a later subscription either.
  const third = f.bridge.watch("page", query("third"));
  f.subscriptions[0].apply();
  await Promise.resolve();
  assert.equal(f.subscriptions[2].unsubscribeCount, 0);
  f.subscriptions[2].apply(); await third; f.bridge.unwatch("page");
  assert.equal(f.subscriptions[2].unsubscribeCount, 1);
});

test("explicit unwatch rejects a pending snapshot and releases its handle exactly once", async () => {
  const f = fixture();
  const pending = f.bridge.snapshot("lookup", "myLibraryMembership", query("cast"));
  const closed = assert.rejects(pending, /Subscription closed/);
  f.bridge.unwatch("lookup"); f.bridge.unwatch("lookup");
  await closed;
  f.subscriptions[0].apply();
  await Promise.resolve();
  assert.equal(f.subscriptions[0].unsubscribeCount, 1);
});

test("subscription failure releases the snapshot and a retry owns a fresh handle", async () => {
  const f = fixture();
  const failed = f.bridge.snapshot("lookup", "myLibraryMembership", query("cast"));
  const rejected = assert.rejects(failed, /denied fixture/);
  f.subscriptions[0].fail(new Error("denied fixture"));
  await rejected;
  assert.equal(f.subscriptions[0].unsubscribeCount, 1);
  const retry = f.bridge.snapshot("lookup", "myLibraryMembership", query("cast"));
  f.setRows([]); f.subscriptions[1].apply();
  assert.deepEqual(JSON.parse(await retry), []);
  assert.equal(f.subscriptions[1].unsubscribeCount, 1);
});

test("disconnect retires all named subscriptions and stops later callbacks reaching the caller", async () => {
  const f = fixture();
  const snapshot = f.bridge.snapshot("one", "myLibraryMembership", query("one"));
  const watch = f.bridge.watch("two", query("two"));
  const closed = [assert.rejects(snapshot, /Subscription closed/), assert.rejects(watch, /Subscription closed/)];
  f.bridge.disconnect();
  await Promise.all(closed);
  assert.equal(f.conn.disconnectCount, 1);
  assert.deepEqual(f.subscriptions.map((s) => s.unsubscribeCount), [1, 1]);
  f.insert({ fid: 7n, hash: "fixture" });
  f.subscriptions.forEach((s) => s.apply());
  await Promise.resolve();
  assert.deepEqual(f.events, []);
});

test("moderation snapshot and updates preserve caller rows, independent revisions and clear tombstones", () => {
  let insert, update;
  const events = [];
  const rows = [
    { key: "7:8", fid: 7n, targetFid: 8n, targetUsername: "alice.eth", muted: true, blocked: false,
      muteRevision: 2n, blockRevision: 3n, updatedAt: 100n },
    { key: "7:9", fid: 7n, targetFid: 9n, targetUsername: "bob", muted: false, blocked: false,
      muteRevision: 4n, blockRevision: 5n, updatedAt: 101n },
  ];
  const bridge = new Bridge({ db: { myAccountFilters: {
    iter: () => rows.values(), onInsert(callback) { insert = callback; }, onDelete() {},
    onUpdate(callback) { update = callback; },
  } } }, (event) => events.push(JSON.parse(event)));
  const expected = [
    { key: "7:8", fid: 7, target_fid: 8, target_username: "alice.eth", muted: true, blocked: false,
      mute_revision: 2, block_revision: 3, updated_at: 100 },
    { key: "7:9", fid: 7, target_fid: 9, target_username: "bob", muted: false, blocked: false,
      mute_revision: 4, block_revision: 5, updated_at: 101 },
  ];
  assert.deepEqual(JSON.parse(bridge.moderationState()), expected);
  insert({}, rows[0]); update({}, rows[0], rows[1]);
  assert.deepEqual(events, [
    { t: "insert", table: "my_account_filters", row: expected[0] },
    { t: "update", table: "my_account_filters", old: expected[0], row: expected[1] },
  ]);
});

test("one moderation reducer call changes only its selected field and reports conflicts", async () => {
  const calls = [];
  const conn = { db: {}, reducers: {
    async setAccountFilter(args) {
      calls.push(args);
      if (args.expectedRevision === 9n) throw new Error("account filter revision changed");
    },
  } };
  const bridge = new Bridge(conn, () => {});
  assert.deepEqual(JSON.parse(await bridge.setAccountFilter(8, "alice.eth", 0, true, 2)), { ok: true });
  assert.deepEqual(calls[0], { targetFid: 8n, targetUsername: "alice.eth", kind: 0, desired: true, expectedRevision: 2n });
  assert.equal("blocked" in calls[0], false);
  assert.equal("muted" in calls[0], false);
  assert.deepEqual(JSON.parse(await bridge.setAccountFilter(8, "alice.eth", 1, false, 9)), {
    ok: false, error: "account filter revision changed",
  });
  const invalid = JSON.parse(await bridge.setAccountFilter(Number.NaN, "", 1, true, 0));
  assert.equal(invalid.ok, false);
  assert.match(invalid.error, /NaN.*BigInt/);
  assert.equal(calls.length, 2);
});

test("snake-case moderation SDK accessors use the same caller-only boundary", async () => {
  const calls = [];
  const bridge = new Bridge({ db: { my_account_filters: {
    iter: () => [], onInsert() {}, onDelete() {},
  } }, reducers: {
    async set_account_filter(args) { calls.push(args); throw new Error("verify your Farcaster account first"); },
  } }, () => {});
  assert.deepEqual(JSON.parse(bridge.moderationState()), []);
  assert.deepEqual(JSON.parse(await bridge.setAccountFilter(8, "", 1, true, 0)), {
    ok: false, error: "verify your Farcaster account first",
  });
  assert.equal(calls.length, 1);
});
