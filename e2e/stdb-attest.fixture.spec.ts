// SPDX-License-Identifier: AGPL-3.0-or-later
import { spawn, type ChildProcess } from "node:child_process";
import { createHmac, createPrivateKey, createPublicKey, verify } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer as createNetServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "@playwright/test";

/*
 * `POST /api/v1/auth/attest`, on the real Worker: a signed-in reader's
 * SpacetimeDB identity, signed with the `ATTEST_KEY` secret for the
 * module's `verify_fid`. The key here is RFC 8032's first test key, the
 * one the module's own test signs with; the signature is checked with
 * Node's crypto against its public half, which `/api/v1/status` reports.
 * This spec runs a Worker of its own with the key set.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no Worker");
test.skip(Boolean(process.env.E2E_BASE_URL), "runs a Worker of its own");
test.describe.configure({ mode: "serial" });

const SECRET = "stdb-attest-end-to-end-session-secret-for-tests-only";
const SEED = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const PUBLIC = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
const IDENTITY = "c200431f50cf410304546489e2600c8446408f1aa727c91d86bc00aef6724d15";
const now = () => Math.floor(Date.now() / 1000);

let ORIGIN = "";
let worker: ChildProcess | undefined;
let state = "";
const printed: string[] = [];

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createNetServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address() as { port: number };
      server.close(() => resolve(port));
    });
  });
}

/** A session cookie as the Worker's sign-in sets it. */
function session(fid: number) {
  const value = `${fid}.${now() + 3_600}`;
  const tag = createHmac("sha256", SECRET).update(`allthing session\n${value}`).digest("hex");
  return `${value}.${tag}`;
}

test.beforeAll(async () => {
  test.setTimeout(150_000);
  const [port, inspector] = [await freePort(), await freePort()];
  ORIGIN = `http://127.0.0.1:${port}`;
  state = mkdtempSync(join(tmpdir(), "allthing-attest-"));
  worker = spawn(
    "npx",
    [
      "wrangler", "dev", "--port", String(port), "--ip", "127.0.0.1", "--inspector-port", String(inspector),
      "--persist-to", state, "--var", `SESSION_SECRET:${SECRET}`, "--var", `ATTEST_KEY:${SEED}`,
    ],
    { stdio: ["ignore", "pipe", "pipe"], detached: true },
  );
  worker.stdout!.on("data", (chunk: Buffer) => printed.push(chunk.toString("utf8")));
  worker.stderr!.on("data", (chunk: Buffer) => printed.push(chunk.toString("utf8")));
  await expect
    .poll(async () => (await fetch(`${ORIGIN}/api/v1/status`).catch(() => null))?.status ?? 0, { timeout: 120_000, intervals: [1_000] })
    .toBe(200);
});

test.afterAll(async () => {
  if (worker?.pid) process.kill(-worker.pid, "SIGTERM");
  if (state) rmSync(state, { recursive: true, force: true });
});

async function attest(body: unknown, { fid = 1234 as number | null, origin = true } = {}) {
  const headers: Record<string, string> = { "content-type": "application/json" };
  if (fid !== null) headers.cookie = `__Host-allthing-session=${session(fid)}`;
  if (origin) headers.origin = ORIGIN;
  const response = await fetch(`${ORIGIN}/api/v1/auth/attest`, { method: "POST", headers, body: JSON.stringify(body) });
  return { status: response.status, cache: response.headers.get("cache-control"), json: await response.json() };
}

test("status reports the attestation key's public half, never the key", async () => {
  const text = await (await fetch(`${ORIGIN}/api/v1/status`)).text();
  expect(JSON.parse(text).spacetime).toEqual({ attest: true, public_key: PUBLIC });
  expect(text).not.toContain(SEED);
});

test("a signed-in reader's identity is attested, for them alone", async () => {
  const before = now();
  const { status, json } = await attest({ identity: IDENTITY });
  expect(status).toBe(200);
  expect(json.identity).toBe(IDENTITY);
  expect(json.fid).toBe(1234);
  expect(json.expiry).toBeGreaterThanOrEqual(before + 600);
  expect(json.expiry).toBeLessThanOrEqual(now() + 601);
  expect(json.signature).toMatch(/^[0-9a-f]{128}$/);
  const key = createPublicKey(createPrivateKey({ key: Buffer.concat([Buffer.from("302e020100300506032b657004220420", "hex"), Buffer.from(SEED, "hex")]), format: "der", type: "pkcs8" }));
  const signed = (message: string) => verify(null, Buffer.from(message), key, Buffer.from(json.signature, "hex"));
  expect(signed(`allthing-spacetimedb-fid-v1|${IDENTITY}|1234|${json.expiry}`)).toBe(true);
  // Not for another account, identity or expiry.
  expect(signed(`allthing-spacetimedb-fid-v1|${IDENTITY}|1235|${json.expiry}`)).toBe(false);
  expect(signed(`allthing-spacetimedb-fid-v1|${IDENTITY.replace("c2", "c3")}|1234|${json.expiry}`)).toBe(false);
  expect(signed(`allthing-spacetimedb-fid-v1|${IDENTITY}|1234|${json.expiry + 1}`)).toBe(false);
});

test("it refuses anyone signed out, another site, and a malformed identity", async () => {
  expect((await attest({ identity: IDENTITY }, { fid: null })).status).toBe(401);
  expect((await attest({ identity: IDENTITY }, { origin: false })).status).toBe(403);
  for (const identity of [`0x${IDENTITY}`, IDENTITY.toUpperCase(), IDENTITY.slice(2), 42, ""]) {
    expect((await attest({ identity })).status).toBe(400);
  }
  expect((await attest({})).status).toBe(400);
  const get = await fetch(`${ORIGIN}/api/v1/auth/attest`, { headers: { cookie: `__Host-allthing-session=${session(1234)}` } });
  expect(get.status).toBe(405);
});
