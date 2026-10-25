// SPDX-License-Identifier: AGPL-3.0-or-later
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { request } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";

let scratch, child, port;
before(async () => {
  scratch = await mkdtemp(path.join(tmpdir(), "allthing-pages-"));
  const root = path.join(scratch, "site");
  const neighbor = path.join(scratch, "site-private");
  await Promise.all([mkdir(root), mkdir(neighbor)]);
  await Promise.all([
    writeFile(path.join(root, "index.html"), "home"),
    writeFile(path.join(root, "404.html"), "fallback"),
    writeFile(path.join(neighbor, "proof.txt"), "outside-root-fixture"),
  ]);
  child = spawn(process.execPath, [fileURLToPath(new URL("../e2e/pages-server.mjs", import.meta.url)), "0", "/Allthing", root], {
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "", errors = "";
  child.stderr.on("data", (data) => { errors += data; });
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`Server startup timed out: ${errors}`)), 5000);
    child.stdout.on("data", (data) => {
      output += data;
      const match = output.match(/127\.0\.0\.1:(\d+)\//);
      if (match) {
        port = Number(match[1]);
        clearTimeout(timer);
        resolve();
      }
    });
    child.once("error", reject);
    child.once("exit", (code) => {
      clearTimeout(timer);
      if (!port) reject(new Error(`Server exited ${code}: ${errors}`));
    });
  });
});

after(async () => {
  if (child && child.exitCode === null) {
    const exited = once(child, "exit");
    child.kill();
    await exited;
  }
  if (scratch) await rm(scratch, { recursive: true, force: true });
});

function get(urlPath) {
  return new Promise((resolve, reject) => {
    const req = request({ hostname: "127.0.0.1", port, path: urlPath }, (res) => {
      let body = "";
      res.setEncoding("utf8");
      res.on("data", (chunk) => { body += chunk; });
      res.on("end", () => resolve({ status: res.statusCode, headers: res.headers, body }));
      res.on("error", reject);
    });
    req.on("error", reject);
    req.end();
  });
}

test("serves the base, preserves redirect queries and falls back for deep links", async () => {
  assert.equal((await get("/Allthing/")).body, "home");
  const redirect = await get("/Allthing?sort=new");
  assert.equal(redirect.status, 301);
  assert.equal(redirect.headers.location, "/Allthing/?sort=new");
  const deep = await get("/Allthing/u/reader");
  assert.equal(deep.status, 404);
  assert.equal(deep.body, "fallback");
});

test("rejects malformed paths and continues serving valid requests", async () => {
  for (const urlPath of ["/Allthing/%", "/Allthing/%E0%A4%A"]) {
    assert.equal((await get(urlPath)).status, 400);
  }
  assert.equal((await get("/Allthing/?q=%")).status, 200);
  assert.equal(child.exitCode, null);
});

test("does not serve a sibling directory whose name starts with the root", async () => {
  const response = await get("/Allthing/%2e%2e%2fsite-private/proof.txt");
  assert.equal(response.status, 404);
  assert.equal(response.body, "fallback");
});
