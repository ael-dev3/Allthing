// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type BrowserContext, type Page, type WebSocketRoute } from "@playwright/test";
import { buildSync } from "esbuild";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { createRequire } from "node:module";
import { join, resolve } from "node:path";
import { appCame } from "./helpers/app";
import { signIn } from "./helpers/farcaster";

/*
 * Notification read watermarks across separate browser storage/session scopes.
 * The real built SDK/bridge runs in Chromium; this Node-only peer exchanges
 * actual SpacetimeDB BSATN frames and exposes only the authenticated FID view.
 * Notification content still comes exclusively from the Worker fixture.
 */
test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no signed-in notifications");
const temp = mkdtempSync(join(tmpdir(), "allthing-notification-wire-"));
buildSync({ entryPoints: [resolve("e2e/fixtures/notification-sync-wire.ts")], outfile: join(temp, "wire.cjs"), bundle: true, platform: "node", format: "cjs", logLevel: "silent" });
const wire = createRequire(resolve("package.json"))(join(temp, "wire.cjs"));
test.afterAll(() => rmSync(temp, { recursive: true, force: true }));

const SEEN = "allthing-notifications-seen";
const at = Math.floor(Date.now() / 1000);
const alice = { fid: 1234, username: "reader", display_name: "Reader" };
const bob = { fid: 5678, username: "other", display_name: "Other" };
const actor = { fid: 88, username: "fixture", display_name: "Fixture" };
const hash = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const meta = () => ({ backend: "hypersnap", fetched_at: Date.now() });
const payload = (fid: number, newest = at - 300) => ({
  groups: [
    { kind: "reply", actors: [actor], count: 1, ts: newest, cast: { hash: hash(1), excerpt: `Newest notification for ${fid}` } },
    { kind: "like", actors: [actor], count: 1, ts: at - 360, cast: { hash: hash(2), excerpt: `Earlier notification for ${fid}` } },
  ], newest, notes: {}, meta: meta(),
});
type Watermark = { fid: number; seenAt: number };
type Connection = { label: string; socket: WebSocketRoute; identity: string; token: string; active: boolean; fid?: number; querySetId?: unknown; row?: Watermark };

