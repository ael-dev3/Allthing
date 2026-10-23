// SPDX-License-Identifier: AGPL-3.0-or-later
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("../scripts/test-browser-fixtures.sh", import.meta.url));

async function run(functional, smoothness, args = []) {
  const scratch = await mkdtemp(path.join(tmpdir(), "allthing-browser-phases-"));
  try {
    const log = path.join(scratch, "calls");
    const npx = path.join(scratch, "npx");
    await writeFile(npx, `#!/bin/sh
printf 'phase\nhtml=%s\nci=%s\n' "$PLAYWRIGHT_HTML_OUTPUT_DIR" "$CI" >> "$PHASE_LOG"
printf 'arg=%s\n' "$@" >> "$PHASE_LOG"
case "$*" in
  *--workers=2*) exit "$FUNCTIONAL_EXIT" ;;
  *--workers=1*) exit "$SMOOTHNESS_EXIT" ;;
  *) exit 90 ;;
esac
`);
    await chmod(npx, 0o755);
    const result = spawnSync("sh", [script, ...args], {
      encoding: "utf8",
      env: { ...process.env, PATH: `${scratch}:${process.env.PATH}`, CI: "1", PHASE_LOG: log, FUNCTIONAL_EXIT: String(functional), SMOOTHNESS_EXIT: String(smoothness) },
    });
    const calls = await readFile(log, "utf8").catch((error) => {
      if (error.code === "ENOENT") return "";
      throw error;
    });
    return { ...result, calls: calls.split("phase\n").filter(Boolean).map((call) => call.trim().split("\n")) };
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
}

test("runs functional then quiet smoothness with separate reports and unchanged CI policy", async () => {
  const result = await run(0, 0);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.calls.length, 2);
  assert.deepEqual(result.calls.map((call) => call.slice(0, 2)), [
    ["html=playwright-report/functional", "ci=1"],
    ["html=playwright-report/smoothness", "ci=1"],
  ]);
  assert.deepEqual(result.calls.map((call) => call.filter((arg) => arg.startsWith("arg=--"))), [
    ["arg=--workers=2", "arg=--output=test-results/functional"],
    ["arg=--workers=1", "arg=--output=test-results/smoothness"],
  ]);
});

test("still measures smoothness when functional assertions or the runner fail", async () => {
  for (const status of [1, 127]) {
    const result = await run(status, 0);
    assert.equal(result.status, 1);
    assert.equal(result.calls.length, 2);
    assert.ok(result.calls[1].includes("arg=--workers=1"));
  }
});

test("either phase failing makes the aggregate fail", async () => {
  for (const statuses of [[0, 1], [1, 1]]) {
    const result = await run(...statuses);
    assert.equal(result.status, 1);
    assert.equal(result.calls.length, 2);
  }
});

test("rejects extra CLI arguments instead of overriding the phase worker counts", async () => {
  const result = await run(0, 0, ["--workers=8"]);
  assert.equal(result.status, 2);
  assert.equal(result.calls.length, 0);
});
