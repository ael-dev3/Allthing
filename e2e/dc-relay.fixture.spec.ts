// SPDX-License-Identifier: AGPL-3.0-or-later
import { spawn, type ChildProcess } from "node:child_process";
import { createHmac } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer, type IncomingHttpHeaders, type Server } from "node:http";
import { createServer as createNetServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "@playwright/test";

/*
 * The Worker's Direct Cast relay (`/api/v1/dc/…`), on the real Worker. It
 * is off unless Allthing's owner sets `DC_RELAY=1`: then it passes a
 * signed-in reader's calls to the Direct Cast API with the key their
 * browser hands it, keeping nothing. The API is a stand-in on this machine
 * here (the Worker's `DC_ORIGIN`, which it honours only for a local port),
 * so the test sees what the Worker sends it: the key reaches it, and is in
 * no answer and no line the Worker logs. This spec runs a Worker of its
 * own, on ports the system picks, with the relay on; the others' Worker,
 * with it off, answers 404.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no Worker");
test.skip(Boolean(process.env.E2E_BASE_URL), "runs a Worker of its own");
// In order, on one Worker: the last test reads what it logged for the others.
test.describe.configure({ mode: "serial" });

const SECRET = "dc-relay-end-to-end-session-secret-for-tests-only";
const KEY = "wc_secret_relayFixture0123456789abcdefKEY";
const SENT_TEXT = "Through the relay, and nowhere else";
/** A message sent through the relay by hand, which no log may have. */
const RELAYED_TEXT = "Relayed once and kept nowhere, 7f3a9c";
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const now = () => Math.floor(Date.now() / 1000);
const conversationId = "636f6e7669643a31323334342d3432";

let ORIGIN = "";
let worker: ChildProcess | undefined;
let api: Server | undefined;
let state = "";
/** All the Worker printed, as it ran. */
const printed: string[] = [];
/** What the stand-in API was asked, in order. */
const asked: { method: string; url: string; headers: IncomingHttpHeaders; body: string }[] = [];
/** Every answer the relay gave this spec: its status line, headers and body. */
const answers: string[] = [];

/** A port no one is using on this machine, as the system picks one. */
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

/** The stand-in Direct Cast API: one conversation with Alice, and what's sent to it. */
function standIn() {
  const messages = [
    { conversationId, messageId: "a1", senderFid: alice.fid, message: "Hi from the stand-in", creationTimestamp: now() - 600, isDeleted: false, isProgrammatic: false, mentions: [] },
  ];
  return createServer((req, res) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => {
      const body = Buffer.concat(chunks).toString("utf8");
      asked.push({ method: req.method!, url: req.url!, headers: req.headers, body });
      const json = (status: number, value: unknown) => res.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify(value));
      if (req.headers.authorization !== `Bearer ${KEY}`) return json(401, { errors: [{ message: "Authentication required" }] });
      const url = new URL(req.url!, "http://stand-in");
      if (req.method === "GET" && url.pathname === "/fc/conversation-list") {
        const last = messages[messages.length - 1];
        return json(200, {
          result: { conversations: [{ conversationId, participantFids: [me.fid, alice.fid], creationTimestamp: now() - 86_400, lastModifiedTimestamp: last.creationTimestamp, lastMessage: last }] },
          next: {},
        });
      }
      if (req.method === "GET" && url.pathname === "/fc/group-list") return json(200, { result: { groups: [] }, next: {} });
      if (req.method === "GET" && url.pathname === "/fc/message-list") return json(200, { result: { messages: [...messages].reverse() }, next: {} });
      if (req.method === "PUT" && url.pathname === "/fc/message") {
        const sent = JSON.parse(body);
        const message = { conversationId, messageId: `m${messages.length + 1}`, senderFid: me.fid, message: sent.message, creationTimestamp: now(), isDeleted: false, isProgrammatic: true, mentions: [] };
        messages.push(message);
        return json(200, { result: { messageId: message.messageId } });
      }
      return json(404, { errors: [{ message: `Path ${url.pathname} does not exist` }] });
    });
  });
}

