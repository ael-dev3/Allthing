// SPDX-License-Identifier: AGPL-3.0-or-later
import { spawn, type ChildProcess } from "node:child_process";
import { createHmac, generateKeyPairSync, randomBytes, sign, type KeyObject } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer, type Server } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "@playwright/test";

/*
 * Signing in by approving Allthing's key request, on the real Worker: the
 * browser that started a sign-in gets its key endorsed, and once the
 * account has approved the key, shows it holds it (its Ed25519 signature
 * of the sign-in's message), which the Worker checks with the runtime's
 * WebCrypto, and against the account's keys as the node lists them. The
 * node is a stand-in on this machine (`HYPERSNAP_ORIGIN`), and the Worker's
 * custody key is made up for the run: nothing here is anyone's.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no Worker");
test.skip(Boolean(process.env.E2E_BASE_URL), "runs a Worker of its own");

const PORT = Number(process.env.E2E_KEYS_PORT ?? 8800);
const NODE_PORT = PORT + 1;
const ORIGIN = `http://127.0.0.1:${PORT}`;
const SECRET = "key-sign-in-end-to-end-session-secret-for-tests-only";
const ALLTHING = 3356318;
const FID = 1234;

/** An Ed25519 key pair, and its public half as the app writes it (`0x…`). */
function keyPair() {
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const raw = publicKey.export({ format: "der", type: "spki" }).subarray(-32);
  return { key: `0x${raw.toString("hex")}`, privateKey };
}
const approved = keyPair();
const otherApps = keyPair();

let worker: ChildProcess | undefined;
let node: Server | undefined;
let state = "";

test.beforeAll(async () => {
  test.setTimeout(150_000);
  // The node: the account's keys, one requested by Allthing's account and
  // one by another app; and who the account is.
  node = createServer((req, res) => {
    const url = new URL(req.url!, `http://127.0.0.1:${NODE_PORT}`);
    const json = (body: object) => res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(body));
    if (url.pathname === "/v1/signersByFid" && url.searchParams.get("fid") === String(FID)) {
      return json({ signers: [{ key: approved.key, requestFid: ALLTHING }, { key: otherApps.key, requestFid: 9152 }] });
    }
    if (url.pathname === "/v2/farcaster/user/bulk") {
      return json({ users: [{ fid: FID, username: "reader", display_name: "Reader" }] });
    }
    res.writeHead(404, { "content-type": "application/json" }).end("{}");
  }).listen(NODE_PORT, "127.0.0.1");
  state = mkdtempSync(join(tmpdir(), "allthing-keys-"));
  worker = spawn(
    "npx",
    [
      "wrangler", "dev", "--port", String(PORT), "--ip", "127.0.0.1", "--inspector-port", String(PORT + 2),
      "--persist-to", state, "--var", `SESSION_SECRET:${SECRET}`, "--var", `HYPERSNAP_ORIGIN:http://127.0.0.1:${NODE_PORT}`,
      // A custody key made up for this run, so the Worker endorses keys.
      "--var", `ALLTHING_CUSTODY_KEY:${randomBytes(32).toString("hex")}`,
    ],
    { stdio: "ignore", detached: true },
  );
  await expect
    .poll(async () => (await fetch(`${ORIGIN}/api/v1/status`).catch(() => null))?.status ?? 0, { timeout: 120_000, intervals: [1_000] })
    .toBe(200);
});

test.afterAll(async () => {
  if (worker?.pid) process.kill(-worker.pid, "SIGTERM");
  await new Promise((done) => (node ? node.close(done) : done(null)));
  if (state) rmSync(state, { recursive: true, force: true });
});

const post = (path: string, body: object, headers: Record<string, string> = {}) =>
  fetch(`${ORIGIN}${path}`, { method: "POST", headers: { "content-type": "application/json", origin: ORIGIN, ...headers }, body: JSON.stringify(body) });

/** A sign-in started here: what `start` said, and its nonce cookie. */
async function start() {
  const response = await post("/api/v1/auth/start", {});
  expect(response.status).toBe(200);
  const cookie = response.headers.getSetCookie().find((c) => c.startsWith("__Host-allthing-siwf="))!.split(";")[0];
  return { started: await response.json(), cookie };
}