class Peer {
  connections: Connection[] = [];
  tokens = new Map<string, string>();
  authorized = new Map<string, number>();
  states = new Map<number, number>();
  offline = new Set<string>();
  calls: { label: string; identity: string; name: string; fid?: number; seenAt?: number }[] = [];
  queries: string[] = [];
  history: { fid: number; seenAt: number }[] = [];
  tokenExchanges: string[] = [];
  verified(label: string) { return this.connections.filter(c => c.label === label && c.active && c.fid !== undefined); }
  seen(fid: number) { return this.states.get(fid) ?? 0; }
  sendState(connection: Connection) {
    if (!connection.active || connection.querySetId === undefined || connection.fid === undefined) return;
    const seenAt = this.states.get(connection.fid);
    if (seenAt === undefined) return;
    const row = { fid: connection.fid, seenAt };
    connection.socket.send(wire.update(connection.querySetId, connection.row, row));
    connection.row = row;
  }
  async attach(page: Page, label: string) {
    // The real SDK exchanges a persisted token before opening its next socket.
    // Keep that HTTP step inside this peer too: fixture tokens never go live.
    await page.route("https://maincloud.spacetimedb.com/**", async route => {
      const headers = { "access-control-allow-origin": "*", "access-control-allow-headers": "authorization", "access-control-allow-methods": "POST, OPTIONS" };
      if (route.request().method() === "OPTIONS") return route.fulfill({ status: 204, headers });
      const token = (await route.request().headerValue("authorization"))?.replace(/^Bearer /, "");
      if (new URL(route.request().url()).pathname !== "/v1/identity/websocket-token" || !token || !this.tokens.has(token)) {
        return route.fulfill({ status: 401, json: { error: "Unknown fixture token" }, headers });
      }
      this.tokenExchanges.push(label);
      if (this.offline.has(label)) return route.fulfill({ status: 503, json: { error: "Fixture service offline" }, headers });
      return route.fulfill({ json: { token }, headers });
    });
    await page.routeWebSocket("wss://maincloud.spacetimedb.com/**", socket => {
      if (this.offline.has(label)) { void socket.close({ code: 1013, reason: "Fixture offline" }); return; }
      const requested = new URL(socket.url()).searchParams.get("token");
      const token = requested && this.tokens.has(requested) ? requested : `notification-token-${this.tokens.size + 1}`;
      let identity = this.tokens.get(token);
      if (!identity) { identity = `c3${(this.tokens.size + 1).toString(16).padStart(62, "0")}`; this.tokens.set(token, identity); }
      const connection: Connection = { label, socket, identity, token, active: true };
      this.connections.push(connection);
      socket.onClose(() => { connection.active = false; });
      socket.onMessage(bytes => {
        const message = wire.client(bytes);
        if (message.tag === "Subscribe") {
          connection.querySetId = message.value.querySetId;
          this.queries.push(...message.value.queryStrings);
          const seenAt = connection.fid === undefined ? undefined : this.states.get(connection.fid);
          connection.row = seenAt === undefined ? undefined : { fid: connection.fid!, seenAt };
          socket.send(wire.snapshot(message.value.requestId, message.value.querySetId, connection.row));
        } else if (message.tag === "CallReducer") {
          const { reducer: name, args, requestId } = message.value;
          try {
            const call = { label, identity: connection.identity, name, fid: undefined as number | undefined, seenAt: undefined as number | undefined };
            this.calls.push(call);
            const decoded = wire.reducer(name, args);
            if (name === "verify_notification_fid") {
              const fid = Number(decoded.fid);
              call.fid = fid;
              if (this.authorized.get(connection.identity) !== fid) throw new Error("attestation does not authorize this FID");
              connection.fid = fid;
              socket.send(wire.ok(requestId));
              this.sendState(connection);
            } else {
              if (connection.fid === undefined) throw new Error("verify the notification reader first");
              const seenAt = Number(decoded.seenAt);
              call.fid = connection.fid; call.seenAt = seenAt;
              if (!Number.isSafeInteger(seenAt) || seenAt < 0 || seenAt > Math.floor(Date.now() / 1000)) throw new Error("invalid seen time");
              const next = Math.max(this.seen(connection.fid), seenAt);
              this.states.set(connection.fid, next);
              this.history.push({ fid: connection.fid, seenAt: next });
              socket.send(wire.ok(requestId));
              for (const subscriber of this.connections) if (subscriber.fid === connection.fid) this.sendState(subscriber);
            }
          } catch (error) { socket.send(wire.error(requestId, String(error))); }
        } else if (message.tag === "Unsubscribe") {
          // This app disconnects its private connection on account changes.
          connection.querySetId = undefined;
        } else { throw new Error(`Unexpected client protocol message ${message.tag}`); }
      });
      socket.send(wire.initial(connection.identity, token, this.connections.length));
    });
  }
  async disconnect(label: string) {
    this.offline.add(label);
    for (const connection of this.connections.filter(c => c.label === label && c.active)) {
      connection.active = false;
      await connection.socket.close({ code: 1013, reason: "Fixture offline" });
    }
  }
}

function fidFrom(cookie: string) {
  const raw = /(?:^|;\s*)__Host-allthing-user=([^;]*)/.exec(cookie)?.[1];
  try { return raw ? Number(JSON.parse(decodeURIComponent(raw)).fid) : 0; } catch { return 0; }
}

