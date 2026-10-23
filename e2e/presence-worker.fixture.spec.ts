// SPDX-License-Identifier: AGPL-3.0-or-later
import { execFile, spawn, type ChildProcess } from "node:child_process";
import { createHmac, randomBytes } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer, type Server } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { promisify } from "node:util";
import { expect, test } from "@playwright/test";

/*
 * Presence on the real Worker, with a local D1: a beat (POST
 * /api/v1/presence) records its browser for the last few minutes and
 * answers how many are reading, who of them shows themselves by name, and
 * when to beat again. A signed-in reader is named by their session alone;
 * anyone else, or a reader who hides, is a keyed hash of the browser's id
 * for the day, and a row holds nothing more (no IP, no raw id). A Cloudflare
 * location recounts at most twice a minute: between recounts a beat writes
 * its row and gets the location's answer, so this spec waits for a recount
 * where it needs one. The node is a stand-in on this machine
 * (`HYPERSNAP_ORIGIN`), for the names and pictures. With `PRESENCE` other
 * than "on", the endpoint answers 404 and the header shows no chip. This
 * spec runs a Worker of its own, then another with presence off.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no Worker");
test.skip(Boolean(process.env.E2E_BASE_URL), "runs a Worker of its own");
test.describe.configure({ mode: "serial" });

const PORT = Number(process.env.E2E_PRESENCE_PORT ?? 8804);
const NODE_PORT = PORT + 1;
const INSPECTOR_PORT = PORT + 2;
const ORIGIN = `http://127.0.0.1:${PORT}`;
const SECRET = "presence-end-to-end-session-secret-for-tests-only";
const ROOT = path.join(__dirname, "..");
/** How long a location keeps the answer it counted, and a little more. */
const RECOUNT_MS = 31_000;
const reader = { fid: 1234, username: "reader", display_name: "Reader", pfp_url: "https://imagedelivery.net/fixture/pfp-reader/original" };
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const now = () => Math.floor(Date.now() / 1000);

let worker: ChildProcess | undefined;
let node: Server | undefined;
let state = "";
/** What the Worker printed, as it ran. */
const printed: string[] = [];
/** What the stand-in node was asked. */
const asked: string[] = [];

/** A session cookie as the Worker's sign-in sets it. */
function session(fid: number) {
  const value = `${fid}.${now() + 3_600}`;
  const tag = createHmac("sha256", SECRET).update(`allthing session\n${value}`).digest("hex");
  return `__Host-allthing-session=${value}.${tag}`;
}

/** A browser's random id for the day. */
const id = () => randomBytes(16).toString("hex");
/** The visitor the Worker keeps for a browser's id: a keyed hash of it. */
const hashed = (id: string) => `a:${createHmac("sha256", SECRET).update(`allthing presence v1\n${id}`).digest("hex")}`;

type Beaten = { status: number; cache: string | null; json: { online?: number; readers?: object[]; next?: number; error?: string } };

/** A beat, as Allthing's page sends it unless told otherwise. */
async function beat(body: object | string, { cookie = "", origin = ORIGIN as string | null } = {}): Promise<Beaten> {
  const headers: Record<string, string> = { "content-type": "application/json" };
  if (origin !== null) headers.origin = origin;
  if (cookie) headers.cookie = cookie;
  const response = await fetch(`${ORIGIN}/api/v1/presence`, { method: "POST", headers, body: typeof body === "string" ? body : JSON.stringify(body) });
  return { status: response.status, cache: response.headers.get("cache-control"), json: await response.json() };
}

/** A query on the Worker's local D1. Not run synchronously: that would
 * hold this process's event loop, and a kept-alive connection to the
 * Worker that closed meanwhile would be taken for an open one. */
async function d1(sql: string): Promise<Record<string, string | number | null>[]> {
  const args = ["wrangler", "d1", "execute", "allthing-index", "--local", "--persist-to", state, "--json", "--command", sql];
  const { stdout } = await promisify(execFile)("npx", args, { cwd: ROOT, encoding: "utf8" });
  return JSON.parse(stdout)[0].results;
}

