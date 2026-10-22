// SPDX-License-Identifier: AGPL-3.0-or-later
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { createHmac } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";

/*
 * Votes in Allthing's poll on the real Worker, with a local D1: one vote per
 * network (an IPv4 address, or an IPv6 address's /64), from Allthing's own
 * pages only. D1 keeps an HMAC of the poll and the network, keyed with the
 * session secret, and the choice: never the address. A network that has
 * voted is told so, with the counts, and never how it voted. wrangler dev
 * passes on the CF-Connecting-IP a request carries (and sets 127.0.0.1
 * where there is none), so each vote here comes from the address it names.
 * This spec runs a Worker of its own, with an empty local D1 and a session
 * secret; the default server, which has no secret in CI, says polls aren't
 * set up. The poll's card: poll.fixture.spec.ts.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no Worker");
test.skip(Boolean(process.env.E2E_BASE_URL), "runs a Worker of its own");

const PORT = Number(process.env.E2E_POLL_PORT ?? 8805);
const ORIGIN = `http://127.0.0.1:${PORT}`;
const SECRET = "poll-end-to-end-session-secret-for-tests-only";
const ROOT = path.join(__dirname, "..");
const POLL = "mobile-client-v1";

let worker: ChildProcess | undefined;
let state = "";

/** The key a network's vote is kept under: the HMAC-SHA256 of "allthing poll v1", a newline and "<poll>|<network>", keyed with the session secret. */
const voter = (network: string) => createHmac("sha256", SECRET).update(`allthing poll v1\n${POLL}|${network}`).digest("hex");

/** A query on the spec's local D1, as the Worker sees it. */
function d1(sql: string): Record<string, string | number>[] {
  const out = execFileSync("npx", ["wrangler", "d1", "execute", "allthing-index", "--local", "--persist-to", state, "--json", "--command", sql], {
    cwd: ROOT,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  });
  return JSON.parse(out)[0].results;
}

type Answer = { status: number; json: { counted?: boolean; already?: boolean; counts?: object; error?: string }; headers: Headers };

/** A vote from `address` (CF-Connecting-IP; `null`: none), from Allthing itself unless told otherwise. */
async function vote(body: object | string, address: string | null, origin: string | null = ORIGIN): Promise<Answer> {
  const headers: Record<string, string> = { "content-type": "application/json" };
  if (address !== null) headers["cf-connecting-ip"] = address;
  if (origin !== null) headers.origin = origin;
  const res = await fetch(`${ORIGIN}/api/v1/poll`, { method: "POST", headers, body: typeof body === "string" ? body : JSON.stringify(body) });
  return { status: res.status, json: await res.json(), headers: res.headers };
}

const yes = { poll: POLL, choice: "yes" };
const no = { poll: POLL, choice: "no" };

test.beforeAll(async () => {
  test.setTimeout(150_000);
  state = mkdtempSync(path.join(tmpdir(), "allthing-poll-"));
  worker = spawn(
    "npx",
    [
      "wrangler", "dev", "--port", String(PORT), "--ip", "127.0.0.1", "--inspector-port", String(PORT + 2),
      "--persist-to", state, "--var", `SESSION_SECRET:${SECRET}`,
    ],
    { cwd: ROOT, stdio: "ignore", detached: true },
  );
  await expect
    .poll(async () => (await fetch(`${ORIGIN}/api/v1/status`).catch(() => null))?.status ?? 0, { timeout: 120_000, intervals: [1_000] })
    .toBe(200);
});

test.afterAll(async () => {
  if (worker?.pid) process.kill(-worker.pid, "SIGTERM");
  if (state) rmSync(state, { recursive: true, force: true });
});

