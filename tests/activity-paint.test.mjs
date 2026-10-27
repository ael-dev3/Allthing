// SPDX-License-Identifier: AGPL-3.0-or-later
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { runInNewContext } from "node:vm";

test("early app statistics accept coherent windows and reject legacy, stale or malformed evidence", () => {
  const source = readFileSync(new URL("../web/paint.js", import.meta.url), "utf8");
  const start = source.indexOf("function verifiedAppCount(");
  const end = source.indexOf("\n// Markup.", start);
  assert(start >= 0 && end > start);
  const accepts = runInNewContext(`(${source.slice(start, end)})`, {
    Date: { now: () => 1_800_000_060_000 }, HOUR: 3600,
  });
  const valid = {
    verification_version: 2, to: 1_800_000_000, counted: 1_800_000_030,
    active: 120, unknown: 1, none: 0, apps: [{ app: 9152, active: 110 }],
  };
  assert.equal(accepts(valid), true);
  assert.equal(accepts({ ...valid, active: 4_294_967_295 }), true);
  const unnamed = { ...valid, unknown: 120 };
  delete unnamed.apps;
  assert.equal(accepts(unnamed), true, "an empty named-app list is valid evidence too");
  const defaults = { ...valid };
  delete defaults.unknown;
  delete defaults.none;
  assert.equal(accepts(defaults), true);
  for (const bad of [
    undefined, null, {}, { ...valid, verification_version: 0 },
    { ...valid, to: valid.to + 1 }, { ...valid, to: valid.to - 10800 },
    { ...valid, counted: valid.to - 1 }, { ...valid, counted: 1_800_000_061 },
    { ...valid, active: 4_294_967_296 }, { ...valid, active: -1 }, { ...valid, unknown: null },
    { ...valid, unknown: 121 }, { ...valid, none: 121 },
    { ...valid, apps: null }, { ...valid, apps: [null] },
    { ...valid, apps: [{ app: 0, active: 1 }] },
    { ...valid, apps: [{ app: 9152, active: 121 }] },
    { ...valid, apps: [{ app: 9152, active: 110 }, { app: 9152, active: 1 }] },
  ]) {
    assert.equal(accepts(bad), false, `unverified window: ${JSON.stringify(bad)}`);
  }
});