/** Start a Worker on PORT with `vars`, an empty D1 and the stand-in node. */
async function start(vars: string[], ready: boolean) {
  state = mkdtempSync(path.join(tmpdir(), "allthing-presence-"));
  printed.length = 0;
  worker = spawn(
    "npx",
    [
      "wrangler", "dev", "--port", String(PORT), "--ip", "127.0.0.1", "--inspector-port", String(INSPECTOR_PORT), "--persist-to", state,
      "--var", `SESSION_SECRET:${SECRET}`, "--var", `HYPERSNAP_ORIGIN:http://127.0.0.1:${NODE_PORT}`, ...vars.flatMap((v) => ["--var", v]),
    ],
    { cwd: ROOT, stdio: ["ignore", "pipe", "pipe"], detached: true },
  );
  worker.stdout!.on("data", (chunk: Buffer) => printed.push(chunk.toString("utf8")));
  worker.stderr!.on("data", (chunk: Buffer) => printed.push(chunk.toString("utf8")));
  // Up, and this Worker (presence as asked), not one before it.
  await expect
    .poll(async () => {
      const status = await fetch(`${ORIGIN}/api/v1/status`).catch(() => null);
      return status?.ok ? (await status.json()).presence?.ready : null;
    }, { timeout: 120_000, intervals: [1_000] })
    .toBe(ready);
}

async function stop() {
  const running = worker;
  worker = undefined;
  if (running?.pid) {
    const exited = new Promise((done) => running.once("exit", done));
    process.kill(-running.pid, "SIGTERM");
    await exited;
  }
  if (state) rmSync(state, { recursive: true, force: true });
  state = "";
}

test.beforeAll(async () => {
  node = createServer((req, res) => {
    asked.push(req.url!);
    const url = new URL(req.url!, `http://127.0.0.1:${NODE_PORT}`);
    const json = (status: number, body: object) => res.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify(body));
    if (url.pathname === "/v2/farcaster/user/bulk") {
      const fids = (url.searchParams.get("fids") ?? "").split(",").map(Number);
      return json(200, { users: fids.filter((fid) => fid === reader.fid).map(() => reader) });
    }
    return json(404, {});
  }).listen(NODE_PORT, "127.0.0.1");
});

test.afterAll(async () => {
  await stop();
  await new Promise((done) => (node ? node.close(done) : done(null)));
});