test("one vote counts per network, which is told it voted already, never how; D1 keeps no address", async () => {
  const status = await (await fetch(`${ORIGIN}/api/v1/status`)).json();
  expect(status.poll).toEqual({ ready: true });

  const first = await vote(yes, "203.0.113.7");
  expect(first.status).toBe(200);
  expect(first.headers.get("cache-control")).toBe("no-store");
  expect(first.json).toEqual({ counted: true, counts: { yes: 1, no: 0 } });
  // The same address again, the other way: not counted, and nothing of how it voted.
  const again = await vote(no, "203.0.113.7");
  expect(again.status).toBe(200);
  expect(again.headers.get("cache-control")).toBe("no-store");
  expect(again.json).toEqual({ counted: false, already: true, counts: { yes: 1, no: 0 } });
  // The same IPv4 address written as IPv6 is the same network.
  expect((await vote(no, "::ffff:203.0.113.7")).json).toEqual({ counted: false, already: true, counts: { yes: 1, no: 0 } });
  // Another network's vote counts.
  expect((await vote(no, "198.51.100.20")).json).toEqual({ counted: true, counts: { yes: 1, no: 1 } });
  // An IPv6 address's /64 is one network: one of its addresses votes for all of them.
  expect((await vote(yes, "2001:db8:1:2::5")).json).toEqual({ counted: true, counts: { yes: 2, no: 1 } });
  expect((await vote(no, "2001:db8:1:2:aaaa:bbbb:cccc:dddd")).json).toEqual({ counted: false, already: true, counts: { yes: 2, no: 1 } });
  expect((await vote(no, "2001:db8:1:3::5")).json).toEqual({ counted: true, counts: { yes: 2, no: 2 } });

  // D1: each network's vote under its HMAC (64 hex digits), the counts its
  // votes keep, and no address anywhere.
  const votes = d1("SELECT poll, voter, choice FROM poll_votes");
  expect(votes).toHaveLength(4);
  for (const row of votes) {
    expect(row.poll).toBe(POLL);
    expect(row.voter).toMatch(/^[0-9a-f]{64}$/);
  }
  const kept = Object.fromEntries(votes.map((row) => [row.voter, row.choice]));
  expect(kept).toEqual({
    [voter("203.0.113.7")]: "yes",
    [voter("198.51.100.20")]: "no",
    [voter("2001:db8:1:2::/64")]: "yes",
    [voter("2001:db8:1:3::/64")]: "no",
  });
  expect(d1(`SELECT choice, n FROM poll_counts WHERE poll = '${POLL}' ORDER BY choice`)).toEqual([
    { choice: "no", n: 2 },
    { choice: "yes", n: 2 },
  ]);
  const everything = JSON.stringify([d1("SELECT * FROM poll_votes"), d1("SELECT * FROM poll_counts")]);
  for (const address of ["203.0.113", "198.51.100", "2001:db8", "127.0.0.1"]) expect(everything).not.toContain(address);
});

test("only Allthing's own pages vote, and only a well-formed vote from a known address counts", async () => {
  const before = d1("SELECT COUNT(*) AS n FROM poll_votes")[0].n;
  // From another site, or with no Origin at all: refused.
  for (const origin of ["https://evil.example", null]) {
    const refused = await vote(yes, "192.0.2.1", origin);
    expect(refused.status).toBe(403);
    expect(refused.headers.get("cache-control")).toBe("no-store");
  }
  // Not a POST.
  expect((await fetch(`${ORIGIN}/api/v1/poll`, { headers: { "cf-connecting-ip": "192.0.2.1" } })).status).toBe(405);
  // Not a vote: not JSON, no choice, a choice there isn't, a poll there isn't, too long.
  for (const body of [
    "",
    "yes",
    "{",
    { poll: POLL },
    { poll: POLL, choice: "maybe" },
    { poll: POLL, choice: "YES" },
    { poll: "another-poll-v1", choice: "yes" },
    { poll: POLL, choice: "yes", note: "x".repeat(300) },
  ]) {
    const refused = await vote(body, "192.0.2.1");
    expect(refused.status, JSON.stringify(body)).toBe(400);
    expect(refused.headers.get("cache-control")).toBe("no-store");
  }
  // From an address that can't be read: not counted for anyone.
  expect((await vote(yes, "not-an-address")).status).toBe(400);
  expect(d1("SELECT COUNT(*) AS n FROM poll_votes")[0].n).toBe(before);
});

test("a server without SESSION_SECRET says polls aren't set up", async ({ request, baseURL }) => {
  const status = await (await request.get("/api/v1/status")).json();
  test.skip(status.poll?.ready === true, "this server has a session secret");
  expect(status.poll).toEqual({ ready: false });
  const refused = await request.post("/api/v1/poll", { data: yes, headers: { origin: new URL(baseURL!).origin } });
  expect(refused.status()).toBe(503);
  expect(refused.headers()["cache-control"]).toBe("no-store");
  expect(await refused.json()).toMatchObject({ error: "Polls aren't set up here" });
});
