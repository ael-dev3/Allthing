// SPDX-License-Identifier: AGPL-3.0-or-later
import { spawn, type ChildProcess } from "node:child_process";
import { createHmac } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer, type Server } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { approvedKey, cast as castOf, envelope } from "./helpers/farcaster";

/*
 * A scheduled cast going out at its minute, end to end, on the real Worker:
 * the browser signs it with the reader's key and hands it to the Worker,
 * which keeps it in D1; at its minute the cron gives the node exactly what
 * the browser signed, and the reader's list says what the node answered. A
 * cancel from a list read before that minute finds the cast gone out, and
 * the Worker's cancel says what it did: taken off, the same when asked
 * again, too late, or not the reader's to cancel.
 * The node is a stand-in on this machine (the Worker's `HYPERSNAP_ORIGIN`),
 * which takes one cast and refuses the other's key, as the network does a
 * key that isn't on the account. This spec runs a Worker of its own, with
 * an empty D1 and a session secret, so the others' Worker is left as it is.
 * It waits for the minute to come, a minute or two.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no Worker");
test.skip(Boolean(process.env.E2E_BASE_URL), "runs a Worker of its own");
test.use({ timezoneId: "Europe/Berlin", locale: "en-GB" });
// The second test reads what the first left in D1.
test.describe.configure({ mode: "serial" });

const PORT = Number(process.env.E2E_CRON_PORT ?? 8796);
const NODE_PORT = PORT + 1;
const ORIGIN = `http://127.0.0.1:${PORT}`;
const SECRET = "scheduled-cron-end-to-end-session-secret-for-tests-only";
const EPOCH = 1_609_459_200;
const NODE = "https://haatz.quilibrium.com/v1";
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" };
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const post = { hash: `0x${"1".repeat(40)}`, author: { fid: 42, username: "alice" }, text: "What should I read next?", ts: 1792235681, likes: 3, replies: 0 };
const now = () => Math.floor(Date.now() / 1000);

let worker: ChildProcess | undefined;
let node: Server | undefined;
let state = "";
/** What the stand-in node was given, in base64, in order. */
const given: string[] = [];
/** Every request the stand-in had, as `<method> <path>`. */
const asked: string[] = [];

/** A session cookie as the Worker's sign-in sets it: `<fid>.<expiry>.<HMAC-SHA256>`. */
function session(fid: number) {
  const value = `${fid}.${now() + 3_600}`;
  const tag = createHmac("sha256", SECRET).update(`allthing session\n${value}`).digest("hex");
  return `${value}.${tag}`;
}

