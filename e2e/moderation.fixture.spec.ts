// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type WebSocketRoute } from "@playwright/test";
import { buildSync } from "esbuild";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { createRequire } from "node:module";
import { join, resolve } from "node:path";
import { appCame } from "./helpers/app";
import { expectDraft } from "./helpers/composer";
import { kept, signIn } from "./helpers/farcaster";

// The browser uses the real Rust state/filter, IndexedDB and SDK/generated
// BSATN codecs. Independent contexts share only this controlled service. This
// peer implements field CAS, not the app's content matching or outbox logic.
test.skip(Boolean(process.env.E2E_PAGES), "the public static copy has no private moderation");
const temporary = mkdtempSync(join(tmpdir(), "allthing-moderation-wire-"));
buildSync({ entryPoints: [resolve("e2e/fixtures/moderation-sync-wire.ts")], outfile: join(temporary, "wire.cjs"), bundle: true, platform: "node", format: "cjs", logLevel: "silent" });
const wire = createRequire(resolve("package.json"))(join(temporary, "wire.cjs"));
test.afterAll(() => rmSync(temporary, { recursive: true, force: true }));

const alice = { fid: 1234, username: "reader", display_name: "Reader" };
const bob = { fid: 5678, username: "second", display_name: "Second Reader" };
const hush = { fid: 88, username: "hush", display_name: "Hidden Account" };
const safe = { fid: 99, username: "ordinary", display_name: "Ordinary Account" };
const at = Math.floor(Date.now() / 1000);
const hash = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const cast = (n: number, text: string, author = safe, extra: object = {}) => ({ hash: hash(n), author, text, ts: at - 120, likes: 0, recasts: 0, replies: 0, ...extra });
const meta = () => ({ backend: "hypersnap", fetched_at: Date.now() });
const key = (value: unknown) => JSON.stringify(value, (_key, value) => typeof value === "bigint" ? value.toString() : value);
type Filter = { key: string; fid: number; targetFid: number; targetUsername: string; muted: boolean; blocked: boolean; muteRevision: number; blockRevision: number; updatedAt: number };
type Selection = { querySetId: unknown; queries: string[]; tables: { table: string; values: object[] }[] };
type Connection = { label: string; id: number; socket: WebSocketRoute; identity: string; active: boolean; fid?: number; selections: Map<string, Selection> };
type Call = { label: string; name: string; fid?: number; args: Record<string, any>; rejected?: string };