async function reader(browser: Browser, peer: Peer, label: string, who: typeof alice | null, options: { clock?: boolean; holdAttestation?: number; configured?: boolean } = {}) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  const page = await context.newPage();
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  if (options.clock) await page.clock.install();
  let releaseAttestation!: () => void;
  const held = new Promise<void>(resolve => { releaseAttestation = resolve; });
  const state = { newest: at - 300, calls: 0, status: 0, attestations: [] as number[], finishedAttestations: [] as number[] };
  await page.route("**/api/v1/**", route => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/status", route => {
    state.status++;
    return options.configured === false
      ? route.fulfill({ status: 404, json: { error: "Sync is not configured" } })
      : route.fulfill({ json: { spacetime: { attest: true } } });
  });
  await page.route("**/api/v1/home**", route => route.fulfill({ json: { pool: [], notes: {}, meta: meta() } }));
  await page.route("**/api/v1/notifications**", async route => {
    const fid = fidFrom(await route.request().headerValue("cookie") ?? "");
    state.calls++;
    return route.fulfill({ json: payload(fid, state.newest), headers: { "cache-control": "private, no-store" } });
  });
  await page.route("**/api/v1/auth/attest", async route => {
    const fid = fidFrom(await route.request().headerValue("cookie") ?? "");
    state.attestations.push(fid);
    if (fid === options.holdAttestation) await held;
    const { identity } = route.request().postDataJSON();
    peer.authorized.set(identity, fid);
    await route.fulfill({ json: { identity, fid, expiry: Math.floor(Date.now() / 1000) + 600, signature: "11".repeat(64) } });
    state.finishedAttestations.push(fid);
  });
  await peer.attach(page, label);
  if (who && options.holdAttestation) {
    await page.addInitScript(user => {
      if (sessionStorage.getItem("notification-fixture-initial-signin")) return;
      sessionStorage.setItem("notification-fixture-initial-signin", "1");
      document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax`;
    }, who);
  } else if (who) await signIn(page, who);
  return { context, page, state, errors, releaseAttestation };
}

const bell = (page: Page) => page.getByRole("link", { name: /^Notifications, \d+ unread$/ });
const rows = (page: Page) => page.locator(".notif-list > .notif:not(.skeleton)");
const localSeen = (page: Page, fid = alice.fid) => page.evaluate(({ key, fid }) => JSON.parse(localStorage.getItem(key) ?? "{}")[fid] ?? 0, { key: SEEN, fid });
async function noMessageKeys(pages: Page[]) {
  for (const page of pages) {
    // A notification visit must not initialize Messages' separate key store.
    const databases = await page.evaluate(async () => (await indexedDB.databases()).map(db => db.name));
    expect(databases).not.toContain("allthing-stdb");
  }
}
async function noMessageSideEffects(peer: Peer, pages: Page[]) {
  expect(peer.queries.length).toBeGreaterThan(0);
  expect(peer.queries.every(query => /^\s*select\s+\*\s+from\s+my_notification_state\s*;?\s*$/i.test(query))).toBe(true);
  expect(peer.calls.every(call => ["verify_notification_fid", "mark_notifications_seen"].includes(call.name))).toBe(true);
  await noMessageKeys(pages);
}
async function close(...readers: { context: BrowserContext }[]) { await Promise.all(readers.map(reader => reader.context.close())); }
async function diagnose(peer: Peer, ...readers: Awaited<ReturnType<typeof reader>>[]) {
  const pages = await Promise.all(readers.map(async reader => ({
    requests: reader.state, errors: reader.errors,
    document: await reader.page.evaluate(key => ({
      boot: Boolean(document.querySelector("#boot")), app: Boolean(document.querySelector("#app")),
      bell: document.querySelector(".bell")?.getAttribute("aria-label"), seen: localStorage.getItem(key),
      bridge: Boolean((globalThis as { allthingStdb?: unknown }).allthingStdb),
    }), SEEN).catch(() => ({ unavailable: true })),
  })));
  await test.info().attach("notification-sync-diagnostics", { contentType: "application/json", body: JSON.stringify({
    pages, queries: peer.queries, history: peer.history, tokenExchanges: peer.tokenExchanges,
    calls: peer.calls.map(({ label, name, fid, seenAt }) => ({ label, name, fid, seenAt })),
    connections: peer.connections.map(({ label, active, fid, querySetId }) => ({ label, active, fid, subscribed: querySetId !== undefined })),
  }, null, 2) });
}

// Contexts are deliberately independent: no shared storageState, localStorage,
// cookies or BroadcastChannel can explain their synchronized badges.
test("reading notifications clears another session and a fresh session hydrates the prior read baseline", async ({ browser }) => {
  const peer = new Peer();
  const a = await reader(browser, peer, "a", alice), b = await reader(browser, peer, "b", alice);
  let fresh: Awaited<ReturnType<typeof reader>> | undefined;
  try {
    await Promise.all([a.page.goto("/"), b.page.goto("/")]);
    await expect.poll(() => peer.verified("a").length + peer.verified("b").length).toBe(2);
    await expect(bell(a.page)).toHaveAccessibleName("Notifications, 2 unread");
    await expect(bell(b.page)).toHaveAccessibleName("Notifications, 2 unread");
    expect(peer.verified("a")[0].identity).not.toBe(peer.verified("b")[0].identity);
    await a.page.goto("/notifications");
    await expect(rows(a.page)).toHaveCount(2);
    await expect(bell(b.page)).toHaveAccessibleName("Notifications, 0 unread");
    await expect.poll(() => localSeen(b.page)).toBe(a.state.newest);
    expect(b.state.calls).toBe(1);
    // Clearing another session's bell does not change this visit's row tint.
    await expect(a.page.locator(".notif.new")).toHaveCount(2);
    fresh = await reader(browser, peer, "fresh", alice);
    await fresh.page.goto("/notifications");
    await expect.poll(() => peer.verified("fresh").length).toBe(1);
    await expect(rows(fresh.page)).toHaveCount(2);
    await expect(bell(fresh.page)).toHaveAccessibleName("Notifications, 0 unread");
    await expect(fresh.page.locator(".notif.new")).toHaveCount(0);
    await noMessageSideEffects(peer, [a.page, b.page, fresh.page]);
    expect([...a.errors, ...b.errors, ...fresh.errors]).toEqual([]);
  } catch (error) { await diagnose(peer, a, b, ...(fresh ? [fresh] : [])); throw error; }
  finally { await close(a, b, ...(fresh ? [fresh] : [])); }
});

test("a later notification becomes unread after a remote read without an extra read on the sync event", async ({ browser }) => {
  const peer = new Peer();
  const a = await reader(browser, peer, "a", alice), b = await reader(browser, peer, "b", alice, { clock: true });
  try {
    await Promise.all([a.page.goto("/notifications"), b.page.goto("/")]);
    await expect.poll(() => peer.seen(alice.fid)).toBe(a.state.newest);
    await expect(bell(b.page)).toHaveAccessibleName("Notifications, 0 unread");
    expect(b.state.calls).toBe(1);
    b.state.newest += 120;
    await b.page.clock.fastForward("01:01");
    await b.page.evaluate(() => window.dispatchEvent(new Event("focus")));
    await expect(bell(b.page)).toHaveAccessibleName("Notifications, 1 unread");
    expect(b.state.calls).toBe(2);
    expect(peer.seen(alice.fid)).toBe(a.state.newest);
    await b.page.goto("/notifications?tab=replies");
    await expect(rows(b.page)).toHaveCount(1);
    await expect.poll(() => peer.seen(alice.fid)).toBe(b.state.newest);
    await expect.poll(() => localSeen(a.page)).toBe(b.state.newest);
    await expect(a.page.locator(".notif.new")).toHaveCount(2);
    await expect(b.page.getByRole("tab", { name: "Replies", exact: true })).toHaveAttribute("aria-selected", "true");
    await noMessageSideEffects(peer, [a.page, b.page]);
    expect([...a.errors, ...b.errors]).toEqual([]);
  } catch (error) { await diagnose(peer, a, b); throw error; }
  finally { await close(a, b); }
});

test("offline local reads reconcile with a newer remote watermark without rolling it back", async ({ browser }) => {
  const peer = new Peer();
  const a = await reader(browser, peer, "a", alice), b = await reader(browser, peer, "b", alice);
  try {
    await Promise.all([a.page.goto("/"), b.page.goto("/")]);
    await expect.poll(() => peer.verified("a").length + peer.verified("b").length).toBe(2);
    await peer.disconnect("a");
    await a.page.goto("/notifications");
    await expect(rows(a.page)).toHaveCount(2);
    await expect.poll(() => localSeen(a.page)).toBe(a.state.newest);
    expect(peer.seen(alice.fid)).toBe(0);
    b.state.newest += 120;
    // A new document discards the old one-minute in-memory source cache.
    await b.page.goto("/notifications");
    await expect.poll(() => peer.seen(alice.fid)).toBe(b.state.newest);
    peer.offline.delete("a");
    await a.page.evaluate(() => { window.dispatchEvent(new Event("online")); window.dispatchEvent(new Event("focus")); });
    await expect.poll(() => peer.verified("a").length).toBe(1);
    await expect.poll(() => localSeen(a.page)).toBe(b.state.newest);
    expect(peer.seen(alice.fid)).toBe(b.state.newest);
    const history = peer.history.filter(row => row.fid === alice.fid).map(row => row.seenAt);
    expect(history.every((seen, i) => i === 0 || seen >= history[i - 1])).toBe(true);
    await a.page.reload();
    await expect(rows(a.page)).toHaveCount(2);
    await expect(bell(a.page)).toHaveAccessibleName("Notifications, 0 unread");
    await expect(a.page.locator(".notif.new")).toHaveCount(0);
    await noMessageSideEffects(peer, [a.page, b.page]);
    expect([...a.errors, ...b.errors]).toEqual([]);
  } catch (error) { await diagnose(peer, a, b); throw error; }
  finally { await close(a, b); }
});

test("sign-out and an account switch fence an old attestation and keep FID read state isolated", async ({ browser }) => {
  const peer = new Peer(); peer.states.set(alice.fid, at - 180);
  const a = await reader(browser, peer, "switch", alice, { holdAttestation: alice.fid });
  const b = await reader(browser, peer, "bob", bob);
  try {
    await Promise.all([a.page.goto("/"), b.page.goto("/")]);
    await expect.poll(() => a.state.attestations.includes(alice.fid)).toBe(true);
    await expect.poll(() => peer.verified("bob").length).toBe(1);
    await expect(bell(b.page)).toHaveAccessibleName("Notifications, 2 unread");
    await a.page.evaluate(user => {
      document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax`;
      new BroadcastChannel("allthing-session").postMessage("changed");
    }, bob);
    await expect.poll(() => peer.verified("switch").some(c => c.fid === bob.fid)).toBe(true);
    await expect(bell(a.page)).toHaveAccessibleName("Notifications, 2 unread");
    expect(await localSeen(a.page, bob.fid)).toBe(0);
    await a.page.evaluate(() => {
      document.cookie = "__Host-allthing-user=; Path=/; Max-Age=0; Secure; SameSite=Lax";
      new BroadcastChannel("allthing-session").postMessage("changed");
    });
    await expect(a.page.locator(".bell")).toHaveCount(0);
    a.releaseAttestation();
    await expect.poll(() => a.state.finishedAttestations.includes(alice.fid)).toBe(true);
    await a.page.goto("/notifications");
    await expect(a.page.getByText("Sign in to see your notifications.")).toBeVisible();
    await expect(a.page.locator(".bell")).toHaveCount(0);
    expect(peer.seen(bob.fid)).toBe(0);
    expect(peer.seen(alice.fid)).toBe(at - 180);
    expect(peer.calls.filter(call => call.label === "switch" && call.name === "verify_notification_fid" && call.fid === alice.fid)).toEqual([]);
    expect(peer.calls.filter(call => call.label === "switch" && call.name === "mark_notifications_seen")).toEqual([]);
    await expect(bell(b.page)).toHaveAccessibleName("Notifications, 2 unread");
    await noMessageSideEffects(peer, [a.page, b.page]);
    expect([...a.errors, ...b.errors]).toEqual([]);
  } catch (error) { await diagnose(peer, a, b); throw error; }
  finally { a.releaseAttestation(); await close(a, b); }
});

