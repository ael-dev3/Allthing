// SPDX-License-Identifier: AGPL-3.0-or-later
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { test } from "node:test";
import { revisionMatches, waitForDeploy } from "../scripts/wait-for-deploy.mjs";

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

test("rejects an invalid target or duration before requesting the site", async () => {
  await assert.rejects(waitForDeploy("http://127.0.0.1:1", "dev"), /full 40-digit Git commit/);
  await assert.rejects(waitForDeploy("http://127.0.0.1:1", expected, { timeoutMs: 0 }), /positive and finite/);
  await assert.rejects(waitForDeploy("file:///tmp/site", expected), /HTTP\(S\) site URL/);
});