class Peer {
  filters = new Map<string, Filter>();
  tokens = new Map<string, string>();
  authorized = new Map<string, number>();
  connections: Connection[] = [];
  calls: Call[] = [];
  queries: string[] = [];
  frames: object[] = [];
  offline = new Set<string>();
  pausedPushes = new Set<string>();
  loseNextAck = new Set<string>();
  failNextWrite = new Set<string>();
  filter(fid: number, targetFid = hush.fid): Filter {
    return { ...(this.filters.get(`${fid}:${targetFid}`) ?? { key: `${fid}:${targetFid}`, fid, targetFid, targetUsername: hush.username, muted: false, blocked: false, muteRevision: 0, blockRevision: 0, updatedAt: 0 }) };
  }
  seed(fid: number, { muted = false, blocked = false, target = hush } = {}) {
    const value = this.filter(fid, target.fid);
    value.targetUsername = target.username;
    value.muted = muted; value.blocked = blocked;
    if (muted) value.muteRevision++;
    if (blocked) value.blockRevision++;
    value.updatedAt = at;
    this.filters.set(value.key, value);
    this.broadcast();
  }
  verified(label: string) { return this.connections.some(connection => connection.active && connection.label === label && connection.fid !== undefined); }
  select(connection: Connection, query: string): { table: string; values: object[] } {
    const match = /^\s*select\s+\*\s+from\s+(my_[a-z_]+)\s*;?\s*$/i.exec(query);
    if (!match) throw new Error(`Unexpected private SQL: ${query}`);
    const table = match[1].toLowerCase();
    if (table === "my_notification_state") return { table, values: [] };
    if (table !== "my_account_filters") throw new Error(`Unexpected moderation view ${query}`);
    return { table, values: [...this.filters.values()].filter(value => value.fid === connection.fid).map(value => ({ ...value, fid: BigInt(value.fid), targetFid: BigInt(value.targetFid), muteRevision: BigInt(value.muteRevision), blockRevision: BigInt(value.blockRevision), updatedAt: BigInt(value.updatedAt) })) };
  }
  broadcast() {
    for (const connection of this.connections.filter(connection => connection.active && !this.pausedPushes.has(connection.label))) {
      for (const selection of connection.selections.values()) {
        const next = selection.queries.map(query => this.select(connection, query));
        const changed = next.map((table, index) => ({ table: table.table, before: selection.tables[index]?.values ?? [], after: table.values })).filter(table => key(table.before) !== key(table.after));
        if (changed.length) {
          this.frames.push(JSON.parse(key({ label: connection.label, phase: "update", tables: changed })));
          connection.socket.send(wire.update(selection.querySetId, changed));
        }
        selection.tables = next;
      }
    }
  }
  mutate(connection: Connection, name: string, args: Record<string, any>) {
    if (connection.fid === undefined) throw new Error("Verify the reader first");
    if (name === "mark_notifications_seen") return;
    if (name !== "set_account_filter") throw new Error(`Unexpected mutation ${name}`);
    if (this.failNextWrite.delete(connection.label)) throw new Error("Fixture preference save unavailable");
    const targetFid = Number(args.targetFid), kind = Number(args.kind), desired = Boolean(args.desired);
    if (targetFid <= 0 || targetFid === connection.fid || ![0, 1].includes(kind)) throw new Error("Invalid target or kind");
    const value = this.filter(connection.fid, targetFid);
    const field = kind === 0 ? "muted" : "blocked", revision = kind === 0 ? "muteRevision" : "blockRevision";
    if (value[field] === desired && value[revision] > 0) return;
    if (Number(args.expectedRevision) !== value[revision]) throw new Error("account filter revision changed");
    value[field] = desired; value[revision]++; value.targetUsername = args.targetUsername;
    value.updatedAt = Math.floor(Date.now() / 1000);
    this.filters.set(value.key, value);
  }
  async attach(page: Page, label: string) {
    await page.route("https://maincloud.spacetimedb.com/**", async route => {
      const headers = { "access-control-allow-origin": "*", "access-control-allow-headers": "authorization", "access-control-allow-methods": "POST, OPTIONS" };
      if (route.request().method() === "OPTIONS") return route.fulfill({ status: 204, headers });
      const token = (await route.request().headerValue("authorization"))?.replace(/^Bearer /, "");
      if (new URL(route.request().url()).pathname !== "/v1/identity/websocket-token" || !token || !this.tokens.has(token)) return route.fulfill({ status: 401, json: { error: "Unknown fixture token" }, headers });
      return this.offline.has(label) ? route.fulfill({ status: 503, json: { error: "Fixture offline" }, headers }) : route.fulfill({ json: { token }, headers });
    });
    await page.routeWebSocket("wss://maincloud.spacetimedb.com/**", socket => {
      if (this.offline.has(label)) { void socket.close({ code: 1013, reason: "Fixture offline" }); return; }
      const requested = new URL(socket.url()).searchParams.get("token");
      const token = requested && this.tokens.has(requested) ? requested : `moderation-token-${this.tokens.size + 1}`;
      let identity = this.tokens.get(token);
      if (!identity) { identity = `c4${(this.tokens.size + 1).toString(16).padStart(62, "0")}`; this.tokens.set(token, identity); }
      const connection: Connection = { id: this.connections.length + 1, label, socket, identity, active: true, selections: new Map() };
      this.connections.push(connection);
      socket.onClose(() => { connection.active = false; });
      socket.onMessage(bytes => {
        const message = wire.client(bytes);
        if (message.tag === "Subscribe") {
          const selection = { querySetId: message.value.querySetId, queries: message.value.queryStrings, tables: [] } as Selection;
          this.queries.push(...selection.queries);
          selection.tables = selection.queries.map(query => this.select(connection, query));
          connection.selections.set(key(selection.querySetId), selection);
          this.frames.push(JSON.parse(key({ label, phase: "snapshot", tables: selection.tables })));
          socket.send(wire.snapshot(message.value.requestId, selection.querySetId, selection.tables));
        } else if (message.tag === "Unsubscribe") {
          const selection = connection.selections.get(key(message.value.querySetId));
          connection.selections.delete(key(message.value.querySetId));
          socket.send(wire.unsubscribed(message.value.requestId, message.value.querySetId, selection?.tables ?? []));
        } else if (message.tag === "CallReducer") {
          const { reducer: name, args, requestId } = message.value;
          const call: Call = { label, name, fid: connection.fid, args: {} }; this.calls.push(call);
          try {
            const decoded = wire.reducer(name, args); call.args = JSON.parse(key(decoded));
            if (name === "verify_notification_fid") {
              const fid = Number(decoded.fid); call.fid = fid;
              if (this.authorized.get(connection.identity) !== fid || (connection.fid !== undefined && connection.fid !== fid)) throw new Error("Attestation FID mismatch");
              connection.fid = fid;
            } else this.mutate(connection, name, decoded);
            if (name === "set_account_filter" && this.loseNextAck.delete(label)) {
              this.offline.add(label); this.pausedPushes.add(label); connection.active = false;
              void socket.close({ code: 1013, reason: "Committed, acknowledgement lost" });
              this.broadcast();
            } else { this.broadcast(); socket.send(wire.ok(requestId)); }
          } catch (error) { call.rejected = String(error); socket.send(wire.error(requestId, String(error))); }
        } else throw new Error(`Unexpected moderation protocol ${message.tag}`);
      });
      socket.send(wire.initial(identity, token, this.connections.length));
    });
  }
  async disconnect(label: string) {
    this.offline.add(label);
    await Promise.all(this.connections.filter(connection => connection.label === label && connection.active).map(async connection => { connection.active = false; await connection.socket.close({ code: 1013, reason: "Fixture offline" }); }));
  }
}

