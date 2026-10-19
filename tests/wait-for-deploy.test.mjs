// SPDX-License-Identifier: AGPL-3.0-or-later
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { gitIncludes, revisionMatches, waitForDeploy } from "../scripts/wait-for-deploy.mjs";

const expected = "a1234567890abcdef1234567890abcdef1234567";
const fast = { timeoutMs: 200, requestTimeoutMs: 40, pollIntervalMs: 10 };

async function server(t, handler) {
  const app = createServer(handler);
  await new Promise(resolve => app.listen(0, "127.0.0.1", resolve));
  t.after(async () => {
    app.closeAllConnections();
    await new Promise(resolve => app.close(resolve));
  });
  return `http://127.0.0.1:${app.address().port}`;
}

test("only a valid deployed revision matching the expected commit is accepted", () => {
  assert.equal(expected.length, 40);
  for (const actual of [expected.slice(0, 7), expected, expected.toUpperCase()]) {
    assert.equal(revisionMatches(actual, expected), true);
  }
  for (const actual of [undefined, null, 42, "dev", "", "a12345", "g123456", "b123456", `${expected}a`]) {
    assert.equal(revisionMatches(actual, expected), false);
  }
  assert.equal(revisionMatches("a123456", "dev"), false);
});

test("waits through the previous release and bypasses cached version responses", async t => {
  let reads = 0;
  const url = await server(t, (req, res) => {
    assert.equal(req.url, "/version");
    assert.equal(req.headers["cache-control"], "no-cache");
    res.setHeader("content-type", "application/json");
    res.end(JSON.stringify({ commit: ++reads === 1 ? "b123456" : expected.slice(0, 7) }));
  });
  assert.equal(await waitForDeploy(url, expected, { ...fast, timeoutMs: 1000 }), expected.slice(0, 7));
  assert.equal(reads, 2);
});

test("recovers from HTTP errors, malformed JSON, and unstamped builds", async t => {
  let reads = 0;
  const url = await server(t, (_, res) => {
    reads++;
    if (reads === 1) { res.writeHead(503); res.end("not ready"); }
    else if (reads === 2) res.end("not JSON");
    else res.end(JSON.stringify({ commit: reads === 3 ? "dev" : expected }));
  });
  assert.equal(await waitForDeploy(url, expected, { ...fast, timeoutMs: 1000 }), expected);
  assert.equal(reads, 4);
});

test("fails within the overall deadline if the intended release never arrives", async t => {
  let reads = 0;
  const url = await server(t, (_, res) => { reads++; res.end(JSON.stringify({ commit: "b123456" })); });
  const started = performance.now();
  await assert.rejects(waitForDeploy(url, expected, fast), /was not live within 0\.2s/);
  assert.ok(reads > 0);
  assert.ok(performance.now() - started < 1500);
});

for (const bodyStarted of [false, true]) {
  test(`aborts a stalled ${bodyStarted ? "response body" : "connection"} and respects the overall deadline`, async t => {
    let reads = 0;
    const url = await server(t, (_, res) => {
      reads++;
      if (bodyStarted) { res.writeHead(200); res.write('{"commit":'); }
    });
    const started = performance.now();
    await assert.rejects(waitForDeploy(url, expected, fast), /was not live within 0\.2s/);
    assert.ok(reads >= 2, "a stalled request must leave room to retry");
    assert.ok(performance.now() - started < 1500);
  });
}

test("accepts a later deployment that includes the expected commit", async t => {
  const asked = [];
  const includes = async (deployed, of) => { asked.push([deployed, of]); return deployed === "c123456"; };
  let reads = 0;
  const url = await server(t, (_, res) => {
    reads++;
    // The previous release, an unstamped build, then a later commit that includes the pushed one.
    res.end(JSON.stringify({ commit: ["b123456", "dev", "c123456"][Math.min(reads, 3) - 1] }));
  });
  assert.equal(await waitForDeploy(url, expected, { ...fast, timeoutMs: 1000, includes }), "c123456");
  assert.deepEqual(asked, [["b123456", expected], ["c123456", expected]], "only valid stamps are looked up");
});

test("a deployment that doesn't include the expected commit is still waited past", async t => {
  const url = await server(t, (_, res) => res.end(JSON.stringify({ commit: "b123456" })));
  await assert.rejects(waitForDeploy(url, expected, { ...fast, includes: async () => false }), /was not live within/);
});

test("gitIncludes follows the clone's history, fetching commits it doesn't know yet", async t => {
  const dir = mkdtempSync(join(tmpdir(), "wait-for-deploy-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const git = (cwd, ...args) =>
    execFileSync("git", ["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "init.defaultBranch=main", ...args],
      { cwd, encoding: "utf8" }).trim();
  const commit = (cwd, name) => {
    writeFileSync(join(cwd, name), name);
    git(cwd, "add", name);
    git(cwd, "commit", "-q", "-m", name);
    return git(cwd, "rev-parse", "HEAD");
  };
  const origin = join(dir, "origin.git");
  const work = join(dir, "work");
  const clone = join(dir, "clone");
  git(dir, "init", "-q", "--bare", origin);
  git(dir, "init", "-q", work);
  git(work, "remote", "add", "origin", origin);
  const old = commit(work, "a");
  const pushed = commit(work, "b");
  git(work, "push", "-q", "origin", "HEAD:main");
  git(dir, "clone", "-q", origin, clone);
  // Merged after the clone was made: a later commit on main, and one elsewhere.
  const later = commit(work, "c");
  git(work, "push", "-q", "origin", "HEAD:main");
  git(work, "checkout", "-q", "-b", "side", old);
  const side = commit(work, "d");
  git(work, "push", "-q", "origin", "side");

  const includes = gitIncludes({ cwd: clone });
  const short = sha => sha.slice(0, 7);
  assert.equal(await includes(short(later), pushed), true, "a later commit on main, fetched");
  assert.equal(await includes(short(pushed), pushed), true);
  assert.equal(await includes(short(old), pushed), false, "the previous release");
  assert.equal(await includes(short(side), pushed), false, "not on main");
  assert.equal(await includes("0000000", pushed), false, "unknown anywhere");
});

test("rejects an invalid target or duration before requesting the site", async () => {
  await assert.rejects(waitForDeploy("http://127.0.0.1:1", "dev"), /full 40-digit Git commit/);
  await assert.rejects(waitForDeploy("http://127.0.0.1:1", expected, { timeoutMs: 0 }), /positive and finite/);
  await assert.rejects(waitForDeploy("file:///tmp/site", expected), /HTTP\(S\) site URL/);
});