test.beforeAll(async () => {
  test.setTimeout(150_000);
  // The node: takes the cast that says so, refuses the other's key; nothing
  // else of it is wanted (the cron's indexer reads it too, and gets nothing).
  node = createServer((req, res) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => {
      asked.push(`${req.method} ${req.url}`);
      if (req.method !== "POST" || req.url !== "/v1/submitMessage") {
        res.writeHead(404, { "content-type": "application/json" }).end("{}");
        return;
      }
      const body = Buffer.concat(chunks);
      given.push(body.toString("base64"));
      const refused = castOf(envelope(body)).text.includes("refused");
      res
        .writeHead(refused ? 400 : 200, { "content-type": "application/json" })
        .end(JSON.stringify(refused ? { errCode: "bad_request.validation_failure", error: "bad_request.validation_failure/invalid signer" } : {}));
    });
  }).listen(NODE_PORT, "127.0.0.1");
  state = mkdtempSync(join(tmpdir(), "allthing-cron-"));
  worker = spawn(
    "npx",
    [
      "wrangler", "dev", "--port", String(PORT), "--ip", "127.0.0.1", "--inspector-port", String(PORT + 2),
      "--persist-to", state, "--var", `SESSION_SECRET:${SECRET}`, "--var", `HYPERSNAP_ORIGIN:http://127.0.0.1:${NODE_PORT}`,
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

/** Signed in on the spec's Worker, with a key approved an hour ago. Only
 * the scheduled casts' endpoints are the Worker's own; the rest are
 * fixtures, as in the other specs. */
async function ready(page: Page) {
  // As `signIn` sets the user's cookie, with the session's beside it.
  await page.addInitScript(
    ({ user, session }) => {
      document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax`;
      document.cookie = `__Host-allthing-session=${session}; Path=/; Secure; SameSite=Lax`;
    },
    { user: me, session: session(me.fid) },
  );
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => route.fulfill({ json: { pool: [post], notes: {}, meta } }));
  await page.route("**/api/v1/signer", (route) => route.fulfill({ json: { app_fid: 9152 } }));
  await page.route("**/api/v1/scheduled**", (route) => route.continue());
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  await page.route(`${NODE}/castsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  await page.route(`${NODE}/submitMessage`, () => {
    throw new Error("the browser mustn't post a scheduled cast itself");
  });
  await page.goto(`${ORIGIN}/`);
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  await approvedKey(page, me.fid, 3_600_000);
  await page.reload();
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
}

/** The field's value for `at` (Unix seconds), in the browser's own zone. */
const field = (page: Page, at: number) =>
  page.evaluate((at) => {
    const d = new Date(at * 1000);
    const pad = (n: number) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
  }, at);

/** Schedule `text` for `at` from the composer; what the browser signed, in base64. */
async function schedule(page: Page, text: string, at: number) {
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await dialog.getByRole("textbox", { name: "What's happening?" }).fill(text);
  await dialog.getByRole("button", { name: "Schedule for later" }).click();
  await dialog.getByLabel("Goes out").fill(await field(page, at));
  const kept = page.waitForResponse((r) => r.url() === `${ORIGIN}/api/v1/scheduled` && r.request().method() === "POST");
  await dialog.getByRole("button", { name: "Schedule", exact: true }).click();
  const response = await kept;
  expect(response.status()).toBe(200);
  await expect(dialog).toBeHidden();
  return (response.request().postDataJSON() as { message: string }).message;
}

/** One run of the every-minute cron, as Cloudflare would start it. */
async function cron() {
  const res = await fetch(`${ORIGIN}/cdn-cgi/local/scheduled?cron=${encodeURIComponent("* * * * *")}`);
  expect(res.status).toBe(200);
}

test("a scheduled cast goes out at its minute: the cron gives the node what the browser signed, and the list says so", async ({ page }) => {
  test.setTimeout(240_000);
  await ready(page);
  // The first whole minute at least 75 s ahead: the composer takes one a
  // minute ahead, with room for scheduling two.
  const at = Math.ceil((now() + 75) / 60) * 60;
  const posted = await schedule(page, "Out at its minute", at);
  const refused = await schedule(page, "This one is refused at its minute", at);
  for (const message of [posted, refused]) {
    const signed = envelope(Buffer.from(message, "base64"));
    expect([signed.type, signed.fid, signed.at + EPOCH]).toEqual([1, me.fid, at]);
  }
  // Before its minute, a run gives the node nothing. Its indexer read the
  // stand-in, so what the Worker sends the node goes there, never to the
  // network.
  await cron();
  expect(given).toEqual([]);
  expect(asked.length).toBeGreaterThan(0);
  await page.goto(`${ORIGIN}/scheduled`);
  const group = (name: RegExp) => page.locator(".scheduled-group", { has: page.getByRole("heading", { name }) });
  await expect(group(/^Waiting/).locator(".scheduled-item")).toHaveCount(2);

  // At its minute, the run gives the node both, byte for byte as signed.
  await expect.poll(now, { timeout: 150_000, intervals: [1_000] }).toBeGreaterThanOrEqual(at);
  await cron();
  expect([...given].sort()).toEqual([posted, refused].sort());
  // The list on screen was read before the minute: cancelling the cast that
  // has just gone out turns its row into a posted one, with its link, and
  // no error.
  const hash = envelope(Buffer.from(posted, "base64")).hash;
  const stale = group(/^Waiting/).locator(".scheduled-item", { hasText: "Out at its minute" });
  await stale.getByRole("button", { name: "Cancel" }).click();
  await stale.getByRole("button", { name: "Yes, cancel it" }).click();
  const went = group(/^Posted this week/).locator(".scheduled-item");
  await expect(went).toContainText("It had already gone out, so it wasn't cancelled.");
  await expect(went.getByRole("link", { name: "View cast" })).toHaveAttribute("href", `/p/${hash}`);
  await expect(page.getByRole("alert")).toHaveCount(0);
  // The list says what became of each: one out, with its link; one not,
  // and why, to edit and sign again.
  await page.reload();
  await expect(group(/^Waiting/)).toHaveCount(0);
  const out = group(/^Posted this week/).locator(".scheduled-item");
  await expect(out).toHaveCount(1);
  await expect(out.locator(".scheduled-text")).toHaveText("Out at its minute");
  await expect(out.getByRole("link", { name: "View cast" })).toHaveAttribute("href", `/p/${hash}`);
  const lost = group(/^Didn't go out/).locator(".scheduled-item");
  await expect(lost).toHaveCount(1);
  await expect(lost.locator(".scheduled-text")).toHaveText("This one is refused at its minute");
  await expect(lost).toContainText("The key it was signed with isn't on your Farcaster account anymore. Edit it to sign it again.");
  await expect(lost.getByRole("button", { name: "Edit to try again" })).toBeVisible();
  // Settled: the next run gives the node nothing more.
  await cron();
  expect(given).toHaveLength(2);
});

type Kept = { id: number; hash: string; state: string };

/** The Worker's scheduled casts' endpoints, as `fid`'s session. */
const as = (request: APIRequestContext, fid: number) => {
  const headers = { cookie: `__Host-allthing-session=${session(fid)}`, origin: ORIGIN };
  return {
    list: async () => ((await (await request.get(`${ORIGIN}/api/v1/scheduled`, { headers })).json()) as { casts: Kept[] }).casts,
    cancel: (id: number) => request.post(`${ORIGIN}/api/v1/scheduled/cancel`, { data: { id }, headers }),
  };
};

test("the Worker's cancel says what it did: taken off, the same again, too late, or not yours", async ({ page, request }) => {
  const mine = as(request, me.fid);
  const casts = await mine.list();
  const out = casts.find((c) => c.state === "posted")!;
  const lost = casts.find((c) => c.state === "failed")!;
  expect(out && lost).toBeTruthy();
  // Gone out: too late, with its link; it stays on the list.
  const late = await mine.cancel(out.id);
  expect(late.status()).toBe(200);
  expect(await late.json()).toEqual({ outcome: "posted", hash: out.hash });
  // Someone else's: refused, and left as it is.
  const theirs = await as(request, 99).cancel(lost.id);
  expect(theirs.status()).toBe(403);
  expect(await theirs.json()).toMatchObject({ error: "That scheduled cast isn't yours to cancel. Reload the page to see your own." });
  expect((await mine.list()).map((c) => c.id).sort()).toEqual([out.id, lost.id].sort());
  // The reader's own that didn't go out: taken off, and asked again, the same.
  for (let i = 0; i < 2; i++) {
    const done = await mine.cancel(lost.id);
    expect(done.status()).toBe(200);
    expect(await done.json()).toEqual({ outcome: "cancelled" });
  }
  expect((await mine.list()).map((c) => c.id)).toEqual([out.id]);

  // One waiting, cancelled on the list: off it at once, and said.
  await ready(page);
  await schedule(page, "Cancel me", Math.ceil((now() + 3_600) / 60) * 60);
  await page.goto(`${ORIGIN}/scheduled`);
  const waiting = page.locator(".scheduled-item", { hasText: "Cancel me" });
  await waiting.getByRole("button", { name: "Cancel" }).click();
  await waiting.getByRole("button", { name: "Yes, cancel it" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Scheduled cast cancelled" })).toBeVisible();
  await expect(waiting).toHaveCount(0);
  expect((await mine.list()).map((c) => c.id)).toEqual([out.id]);
  // The cron's run since gives the node nothing.
  await cron();
  expect(given).toHaveLength(2);
});