function cookieFid(cookie: string) {
  const value = /(?:^|;\s*)__Host-allthing-user=([^;]*)/.exec(cookie)?.[1];
  try { return value ? Number(JSON.parse(decodeURIComponent(value)).fid) : 0; } catch { return 0; }
}
async function reader(browser: Browser, peer: Peer, label: string, who: typeof alice | null, options: { pool?: ReturnType<typeof cast>[]; quotes?: object; groups?: object[]; holdAttestation?: number; touch?: boolean } = {}) {
  const context = await browser.newContext({ viewport: options.touch ? { width: 390, height: 844 } : { width: 1280, height: 900 }, hasTouch: options.touch });
  const page = await context.newPage(), errors: string[] = [], publicWrites: string[] = [], attest: number[] = [];
  page.on("pageerror", error => errors.push(error.message));
  page.on("request", request => { if (request.method() === "POST" && /submitMessage|auth\/(?:signer|key|start)/.test(request.url())) publicWrites.push(request.url()); });
  await page.addInitScript(() => {
    Object.defineProperty(Document.prototype, "hidden", { configurable: true, get: () => false });
    Object.defineProperty(Document.prototype, "visibilityState", { configurable: true, get: () => "visible" });
    (window as any).__moderationPaint = [];
    (window as any).__moderationLeaks = [];
    new MutationObserver(() => {
      for (const card of document.querySelectorAll("#boot .cast-card[data-hash], #app .cast-card[data-hash], #app .post[data-hash], #app .comment[data-hash]")) {
        const value = card.getAttribute("data-hash")!;
        if (!(window as any).__moderationPaint.includes(value)) (window as any).__moderationPaint.push(value);
      }
      const text = `${document.querySelector("#app")?.textContent ?? ""} ${document.querySelector("#boot")?.textContent ?? ""}`;
      for (const sentinel of ["A hidden reply sentinel", "A blocked root sentinel"]) {
        if (text.includes(sentinel) && !(window as any).__moderationLeaks.includes(sentinel)) (window as any).__moderationLeaks.push(sentinel);
      }
    }).observe(document, { childList: true, subtree: true });
  });
  if (who) await signIn(page, who);
  let release!: () => void; const held = new Promise<void>(resolve => { release = resolve; });
  const pool = options.pool ?? [cast(1, "A safe public post"), cast(2, "A hidden author's public post", hush)];
  await page.route("**/api/v1/**", route => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/status", route => route.fulfill({ json: { spacetime: { attest: true, library: false, moderation: true }, writes: { ready: true } } }));
  for (const endpoint of ["home", "latest", "following"]) await page.route(`**/api/v1/${endpoint}**`, route => route.fulfill({ json: { pool, cursors: [], notes: { quotes: options.quotes ?? {} }, meta: meta() } }));
  await page.route("**/api/v1/notifications**", route => route.fulfill({ json: { groups: options.groups ?? [], newest: at - 1, notes: {}, meta: meta() } }));
  await page.route("**/api/v1/user?*", route => {
    const name = new URL(route.request().url()).searchParams.get("name")?.replace(/^@/, "");
    const user = [alice, bob, hush, safe].find(person => person.username === name) ?? hush;
    return route.fulfill({ json: { user: { ...user, bio: "An account profile", follower_count: 1, following_count: 1 }, pool: [], notes: {}, meta: meta() } });
  });
  await page.route("**/api/v1/cast?*", route => {
    const id = new URL(route.request().url()).searchParams.get("hash");
    const root = pool.find(value => value.hash === id) ?? cast(50, "An ordinary conversation root");
    return route.fulfill({ json: { thread: { cast: root, children: [{ cast: cast(51, "A hidden reply sentinel", hush), children: [] }, { cast: cast(52, "An ordinary reply sentinel"), children: [] }] }, ancestors: [], notes: { quotes: options.quotes ?? {} }, meta: meta() } });
  });
  await page.route("**/api/v1/auth/attest", async route => {
    const fid = cookieFid(await route.request().headerValue("cookie") ?? ""); attest.push(fid);
    if (fid === options.holdAttestation) await held;
    const { identity } = route.request().postDataJSON(); peer.authorized.set(identity, fid);
    return route.fulfill({ json: { identity, fid, expiry: Math.floor(Date.now() / 1000) + 600, signature: "11".repeat(64) } });
  });
  await page.route("https://haatz.quilibrium.com/**", route => route.fulfill({ headers: { "access-control-allow-origin": "*" }, json: { messages: [] } }));
  await peer.attach(page, label);
  return { context, page, errors, publicWrites, attest, releaseAttestation: release };
}
type Reader = Awaited<ReturnType<typeof reader>>;
const cards = (page: Page) => page.locator("#app .cast-card[data-hash]");
const card = (page: Page, n: number) => page.locator(`#app .cast-card[data-hash="${hash(n)}"]`);
const profileControls = (page: Page) => page.locator(".profile .account-controls");
async function profile(value: Reader, username = hush.username) {
  await value.page.goto(`/u/${username}`);
  await expect(value.page.locator(".profile .profile-name")).toBeVisible();
}
async function ready(peer: Peer, value: Reader, label: string) {
  await expect.poll(() => peer.verified(label)).toBe(true);
  await expect.poll(() => peer.frames.some((frame: any) => frame.label === label && frame.phase === "snapshot" && frame.tables.some((table: any) => table.table === "my_account_filters"))).toBe(true);
  await expect.poll(() => value.page.locator(".account-controls-status").allTextContents()).not.toContain("Loading private preferences…");
}
async function management(page: Page) {
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.locator(".account-management > summary").click();
  return page.locator(".account-management");
}
async function reconnect(peer: Peer, value: Reader, label: string) {
  peer.offline.delete(label); peer.pausedPushes.delete(label);
  await value.page.evaluate(() => { window.dispatchEvent(new Event("online")); window.dispatchEvent(new Event("focus")); });
}
async function switchReader(page: Page, user: typeof alice | null) {
  await page.evaluate(user => {
    document.cookie = user ? `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax` : "__Host-allthing-user=; Path=/; Secure; Max-Age=0";
    const channel = new BroadcastChannel("allthing-session"); channel.postMessage("changed"); channel.close();
  }, user);
}
async function finish(peer: Peer, ...values: Reader[]) {
  await test.info().attach("moderation-diagnostics", { contentType: "application/json", body: JSON.stringify({ filters: [...peer.filters.values()], calls: peer.calls, queries: peer.queries, frames: peer.frames.slice(-100), readers: await Promise.all(values.map(async value => ({ errors: value.errors, publicWrites: value.publicWrites, url: value.page.url(), text: await value.page.locator("#app").innerText().catch(() => "unavailable") }))) }, null, 2) });
  await Promise.all(values.map(value => value.context.close()));
}

test("a fresh session never paints muted accounts, mentions, quotes, reply provenance or recasts before private sync", async ({ browser }) => {
  const peer = new Peer(); peer.seed(alice.fid, { muted: true });
  const hidden = [
    cast(2, "Hidden author sentinel", hush),
    cast(3, "Mention sentinel @hush", safe, { mentions: [{ fid: hush.fid, username: hush.username }] }),
    cast(4, "Quote sentinel", safe, { embeds: [{ kind: "cast", hash: hash(20) }] }),
    cast(5, "Reply provenance sentinel", safe, { parent_hash: hash(30), references: { fids: [hush.fid] } }),
    cast(6, "Recast provenance sentinel", safe, { references: { fids: [hush.fid] } }),
    cast(7, "Account link sentinel https://farcaster.xyz/hush"),
  ];
  const a = await reader(browser, peer, "a", alice, { pool: [cast(1, "An unrelated public post"), ...hidden], quotes: { [hash(20)]: cast(20, "Hidden quoted sentinel", hush) }, holdAttestation: alice.fid });
  try {
    await a.page.goto("/");
    await expect.poll(() => a.attest.includes(alice.fid)).toBe(true);
    await expect(cards(a.page)).toHaveCount(0);
    await expect(a.page.locator("#boot .cast-card")).toHaveCount(0);
    expect(await a.page.evaluate(() => (window as any).__moderationPaint)).toEqual([]);
    a.releaseAttestation();
    await expect(card(a.page, 1)).toBeVisible();
    await expect(cards(a.page)).toHaveCount(1);
    expect(await a.page.evaluate(() => (window as any).__moderationPaint)).toEqual([hash(1)]);
    await a.page.goto("/?feed=following&sort=new");
    for (const value of hidden) await expect(a.page.locator(`#app .cast-card[data-hash="${value.hash}"]`)).toHaveCount(0);
    expect((await a.page.evaluate(() => (window as any).__moderationPaint) as string[]).some(value => hidden.some(item => item.hash === value))).toBe(false);
    expect(a.publicWrites).toEqual([]); expect(await kept(a.page, alice.fid)).toBeNull(); expect(a.errors).toEqual([]);
  } finally { await finish(peer, a); }
});

test("blocked identities are absent from notification rows and unread counts, including beyond the visible actor list and excerpt", async ({ browser }) => {
  const peer = new Peer(); peer.seed(alice.fid, { blocked: true });
  const groups = [
    { kind: "follow", actors: [hush], count: 1, ts: at - 1 },
    { kind: "like", actors: [safe], count: 6, ts: at - 2, cast: { hash: hash(10), excerpt: "An ordinary shortened excerpt" }, references: { fids: [safe.fid, hush.fid] } },
    { kind: "mention", actors: [safe], count: 1, ts: at - 3, cast: { hash: hash(11), excerpt: "A longer post whose late mention is beyond the excerpt" }, references: { fids: [hush.fid] } },
    { kind: "follow", actors: [safe], count: 1, ts: at - 4, references: { fids: [safe.fid] } },
  ];
  const a = await reader(browser, peer, "a", alice, { groups });
  try {
    await a.page.goto("/"); await ready(peer, a, "a");
    await expect(a.page.getByRole("link", { name: "Notifications, 1 unread", exact: true })).toBeVisible();
    await a.page.goto("/notifications");
    await expect(a.page.locator(".notif-list .notif")).toHaveCount(1);
    await expect(a.page.locator(".notif-list")).toContainText("Ordinary Account followed you");
    await expect(a.page.locator(".notif-list")).not.toContainText("Hidden Account");
    await expect(a.page.locator(".notif-list")).not.toContainText("ordinary shortened excerpt");
    await expect(a.page.locator(".notif-list")).not.toContainText("late mention");
    expect(a.errors).toEqual([]);
  } finally { await finish(peer, a); }
});

test("a remote mute removes already drawn posts and Settings can unmute it from an independent session", async ({ browser }) => {
  const peer = new Peer();
  const a = await reader(browser, peer, "a", alice), b = await reader(browser, peer, "b", alice);
  try {
    await profile(a); await ready(peer, a, "a");
    await b.page.goto("/"); await expect(card(b.page, 2)).toBeVisible(); await ready(peer, b, "b");
    await profileControls(a.page).getByRole("button", { name: "Mute", exact: true }).click();
    await expect.poll(() => peer.filter(alice.fid).muted).toBe(true);
    await expect(card(b.page, 2)).toHaveCount(0);
    const manage = await management(b.page);
    await expect(manage.locator(".account-management-list")).toContainText("@hush");
    await manage.getByRole("searchbox", { name: "Find an account", exact: true }).fill("no-match");
    await expect(manage.getByText("No matching accounts.", { exact: true })).toBeVisible();
    await manage.getByRole("searchbox", { name: "Find an account", exact: true }).fill("88");
    await manage.getByRole("button", { name: "Unmute @hush", exact: true }).click();
    await expect.poll(() => peer.filter(alice.fid).muted).toBe(false);
    await expect(profileControls(a.page).getByRole("button", { name: "Mute", exact: true })).toBeEnabled();
    await expect(card(b.page, 2)).toBeVisible();
    expect(peer.filter(alice.fid).muteRevision).toBe(2);
    expect(a.publicWrites).toEqual([]); expect(b.publicWrites).toEqual([]); expect(a.errors).toEqual([]); expect(b.errors).toEqual([]);
  } finally { await finish(peer, a, b); }
});

test("concurrent mute and block use independent revisions and Undo leaves the other choice intact", async ({ browser }) => {
  const peer = new Peer();
  const a = await reader(browser, peer, "a", alice), b = await reader(browser, peer, "b", alice);
  try {
    await Promise.all([profile(a), profile(b)]); await ready(peer, a, "a"); await ready(peer, b, "b");
    peer.pausedPushes.add("a"); peer.pausedPushes.add("b");
    await Promise.all([profileControls(a.page).getByRole("button", { name: "Mute", exact: true }).click(), profileControls(b.page).getByRole("button", { name: "Block", exact: true }).click()]);
    await expect.poll(() => [peer.filter(alice.fid).muted, peer.filter(alice.fid).blocked]).toEqual([true, true]);
    expect([peer.filter(alice.fid).muteRevision, peer.filter(alice.fid).blockRevision]).toEqual([1, 1]);
    peer.pausedPushes.clear(); peer.broadcast();
    await expect(profileControls(a.page).getByRole("button", { name: "Unmute", exact: true })).toBeEnabled();
    await expect(profileControls(a.page).getByRole("button", { name: "Unblock", exact: true })).toBeEnabled();
    await profileControls(a.page).getByRole("button", { name: "Unmute", exact: true }).click();
    await expect.poll(() => [peer.filter(alice.fid).muted, peer.filter(alice.fid).blocked]).toEqual([false, true]);
    await expect(profileControls(b.page).getByRole("button", { name: "Mute", exact: true })).toBeEnabled();
    await profileControls(b.page).getByRole("button", { name: "Unblock", exact: true }).click();
    await expect.poll(() => [peer.filter(alice.fid).muted, peer.filter(alice.fid).blocked]).toEqual([false, false]);
    expect(peer.calls.filter(call => call.name === "set_account_filter" && call.rejected)).toEqual([]);
    expect(a.errors).toEqual([]); expect(b.errors).toEqual([]);
  } finally { await finish(peer, a, b); }
});

test("an acknowledged remote unmute survives an older mute whose commit acknowledgement was lost offline", async ({ browser }) => {
  const peer = new Peer();
  const a = await reader(browser, peer, "a", alice), b = await reader(browser, peer, "b", alice);
  try {
    await profile(a); await ready(peer, a, "a");
    peer.loseNextAck.add("a");
    await profileControls(a.page).getByRole("button", { name: "Mute", exact: true }).click();
    await expect.poll(() => [peer.offline.has("a"), peer.filter(alice.fid).muted]).toEqual([true, true]);
    await profile(b); await ready(peer, b, "b");
    await expect(profileControls(b.page).getByRole("button", { name: "Unmute", exact: true })).toBeEnabled();
    await profileControls(b.page).getByRole("button", { name: "Unmute", exact: true }).click();
    await expect.poll(() => peer.filter(alice.fid).muteRevision).toBe(2);
    await reconnect(peer, a, "a");
    await expect(profileControls(a.page).getByRole("button", { name: "Mute", exact: true })).toBeEnabled();
    expect(peer.filter(alice.fid).muted).toBe(false);
    expect(peer.filter(alice.fid).muteRevision).toBe(2);
    expect(a.errors).toEqual([]); expect(b.errors).toEqual([]);
  } finally { await finish(peer, a, b); }
});

test("an offline choice survives reload and is delivered when the private connection returns", async ({ browser }) => {
  const peer = new Peer(), a = await reader(browser, peer, "a", alice);
  try {
    await profile(a); await ready(peer, a, "a");
    await peer.disconnect("a");
    await profileControls(a.page).getByRole("button", { name: "Mute", exact: true }).click();
    await expect(profileControls(a.page).getByRole("button", { name: "Unmute", exact: true })).toHaveAttribute("aria-pressed", "true");
    await a.page.reload();
    await expect(profileControls(a.page).getByRole("button", { name: "Unmute", exact: true })).toHaveAttribute("aria-pressed", "true");
    expect(peer.filter(alice.fid).muted).toBe(false);
    await reconnect(peer, a, "a");
    await expect.poll(() => peer.filter(alice.fid).muted).toBe(true);
    await expect(profileControls(a.page).getByRole("button", { name: "Unmute", exact: true })).toBeEnabled();
    expect(a.errors).toEqual([]);
  } finally { await finish(peer, a); }
});

test("a failed preference write is explained and Retry sync preserves the requested choice", async ({ browser }) => {
  const peer = new Peer(), a = await reader(browser, peer, "a", alice);
  try {
    await profile(a); await ready(peer, a, "a");
    peer.failNextWrite.add("a");
    await profileControls(a.page).getByRole("button", { name: "Block", exact: true }).click();
    await expect(profileControls(a.page).getByRole("status")).toContainText("not synced");
    await profileControls(a.page).getByRole("button", { name: "Retry sync", exact: true }).click();
    await expect.poll(() => peer.filter(alice.fid).blocked).toBe(true);
    await expect(profileControls(a.page).getByRole("button", { name: "Unblock", exact: true })).toBeEnabled();
    expect(peer.calls.filter(call => call.name === "set_account_filter").map(call => Boolean(call.args.desired))).toEqual([true, true]);
    expect(a.errors).toEqual([]);
  } finally { await finish(peer, a); }
});

test("changing accounts cannot expose or apply the previous reader's private preferences", async ({ browser }) => {
  const peer = new Peer(); peer.seed(alice.fid, { muted: true });
  const a = await reader(browser, peer, "a", alice, { holdAttestation: bob.fid });
  try {
    await profile(a); await ready(peer, a, "a");
    await expect(profileControls(a.page).getByRole("button", { name: "Unmute", exact: true })).toBeEnabled();
    await switchReader(a.page, bob);
    await expect.poll(() => a.attest.includes(bob.fid)).toBe(true);
    await expect(profileControls(a.page).getByRole("button", { name: "Unmute", exact: true })).toHaveCount(0);
    a.releaseAttestation();
    await expect(profileControls(a.page).getByRole("button", { name: "Mute", exact: true })).toBeEnabled();
    await profileControls(a.page).getByRole("button", { name: "Block", exact: true }).click();
    await expect.poll(() => peer.filter(bob.fid).blocked).toBe(true);
    expect([peer.filter(alice.fid).muted, peer.filter(alice.fid).blocked]).toEqual([true, false]);
    await switchReader(a.page, null);
    await expect(profileControls(a.page)).toHaveCount(0);
    await a.page.getByRole("button", { name: "Settings", exact: true }).click();
    await expect(a.page.locator(".account-management")).toHaveCount(0);
    expect(a.errors).toEqual([]);
  } finally { await finish(peer, a); }
});

test("profile controls require another signed-in account; help opens on keyboard focus and touch without changing preferences", async ({ browser }) => {
  const peer = new Peer(); peer.seed(alice.fid, { muted: true });
  const signedOut = await reader(browser, peer, "out", null), own = await reader(browser, peer, "own", alice), touch = await reader(browser, peer, "touch", alice, { touch: true });
  try {
    await profile(signedOut); await appCame(signedOut.page); await expect(profileControls(signedOut.page)).toHaveCount(0);
    await profile(own, alice.username); await expect(profileControls(own.page)).toHaveCount(0);
    await profile(own); await ready(peer, own, "own");
    await profileControls(own.page).getByRole("button", { name: "About mute and block", exact: true }).focus();
    const help = own.page.getByRole("dialog", { name: "About mute and block", exact: true });
    await expect(help).toBeVisible();
    await expect(help).toContainText("stops new Allthing direct messages in both directions");
    await expect(help).toContainText("do not delete public posts");
    await expect(help).toContainText("text in images");
    await own.page.keyboard.press("Escape"); await expect(help).toHaveCount(0);
    await own.page.goto("/");
    await expect(card(own.page, 1)).toBeVisible();
    await card(own.page, 1).locator(".user-chip .author").hover();
    const hoverCard = own.page.getByRole("dialog", { name: "Profile: Ordinary Account (@ordinary)", exact: true });
    await expect(hoverCard.getByRole("button", { name: "Mute", exact: true })).toBeVisible();
    await expect(hoverCard.getByRole("button", { name: "Block", exact: true })).toBeVisible();
    await own.page.keyboard.press("Escape");
    const settings = await management(own.page);
    await expect(settings.getByRole("button", { name: "Unmute @hush", exact: true })).toBeVisible();
    await own.page.getByRole("button", { name: "Write a cast", exact: true }).click();
    await expect(own.page.getByRole("dialog", { name: "New cast", exact: true })).toBeVisible();
    await expect(own.page.getByRole("dialog", { name: "Settings", exact: true })).toBeHidden();
    await own.page.keyboard.press("Escape");
    await profile(touch); await ready(peer, touch, "touch");
    await profileControls(touch.page).getByRole("button", { name: "About mute and block", exact: true }).tap();
    await expect(touch.page.getByRole("dialog", { name: "About mute and block", exact: true })).toBeVisible();
    expect(await touch.page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await touch.page.screenshot({ path: test.info().outputPath("moderation-help-phone390.png"), fullPage: true });
    expect(peer.calls.filter(call => call.name === "set_account_filter")).toEqual([]);
    expect(signedOut.errors).toEqual([]); expect(own.errors).toEqual([]); expect(touch.errors).toEqual([]);
  } finally { await finish(peer, signedOut, own, touch); }
});

test("opening a conversation directly cannot reveal a hidden reply or a blocked root", async ({ browser }) => {
  const peer = new Peer(); peer.seed(alice.fid, { blocked: true });
  const a = await reader(browser, peer, "a", alice, { pool: [cast(2, "A blocked root sentinel", hush)] });
  try {
    await a.page.goto(`/p/${hash(50)}`);
    await expect(a.page.locator("#app")).toContainText("An ordinary conversation root");
    await expect(a.page.locator("#app")).toContainText("An ordinary reply sentinel");
    await expect(a.page.locator("#app")).not.toContainText("A hidden reply sentinel");
    expect(await a.page.evaluate(() => (window as any).__moderationLeaks)).toEqual([]);
    await a.page.goto(`/p/${hash(2)}`);
    await expect.poll(() => peer.verified("a")).toBe(true);
    await expect(a.page.locator("#app .post")).toHaveCount(0);
    await expect(a.page.locator("#app")).not.toContainText("A blocked root sentinel");
    expect(await a.page.evaluate(() => (window as any).__moderationLeaks)).toEqual([]);
    expect(a.errors).toEqual([]);
  } finally { await finish(peer, a); }
});

test("a remote mute closes an open media viewer using the full post identities beyond its shortened caption", async ({ browser }) => {
  const peer = new Peer();
  const image = "https://media.example/moderation-viewer.png";
  const text = `Lightbox private sentinel. ${"A quiet afternoon by the water. ".repeat(9)}A late mention of @hush.`;
  const pool = [cast(70, text, safe, { mentions: [{ fid: hush.fid, username: hush.username }], embeds: [{ kind: "image", url: image }] })];
  const a = await reader(browser, peer, "a", alice, { pool }), b = await reader(browser, peer, "b", alice, { pool });
  try {
    for (const value of [a, b]) await value.page.route(image, route => route.fulfill({ contentType: "image/png", body: Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aP1cAAAAASUVORK5CYII=", "base64") }));
    await a.page.goto(`/p/${hash(70)}`); await ready(peer, a, "a");
    await a.page.locator(".post .tile-image").click();
    const viewer = a.page.getByRole("dialog", { name: "Media viewer", exact: true });
    await expect(viewer).toBeVisible();
    await expect(viewer.locator(".lb-caption")).toContainText("Lightbox private sentinel");
    await expect(viewer.locator(".lb-caption")).not.toContainText("@hush");
    await expect(viewer.locator("img.lb-media")).not.toHaveAttribute("alt", /@hush/);
    peer.seed(alice.fid, { muted: true });
    await expect(viewer).toHaveCount(0);
    await expect(a.page.locator(".lb-media, .lb-caption")).toHaveCount(0);
    await expect(a.page.locator(`img[src="${image}"]`)).toHaveCount(0);
    await expect(a.page.locator("#app")).not.toContainText("Lightbox private sentinel");
    await expect.poll(() => a.page.evaluate(() => getComputedStyle(document.documentElement).overflowY)).not.toBe("hidden");
    await b.page.goto(`/p/${hash(70)}`); await ready(peer, b, "b");
    await expect(b.page.locator(".post .tile-image, .lightbox")).toHaveCount(0);
    await expect(b.page.locator("#app")).not.toContainText("Lightbox private sentinel");
    expect(a.errors).toEqual([]); expect(b.errors).toEqual([]);
  } finally { await finish(peer, a, b); }
});

for (const kind of ["Reply", "Quote"] as const) {
  test(`a remote mute hides detached ${kind} target context without losing the saved draft`, async ({ browser }) => {
    const peer = new Peer();
    const image = "https://media.example/moderation-composer.png";
    const text = `Composer target sentinel. ${"Ordinary words before a distant mention. ".repeat(9)}@hush`;
    const pool = [cast(80, text, safe, { mentions: [{ fid: hush.fid, username: hush.username }], embeds: [{ kind: "image", url: image }] })];
    const a = await reader(browser, peer, "a", alice, { pool });
    const ownDraft = `My private ${kind.toLowerCase()} draft stays mine.`;
    try {
      await a.page.route(image, route => route.fulfill({ contentType: "image/png", body: Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aP1cAAAAASUVORK5CYII=", "base64") }));
      await a.page.goto(`/p/${hash(80)}`); await ready(peer, a, "a");
      const action = kind === "Reply" ? "Reply to @ordinary" : "Quote @ordinary's cast";
      await a.page.locator(".post").getByRole("button", { name: action, exact: true }).click();
      const dialog = a.page.getByRole("dialog", { name: kind, exact: true });
      const box = dialog.getByRole("textbox", { name: kind === "Reply" ? "Cast your reply" : "Add your thoughts", exact: true });
      const context = dialog.locator(kind === "Reply" ? ".reply-parent" : ".compose-context");
      await expect(context).toContainText("@ordinary");
      await expect(context).toContainText("Composer target sentinel");
      await box.fill(ownDraft);
      await expectDraft(box, ownDraft);
      await expect.poll(() => a.page.evaluate(text => {
        const saved = JSON.parse(localStorage.getItem("allthing-drafts") ?? "{\"drafts\":[]}");
        return saved.drafts.some((row: any) => row.draft.text === text);
      }, ownDraft)).toBe(true);
      peer.seed(alice.fid, { muted: true });
      await expect(context).toContainText("Referenced post hidden by your account preferences. Your draft is unchanged.");
      await expect(context).not.toContainText("@ordinary");
      await expect(context).not.toContainText("Composer target sentinel");
      await expect(context.locator("img")).toHaveCount(0);
      if (kind === "Reply") {
        await expect(dialog.locator("#compose-replying")).not.toContainText("@ordinary");
        await expect(dialog.locator("#compose-replying")).not.toContainText("Composer target sentinel");
      }
      await expectDraft(box, ownDraft);
      await a.page.keyboard.press("Escape");
      await a.page.goto("/notifications"); await a.page.reload();
      await a.page.getByRole("button", { name: "Write a cast", exact: true }).click();
      const fresh = a.page.getByRole("dialog", { name: "New cast", exact: true });
      await fresh.getByRole("tab", { name: /Drafts/ }).click();
      const item = fresh.locator(".draft-item");
      await expect(item).toHaveCount(1);
      await expect(item.locator(".draft-label")).toContainText(`${kind} draft`);
      await expect(item.locator(".draft-label")).not.toContainText("@ordinary");
      await expect(item.locator(".draft-snippet")).toHaveText(ownDraft);
      await item.locator(".draft-open").click();
      await expectDraft(box, ownDraft);
      await expect(context).not.toContainText("Composer target sentinel");
      await expect(context).not.toContainText("@ordinary");
      expect(a.publicWrites).toEqual([]); expect(a.errors).toEqual([]);
    } finally { await finish(peer, a); }
  });
}
