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