test("signed-out pages make no notification-sync connection, attestation or message keys", async ({ browser }) => {
  const peer = new Peer();
  const a = await reader(browser, peer, "signed-out", null);
  let unconfigured: Awaited<ReturnType<typeof reader>> | undefined;
  try {
    await a.page.goto("/notifications");
    await appCame(a.page);
    await expect(a.page.getByText("Sign in to see your notifications.")).toBeVisible();
    expect(peer.connections).toHaveLength(0);
    expect(a.state.attestations).toEqual([]);
    expect(a.state.calls).toBe(0);
    expect(a.state.status).toBe(0);
    await noMessageKeys([a.page]);
    expect(a.errors).toEqual([]);
    // A signed-in browser on an unconfigured deployment still reads locally;
    // the status probe must stop before SDK loading, token storage or sockets.
    unconfigured = await reader(browser, peer, "unconfigured", alice, { configured: false });
    await unconfigured.page.goto("/notifications");
    await expect(rows(unconfigured.page)).toHaveCount(2);
    await expect(bell(unconfigured.page)).toHaveAccessibleName("Notifications, 0 unread");
    await expect.poll(() => unconfigured!.state.status).toBe(1);
    expect(peer.connections).toHaveLength(0);
    expect(unconfigured.state.attestations).toEqual([]);
    expect(unconfigured.state.calls).toBe(1);
    await expect(unconfigured.page.locator("script[data-stdb]")).toHaveCount(0);
    await noMessageKeys([unconfigured.page]);
    expect(await unconfigured.page.evaluate(async () => (await indexedDB.databases()).map(db => db.name))).not.toContain("allthing-notification-sync");
    expect(unconfigured.errors).toEqual([]);
  } catch (error) { await diagnose(peer, a, ...(unconfigured ? [unconfigured] : [])); throw error; }
  finally { await close(a, ...(unconfigured ? [unconfigured] : [])); }
});