const signature = (privateKey: KeyObject, message: string) => `0x${sign(null, Buffer.from(message), privateKey).toString("hex")}`;
const message = (started: { domain: string; nonce: string }) => `${started.domain} asks you to sign in with your Allthing key.\nNonce: ${started.nonce}`;

test("a browser signing in has its key endorsed, and signs in by holding the key the account approved", async () => {
  const { started, cookie } = await start();
  // The Worker endorses keys, so the sign-in is the approval itself.
  expect(started.keys).toBe(true);
  expect(started.nonce).toMatch(/^[0-9a-f]{32}$/);
  // Its key is endorsed while it signs in, with no session yet.
  const endorsed = await post("/api/v1/auth/signer", { key: approved.key }, { cookie });
  expect(endorsed.status).toBe(200);
  expect(await endorsed.json()).toMatchObject({ key: approved.key, requestFid: ALLTHING });
  // Approved in the Farcaster app (the node lists the key), it signs in.
  const signedIn = await post("/api/v1/auth/key", { fid: FID, key: approved.key, signature: signature(approved.privateKey, message(started)) }, { cookie });
  expect(signedIn.status).toBe(200);
  expect(await signedIn.json()).toEqual({ user: { fid: FID, username: "reader", display_name: "Reader" } });
  const cookies = signedIn.headers.getSetCookie();
  const session = cookies.find((c) => c.startsWith("__Host-allthing-session="))!.split(";")[0].split("=")[1];
  const [fid, expires, tag] = session.split(".");
  expect(Number(fid)).toBe(FID);
  expect(tag).toBe(createHmac("sha256", SECRET).update(`allthing session\n${fid}.${expires}`).digest("hex"));
  expect(cookies.some((c) => c.startsWith("__Host-allthing-user=") && c.includes("reader"))).toBe(true);
  // The sign-in is over: its nonce goes.
  expect(cookies.some((c) => c.startsWith("__Host-allthing-siwf=;"))).toBe(true);
  // A session gets endorsements as before.
  const later = await post("/api/v1/auth/signer", { key: approved.key }, { cookie: `__Host-allthing-session=${session}` });
  expect(later.status).toBe(200);
});

test("the Worker refuses a key sign-in it can't trust", async () => {
  const { started, cookie } = await start();
  const good = { fid: FID, key: approved.key, signature: signature(approved.privateKey, message(started)) };
  // Not from Allthing, or from a browser that didn't start a sign-in here.
  expect((await post("/api/v1/auth/key", good, { cookie, origin: "https://evil.example" })).status).toBe(403);
  expect((await post("/api/v1/auth/key", good)).status).toBe(401);
  expect((await post("/api/v1/auth/signer", { key: approved.key })).status).toBe(401);
  // A signature of something else: another nonce, another site.
  const otherNonce = { ...good, signature: signature(approved.privateKey, message({ ...started, nonce: "0".repeat(32) })) };
  expect((await post("/api/v1/auth/key", otherNonce, { cookie })).status).toBe(401);
  const otherSite = { ...good, signature: signature(approved.privateKey, message({ ...started, domain: "evil.example" })) };
  expect((await post("/api/v1/auth/key", otherSite, { cookie })).status).toBe(401);
  // Another key's signature for this key.
  const forged = { ...good, signature: signature(otherApps.privateKey, message(started)) };
  const refused = await post("/api/v1/auth/key", forged, { cookie });
  expect(refused.status).toBe(401);
  expect((await refused.json()).error).toBe("The signature doesn't match this sign-in");
  // A key of the account's that another app asked for, or one of another account.
  const otherApp = { fid: FID, key: otherApps.key, signature: signature(otherApps.privateKey, message(started)) };
  const notOurs = await post("/api/v1/auth/key", otherApp, { cookie });
  expect(notOurs.status).toBe(401);
  expect((await notOurs.json()).error).toBe("That key isn't one this account approved for Allthing");
  expect((await post("/api/v1/auth/key", { ...good, fid: 5678 }, { cookie })).status).toBe(401);
  // Malformed.
  expect((await post("/api/v1/auth/key", { fid: FID, key: "0x12", signature: good.signature }, { cookie })).status).toBe(400);
  expect((await post("/api/v1/auth/key", { fid: 0, key: approved.key, signature: good.signature }, { cookie })).status).toBe(400);
  // None of it signed anyone in.
  expect(refused.headers.getSetCookie().some((c) => c.startsWith("__Host-allthing-session="))).toBe(false);
});
