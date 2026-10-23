// SPDX-License-Identifier: AGPL-3.0-or-later
import test from "node:test";
import assert from "node:assert/strict";
import { buildSync } from "esbuild";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createRequire } from "node:module";

test("CSP closures match SDK BSATN bytes and decoded values, with recursive and malformed types", async () => {
  const temp = mkdtempSync(join(tmpdir(), "allthing-stdb-codecs-"));
  try {
    buildSync({ entryPoints: [resolve("tests/fixtures/stdb-codecs.ts")], bundle: true, platform: "node", format: "cjs", outfile: join(temp, "check.cjs"), logLevel: "silent" });
    const { check } = createRequire(import.meta.url)(join(temp, "check.cjs"));
    assert.equal(await check(), 11);
  } finally { rmSync(temp, { recursive: true, force: true }); }
});