test.beforeAll(async () => {
  test.setTimeout(150_000);
  api = standIn();
  await new Promise<void>((done) => api!.listen(0, "127.0.0.1", done));
  const apiPort = (api.address() as { port: number }).port;
  const [port, inspector] = [await freePort(), await freePort()];
  ORIGIN = `http://127.0.0.1:${port}`;
  state = mkdtempSync(join(tmpdir(), "allthing-dc-relay-"));
  worker = spawn(
    "npx",
    [
      "wrangler", "dev", "--port", String(port), "--ip", "127.0.0.1", "--inspector-port", String(inspector),
      "--persist-to", state, "--var", `SESSION_SECRET:${SECRET}`, "--var", "DC_RELAY:1", "--var", `DC_ORIGIN:http://127.0.0.1:${apiPort}`,
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
  await new Promise((done) => (api ? api.close(done) : done(null)));
  if (state) rmSync(state, { recursive: true, force: true });
});

/** A call to this spec's relay, as a signed-in reader's browser on Allthing makes it. */
async function relay(path: string, { method = "GET", key = KEY as string | null, signedIn = true, fid = me.fid, headers = {} as Record<string, string>, body = undefined as string | undefined } = {}) {
  const sent: Record<string, string> = { "sec-fetch-site": "same-origin", ...headers };
  if (signedIn) sent.cookie = `__Host-allthing-session=${session(fid)}`;
  if (key !== null) sent["x-dc-key"] = key;
  if (method !== "GET") sent.origin ??= ORIGIN;
  if (body !== undefined) sent["content-type"] = "application/json";
  const response = await fetch(`${ORIGIN}/api/v1/dc${path}`, { method, headers: sent, body });
  const text = await response.text();
  answers.push(`${response.status}\n${[...response.headers].map(([k, v]) => `${k}: ${v}`).join("\n")}\n${text}`);
  return { status: response.status, headers: response.headers, text };
}

test("it is off unless DC_RELAY is set: the others' Worker answers 404, and the status says which", async ({ request }) => {
  // The Worker the other specs use has no DC_RELAY.
  expect((await request.get("/api/v1/dc")).status()).toBe(404);
  expect((await request.get("/api/v1/dc/conversation-list?limit=1", { headers: { "x-dc-key": KEY } })).status()).toBe(404);
  expect((await (await request.get("/api/v1/status")).json()).dc_relay).toBe(false);
  // This spec's has.
  const on = await fetch(`${ORIGIN}/api/v1/dc`);
  expect(on.status).toBe(200);
  expect(on.headers.get("cache-control")).toBe("no-store");
  expect(await on.json()).toEqual({ relay: true });
  expect((await (await fetch(`${ORIGIN}/api/v1/status`)).json()).dc_relay).toBe(true);
});

test("a signed-in reader's call goes on with their key alone, checked, and the answers never carry it", async () => {
  // Refused by the relay itself, and never sent on: no session, no key, a
  // key that isn't one, another site.
  const refused = [
    [await relay("/conversation-list?limit=1", { signedIn: false }), 401],
    [await relay("/conversation-list?limit=1", { key: null }), 400],
    [await relay("/conversation-list?limit=1", { key: "sk_live_not_farcaster" }), 400],
    [await relay("/conversation-list?limit=1", { headers: { "sec-fetch-site": "cross-site", origin: "https://elsewhere.example" } }), 403],
    // Each endpoint takes its one method, and nothing else is relayed.
    [await relay("/conversation-list", { method: "PUT", body: "{}" }), 405],
    [await relay("/message?conversationId=x"), 405],
    [await relay("/group"), 404],
    [await relay("/../v2/me"), 404],
  ] as const;
  for (const [answer, status] of refused) {
    expect(answer.status).toBe(status);
    expect(answer.headers.get("x-allthing-relay")).toBeNull();
    expect(answer.headers.get("cache-control")).toBe("no-store");
  }
  expect(asked).toEqual([]);

  // Passed on: only the endpoint's own query keys, the key as a bearer
  // token, nothing of the reader's cookies or headers; the API's answer
  // back as it was, marked as the API's, and kept nowhere.
  const listed = await relay("/conversation-list?limit=1&category=default&evil=1");
  expect(listed.status).toBe(200);
  expect(listed.headers.get("x-allthing-relay")).toBe("upstream");
  expect(listed.headers.get("cache-control")).toBe("no-store");
  expect(JSON.parse(listed.text).result.conversations[0].conversationId).toBe(conversationId);
  const call = asked.at(-1)!;
  expect(`${call.method} ${call.url}`).toBe("GET /fc/conversation-list?category=default&limit=1");
  expect(call.headers.authorization).toBe(`Bearer ${KEY}`);
  for (const name of ["cookie", "x-dc-key", "origin", "sec-fetch-site"]) expect(call.headers[name], name).toBeUndefined();

  // The API's refusals come back as its own: a 401 says the key was turned away.
  const turnedAway = await relay("/conversation-list?limit=1", { key: "wc_secret_revoked" });
  expect(turnedAway.status).toBe(401);
  expect(turnedAway.headers.get("x-allthing-relay")).toBe("upstream");

  // A send: from Allthing's own origin, at most 8 KB, the message checked,
  // and sent on as it was checked, with its idempotency key.
  const idempotency = "8f1c2e9a-7c1d-4a59-9d4e-3b1e6f0a2c11";
  const sent = await relay("/message", { method: "PUT", body: JSON.stringify({ conversationId, message: RELAYED_TEXT }), headers: { "idempotency-key": idempotency } });
  expect(sent.status).toBe(200);
  expect(sent.headers.get("x-allthing-relay")).toBe("upstream");
  expect(asked.at(-1)!.method).toBe("PUT");
  expect(asked.at(-1)!.url).toBe("/fc/message");
  expect(asked.at(-1)!.body).toBe(`{"conversationId":"${conversationId}","message":"${RELAYED_TEXT}"}`);
  expect(asked.at(-1)!.headers["idempotency-key"]).toBe(idempotency);
  const before = asked.length;
  const notSent = [
    [await relay("/message", { method: "PUT", body: JSON.stringify({ conversationId, message: "x".repeat(9_000) }) }), 413],
    [await relay("/message", { method: "PUT", body: JSON.stringify({ conversationId, message: "  " }) }), 400],
    [await relay("/message", { method: "PUT", body: JSON.stringify({ message: "to no one" }) }), 400],
    [await relay("/message", { method: "PUT", body: JSON.stringify({ conversationId, message: "hi" }), headers: { origin: "https://elsewhere.example" } }), 403],
    [await relay("/message", { method: "PUT", body: JSON.stringify({ conversationId, message: "hi" }), headers: { "idempotency-key": "not one, x" } }), 400],
  ] as const;
  for (const [answer, status] of notSent) expect(answer.status).toBe(status);
  expect(asked.length).toBe(before);

  // A reader's calls are counted, refused ones too: 30 a minute, then 429
  // from the relay. Another reader's, so the page's below are still
  // allowed, and without a key, so none goes on to the API.
  let limited: Awaited<ReturnType<typeof relay>> | undefined;
  for (let i = 0; i < 40 && !limited; i++) {
    const answer = await relay("/group-list?category=default", { fid: 5678, key: null });
    if (answer.status === 429) limited = answer;
  }
  expect(limited, "a reader's calls are limited").toBeDefined();
  expect(asked.length).toBe(before);
  expect(limited!.headers.get("x-allthing-relay")).toBeNull();
  expect(limited!.headers.get("retry-after")).toBe("60");

  // The key is in no answer: not in a header, not in a body.
  for (const answer of answers) expect(answer).not.toContain(KEY.slice("wc_secret_".length));
});

test("the Messages page reads and sends through the relay when the API won't answer the browser", async ({ page }) => {
  await page.addInitScript(
    ({ user, session, fid, key }) => {
      document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax`;
      document.cookie = `__Host-allthing-session=${session}; Path=/; Secure; SameSite=Lax`;
      const open = indexedDB.open("allthing-dc", 1);
      open.onupgradeneeded = () => open.result.createObjectStore("keys", { keyPath: "fid" });
      open.onsuccess = () => {
        open.result.transaction("keys", "readwrite").objectStore("keys").put({ fid, key });
        open.result.close();
      };
    },
    { user: me, session: session(me.fid), fid: me.fid, key: KEY },
  );
  // The Farcaster app's API refuses this origin, as it does Allthing's.
  await page.route("https://api.farcaster.xyz/**", (route) => route.abort("failed"));
  await page.route("https://haatz.quilibrium.com/v2/farcaster/user/bulk?*", (route) =>
    route.fulfill({ headers: { "access-control-allow-origin": "*" }, json: { users: [me, alice] } }),
  );
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => route.fulfill({ json: { pool: [], meta } }));
  await page.route("**/api/v1/dc**", (route) => route.continue());
  await page.goto(`${ORIGIN}/messages`);
  const row = page.getByRole("navigation", { name: "Conversations" }).getByRole("link", { name: /Alice/ });
  // Its last message is the one sent through the relay above.
  await expect(row).toContainText(`You: ${RELAYED_TEXT}`);
  await row.click();
  const thread = page.getByRole("region", { name: "Messages with Alice" });
  await expect(thread.locator(".dm-message")).toHaveCount(2);
  await expect(thread.locator(".dm-message").first()).toContainText("Hi from the stand-in");
  const box = thread.getByRole("textbox", { name: "Message @alice" });
  await box.fill(SENT_TEXT);
  await box.press("Enter");
  await expect(thread.locator(".dm-message").last()).toContainText(SENT_TEXT);
  await expect(thread.locator(".dm-outgoing")).toHaveCount(0);
  // With DC_SHOTS=<folder>, a screenshot of it.
  if (process.env.DC_SHOTS) {
    await page.evaluate(() => document.fonts.ready);
    await page.screenshot({ path: `${process.env.DC_SHOTS}/relay-thread-1280.png` });
  }
  const put = asked.filter((a) => a.method === "PUT").at(-1)!;
  expect(JSON.parse(put.body)).toEqual({ conversationId, message: SENT_TEXT });
  expect(put.headers.authorization).toBe(`Bearer ${KEY}`);
  expect(put.headers["idempotency-key"]).toMatch(/^[0-9a-f-]{36}$/);

  // Its note says the key may go through Allthing's server.
  await page.getByRole("button", { name: "Forget key" }).click();
  await expect(page.getByRole("region", { name: "Read and send your Direct Casts here" })).toContainText(
    "through Allthing's server, which passes it on with each request and keeps nothing",
  );
  if (process.env.DC_SHOTS) await page.screenshot({ path: `${process.env.DC_SHOTS}/relay-key-setup-1280.png` });
});

test("and the Worker logged neither the key nor a message", async () => {
  const secrets = [KEY.slice("wc_secret_".length), SENT_TEXT, RELAYED_TEXT];
  // What it printed: wrangler prints each request's line, the path alone
  // (in color, which is left out here).
  const all = printed.join("").replace(/\x1b\[[0-9;]*m/g, "");
  expect(all).toContain("PUT /api/v1/dc/message");
  for (const secret of secrets) expect(all).not.toContain(secret);
  // What wrangler's local observability captured of each request, as
  // Workers Logs does in Cloudflare: its method and URL, and any line the
  // Worker logged.
  const query = async (sql: string) => {
    const answer = await fetch(`${ORIGIN}/cdn-cgi/local/explorer/api/local/observability/query`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ sql }),
    }).catch(() => null);
    const read = answer?.ok ? ((await answer.json()) as { success?: boolean; result?: unknown }) : null;
    return read?.success ? JSON.stringify(read.result) : null;
  };
  const [logs, spans] = [await query("SELECT * FROM logs"), await query("SELECT * FROM spans")];
  test.info().annotations.push({ type: "observability", description: logs === null ? "not available in this wrangler" : "checked" });
  if (logs !== null && spans !== null) {
    expect(logs).toContain("/api/v1/dc/message");
    for (const secret of secrets) {
      expect(logs).not.toContain(secret);
      expect(spans).not.toContain(secret);
    }
  }
});