test.describe("on", () => {
  /** When this location last counted. */
  let counted = 0;
  /** Wait until the next beat recounts. */
  const recount = async () => {
    const wait = counted + RECOUNT_MS - Date.now();
    if (wait > 0) await new Promise((done) => setTimeout(done, wait));
    counted = Date.now();
  };

  test.beforeAll(async () => {
    test.setTimeout(150_000);
    await start([], true);
  });
  test.afterAll(stop);

  test("beats count each browser once, by a hash of its id, and name a signed-in reader by their session alone", async () => {
    test.setTimeout(150_000);
    const [a, b, signedIn, forged, readable] = [id(), id(), id(), id(), id()];

    // The first beat counts: one here, no one named, beat again in four minutes.
    counted = Date.now();
    const first = await beat({ id: a, hidden: false });
    expect(first.status).toBe(200);
    expect(first.cache).toBe("no-store");
    expect(first.json).toEqual({ online: 1, readers: [], next: 240 });
    // The same browser again: still one, in one row.
    expect((await beat({ id: a })).json.online).toBe(1);
    expect(await d1("SELECT COUNT(*) AS n FROM presence")).toEqual([{ n: 1 }]);
    // Another browser: a row of its own. (The answer is the one this
    // location counted a moment ago.)
    expect((await beat({ id: b })).status).toBe(200);
    // A signed-in reader, named by their session cookie.
    expect((await beat({ id: signedIn }, { cookie: session(reader.fid) })).status).toBe(200);
    // Never by what the browser says: a fid in the beat, or the readable user cookie.
    expect((await beat({ id: forged, fid: 7, username: "forged" })).status).toBe(200);
    const userCookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify({ fid: 8, username: "claimed" }))}`;
    expect((await beat({ id: readable }, { cookie: userCookie })).status).toBe(200);
    const rows = await d1("SELECT visitor, fid FROM presence ORDER BY visitor");
    expect(rows).toEqual(
      [
        { visitor: "f:1234", fid: 1234 },
        ...[a, b, forged, readable].map((id) => ({ visitor: hashed(id), fid: null })),
      ].sort((x, y) => x.visitor.localeCompare(y.visitor)),
    );

    // Rows a quarter of an hour old go as the location next counts; those
    // of ten minutes ago stay, but aren't counted.
    const [old, earlier] = [`a:${"0".repeat(64)}`, `a:${"1".repeat(64)}`];
    await d1(`INSERT INTO presence (visitor, fid, seen) VALUES ('${old}', NULL, ${now() - 2_000}), ('${earlier}', NULL, ${now() - 600})`);
    await recount();
    const both = await beat({ id: a });
    expect(both.json.online).toBe(5);
    expect(both.json.next).toBe(240);
    // The signed-in reader, with the name and picture the node has.
    expect(both.json.readers).toEqual([reader]);
    expect(asked).toContain(`/v2/farcaster/user/bulk?fids=${reader.fid}`);
    expect(await d1(`SELECT visitor FROM presence WHERE visitor IN ('${old}', '${earlier}')`)).toEqual([{ visitor: earlier }]);

    // Hiding: counted as one signed out, and the row by name goes at once.
    expect((await beat({ id: signedIn, hidden: true }, { cookie: session(reader.fid) })).status).toBe(200);
    expect(await d1("SELECT COUNT(*) AS n FROM presence WHERE fid IS NOT NULL")).toEqual([{ n: 0 }]);
    expect(await d1(`SELECT fid FROM presence WHERE visitor = '${hashed(signedIn)}'`)).toEqual([{ fid: null }]);
    // A cached named answer must not leak the now-hidden reader, even
    // before its normal 30-second recount. The same D1 batch checks names.
    expect((await beat({ id: b })).json.readers).toEqual([]);
    await recount();
    const hidden = await beat({ id: b });
    expect(hidden.json).toEqual({ online: 5, readers: [], next: 240 });

    // Showing again atomically replaces this browser's anonymous row and
    // refreshes the edge answer, never counting the same reader twice.
    const shown = await beat({ id: signedIn, hidden: false, refresh: true }, { cookie: session(reader.fid) });
    expect(shown.json).toEqual({ online: 5, readers: [reader], next: 240 });
    expect(await d1(`SELECT COUNT(*) AS n FROM presence WHERE visitor IN ('${hashed(signedIn)}', 'f:${reader.fid}')`)).toEqual([{ n: 1 }]);
    expect(await d1(`SELECT visitor, fid FROM presence WHERE visitor = 'f:${reader.fid}'`)).toEqual([{ visitor: `f:${reader.fid}`, fid: reader.fid }]);
    // Hiding in a fresh edge-cache window returns a clean list at once.
    const hiddenAgain = await beat({ id: signedIn, hidden: true, refresh: true }, { cookie: session(reader.fid) });
    expect(hiddenAgain.json).toEqual({ online: 5, readers: [], next: 240 });

    // The ordinary anonymous -> sign-in path needs no trusted client flag.
    const signedAgain = await beat({ id: signedIn }, { cookie: session(reader.fid) });
    expect(signedAgain.json).toEqual({ online: 5, readers: [reader], next: 240 });
    expect(await d1("SELECT COUNT(*) AS n FROM presence")).toEqual([{ n: 6 }]); // five current and one older row kept

    // Simulate a hide committed by another location while this location's
    // edge answer still names the reader. A cache-hit beat rechecks D1.
    await d1(`DELETE FROM presence WHERE visitor = 'f:${reader.fid}'`);
    await d1(`INSERT INTO presence (visitor, fid, seen) VALUES ('${hashed(signedIn)}', NULL, ${now()})`);
    expect((await beat({ id: b })).json.readers).toEqual([]);

    // Sign-out removes a public row while the session can still prove
    // whose it is, and the next anonymous beat does not double-count it.
    await beat({ id: signedIn, refresh: true }, { cookie: session(reader.fid) });
    const signout = await fetch(`${ORIGIN}/api/v1/auth/signout`, {
      method: "POST", headers: { origin: ORIGIN, cookie: session(reader.fid) },
    });
    expect(signout.status).toBe(200);
    expect(await d1("SELECT COUNT(*) AS n FROM presence WHERE fid IS NOT NULL")).toEqual([{ n: 0 }]);
    const signedOut = await beat({ id: signedIn, refresh: true });
    expect(signedOut.json).toEqual({ online: 5, readers: [], next: 240 });

    // Every row is an account or a hash, and holds nothing else: no IP, no raw id.
    const all = await d1("SELECT * FROM presence");
    for (const row of all) {
      expect(Object.keys(row).sort()).toEqual(["fid", "seen", "visitor"]);
      expect(String(row.visitor)).toMatch(/^(f:[1-9][0-9]*|a:[0-9a-f]{64})$/);
    }
    const kept = JSON.stringify(all);
    for (const raw of [a, b, signedIn, forged, readable, "127.0.0.1", "forged", "claimed"]) expect(kept).not.toContain(raw);
    // Nor did the Worker print them.
    const logged = printed.join("");
    for (const raw of [a, b, signedIn, forged, readable]) expect(logged).not.toContain(raw);
  });

  test("a beat from another site, with a bad id or too big a body, or not a POST, is refused and writes nothing", async () => {
    const rows = (await d1("SELECT COUNT(*) AS n FROM presence"))[0].n;
    const refused = [
      [await beat({ id: id() }, { origin: "https://elsewhere.example" }), 403],
      [await beat({ id: id() }, { origin: null }), 403],
      [await beat({ id: id().toUpperCase() }), 400],
      [await beat({ id: id().slice(0, 31) }), 400],
      [await beat({ id: `${id()}0` }), 400],
      [await beat({ id: "g".repeat(32) }), 400],
      [await beat({}), 400],
      [await beat("not json"), 400],
      [await beat({ id: id(), padding: "x".repeat(300) }), 400],
    ] as const;
    for (const [answer, status] of refused) {
      expect(answer.status).toBe(status);
      expect(answer.cache).toBe("no-store");
    }
    expect((await fetch(`${ORIGIN}/api/v1/presence`)).status).toBe(405);
    expect((await d1("SELECT COUNT(*) AS n FROM presence"))[0].n).toBe(rows);
  });

  test("the status says presence is on", async () => {
    const status = await (await fetch(`${ORIGIN}/api/v1/status`)).json();
    expect(status.presence).toEqual({ ready: true });
  });
});

test.describe("off", () => {
  test.beforeAll(async () => {
    test.setTimeout(150_000);
    await start(["PRESENCE:off"], false);
  });
  test.afterAll(stop);

  test("with PRESENCE off, a beat is a 404 and the header has no chip", async ({ browser }, info) => {
    const answer = await beat({ id: id() });
    expect(answer.status).toBe(404);
    expect(answer.json.error).toBe("Presence is off here");
    for (const width of [1280, 390]) {
      const context = await browser.newContext({ viewport: { width, height: 844 }, isMobile: width < 400, hasTouch: width < 400 });
      const page = await context.newPage();
      // The page's own read from a fixture; the beat to this Worker.
      await page.route("**/api/v1/home**", (route) =>
        route.fulfill({ json: { pool: [{ hash: `0x${"5".repeat(40)}`, author: reader, text: "A quiet day", ts: now() - 600, likes: 1, recasts: 0, replies: 0 }], notes: {}, meta } }),
      );
      await page.route("https://imagedelivery.net/**", (route) => route.fulfill({ status: 404, body: "" }));
      const beaten = page.waitForResponse((response) => response.url().endsWith("/api/v1/presence"), { timeout: 20_000 });
      await page.goto(`${ORIGIN}/`);
      expect((await beaten).status()).toBe(404);
      await page.waitForTimeout(500);
      await expect(page.locator(".presence-chip")).toHaveCount(0);
      await expect(page.getByRole("link", { name: "Home", exact: true })).toBeVisible();
      await page.screenshot({ path: info.outputPath(`presence-off-${width}.png`), clip: { x: 0, y: 0, width, height: 120 } });
      await context.close();
    }
  });
});
