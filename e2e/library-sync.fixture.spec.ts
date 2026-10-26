// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type BrowserContext, type Page, type WebSocketRoute } from "@playwright/test";
import { buildSync } from "esbuild";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { createRequire } from "node:module";
import { join, resolve } from "node:path";
import { kept, signIn } from "./helpers/farcaster";

// Independent browser contexts share only this controlled authenticated service.
// Rust/controller, SDK, generated BSATN codecs, DOM dwell and storage are real.
// Real module authorization/index/transaction behavior is tested separately.
test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no private Library");
const temporary = mkdtempSync(join(tmpdir(), "allthing-library-wire-"));
buildSync({ entryPoints: [resolve("e2e/fixtures/library-sync-wire.ts")], outfile: join(temporary, "wire.cjs"), bundle: true, platform: "node", format: "cjs", logLevel: "silent" });
const wire = createRequire(resolve("package.json"))(join(temporary, "wire.cjs"));
test.afterAll(() => rmSync(temporary, { recursive: true, force: true }));

const alice = { fid: 1234, username: "reader", display_name: "Reader" };
const bob = { fid: 5678, username: "other", display_name: "Other" };
const author = { fid: 88, username: "fixture", display_name: "Fixture" };
const at = Math.floor(Date.now() / 1000);
const hash = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const cast = (n: number) => ({ hash: hash(n), author, text: `Private Library fixture ${n}.`, ts: at - 120, likes: 1, recasts: 0, replies: 0 });
const meta = () => ({ backend: "hypersnap", fetched_at: Date.now() });
const key = (value: unknown) => JSON.stringify(value, (_key, value) => typeof value === "bigint" ? value.toString() : value);
type Account = { fid: number; generation: number; revision: number; clearedAt: number; lastClearId: string };
type Post = { hash: string; generation: number; seen: boolean; seenAt: number; seenRevision: number; bookmarked: boolean; bookmarkAt: number; bookmarkRevision: number };
type Selection = { querySetId: unknown; queries: string[]; tables: { table: string; values: object[] }[] };
type Connection = { id: number; label: string; socket: WebSocketRoute; identity: string; active: boolean; fid?: number; selections: Map<string, Selection> };
type Call = { label: string; name: string; fid?: number; args: Record<string, unknown>; rejected?: string };

class Peer {
  accounts = new Map<number, Account>();
  posts = new Map<string, Post>();
  tokens = new Map<string, string>();
  authorized = new Map<string, number>();
  connections: Connection[] = [];
  calls: Call[] = [];
  queries: string[] = [];
  frames: object[] = [];
  exchanges: string[] = [];
  offline = new Set<string>();
  // Independently delay unsolicited view pushes while requested snapshots still
  // use current state. Recovery must not depend on a later metadata push.
  pausedPushes = new Set<string>();
  holdSeen = new Set<string>();
  heldSeen = new Map<string, (() => void)[]>();
  releaseSeen(label: string) {
    this.holdSeen.delete(label);
    for (const acknowledge of this.heldSeen.get(label) ?? []) acknowledge();
    this.heldSeen.delete(label);
  }
  account(fid: number) {
    let value = this.accounts.get(fid);
    if (!value) { value = { fid, generation: 0, revision: 0, clearedAt: 0, lastClearId: "" }; this.accounts.set(fid, value); }
    return value;
  }
  post(fid: number, value: string): Post {
    const generation = this.account(fid).generation;
    const found = this.posts.get(`${fid}:${value}`);
    if (!found) return { hash: value, generation, seen: false, seenAt: 0, seenRevision: 0, bookmarked: false, bookmarkAt: 0, bookmarkRevision: 0 };
    return found.generation === generation ? { ...found } : { ...found, generation, seen: false, seenAt: 0, seenRevision: 0 };
  }
  save(fid: number, post: Post) { this.posts.set(`${fid}:${post.hash}`, { ...post }); }
  seedSeen(fid: number, n: number, time = at - 20) {
    const post = this.post(fid, hash(n)); post.seen = true; post.seenAt = time; post.seenRevision++;
    this.save(fid, post); this.account(fid).revision++;
  }
  seedBookmark(fid: number, n: number, time = at - 10) {
    const post = this.post(fid, hash(n)); post.bookmarked = true; post.bookmarkAt = time; post.bookmarkRevision++;
    this.save(fid, post); this.account(fid).revision++;
  }
  forFid(fid: number) { return [...this.posts].filter(([id]) => id.startsWith(`${fid}:`)).map(([, post]) => this.post(fid, post.hash)); }
  verified(label: string) { return this.connections.filter(connection => connection.active && connection.label === label && connection.fid !== undefined); }
  item(fid: number, post: Post, cursor = "") {
    return { fid: BigInt(fid), generation: BigInt(post.generation), hash: post.hash, seen: post.seen, seenAt: BigInt(post.seenAt), seenRevision: BigInt(post.seenRevision), bookmarked: post.bookmarked, bookmarkAt: BigInt(post.bookmarkAt), bookmarkRevision: BigInt(post.bookmarkRevision), cursor };
  }
  select(connection: Connection, query: string): { table: string; values: object[] } {
    const parsed = /^\s*select\s+\*\s+from\s+(my_[a-z_]+)(?:\((.*)\))?\s*;?\s*$/i.exec(query);
    if (!parsed) throw new Error(`Unexpected private SQL: ${query}`);
    const table = parsed[1].toLowerCase();
    if (connection.fid === undefined) return { table, values: [] };
    const fid = connection.fid, state = this.account(fid);
    if (table === "my_notification_state") return { table, values: [] };
    if (table === "my_library_state") return { table, values: [{ fid: BigInt(fid), generation: BigInt(state.generation), revision: BigInt(state.revision), seenCount: BigInt(this.forFid(fid).filter(post => post.seen).length), bookmarkCount: BigInt(this.forFid(fid).filter(post => post.bookmarked).length), clearedAt: BigInt(state.clearedAt), lastClearId: state.lastClearId }] };
    const args = [...(parsed[2] ?? "").matchAll(/'((?:''|[^'])*)'|(-?\d+)/g)].map(match => match[1] === undefined ? Number(match[2]) : match[1].replace(/''/g, "'"));
    if (table === "my_library_membership") {
      if (args.length !== 1 || typeof args[0] !== "string") throw new Error("Unexpected membership arguments");
      const hashes = [...new Set(args[0].split(","))];
      if (!hashes.length || hashes.length > 100 || hashes.some(value => !/^0x[0-9a-f]{40}$/.test(value))) throw new Error("Unbounded/invalid membership request");
      return { table, values: hashes.map(value => this.item(fid, this.post(fid, value))) };
    }
    if (table !== "my_library_page" || args.length !== 4) throw new Error(`Unexpected Library view ${query}`);
    const [kind, generation, after, limit] = args;
    if (![0, 1].includes(Number(kind)) || generation !== state.generation || typeof after !== "string" || typeof limit !== "number" || limit < 1 || limit > 50) return { table, values: [] };
    let posts = this.forFid(fid).filter(post => kind === 0 ? post.seen : post.bookmarked);
    const time = (post: Post) => kind === 0 ? post.seenAt : post.bookmarkAt;
    posts.sort((a, b) => time(b) - time(a) || b.hash.localeCompare(a.hash));
    if (after) {
      const colon = after.indexOf(":"), beforeAt = Number(after.slice(0, colon)), beforeHash = after.slice(colon + 1);
      posts = posts.filter(post => time(post) < beforeAt || (time(post) === beforeAt && post.hash < beforeHash));
    }
    return { table, values: posts.slice(0, limit + 1).map(post => this.item(fid, post, `${time(post)}:${post.hash}`)) };
  }
  broadcast() {
    for (const connection of this.connections.filter(connection => connection.active)) {
      if (this.pausedPushes.has(connection.label)) continue;
      for (const selection of connection.selections.values()) {
        const next = selection.queries.map(query => this.select(connection, query));
        const changed = next.map((table, i) => ({ table: table.table, before: selection.tables[i]?.values ?? [], after: table.values })).filter(table => key(table.before) !== key(table.after));
        if (changed.length) {
          this.frames.push(JSON.parse(key({ connection: connection.id, label: connection.label, phase: "update", querySetId: selection.querySetId, tables: changed })));
          connection.socket.send(wire.update(selection.querySetId, changed));
        }
        selection.tables = next;
      }
    }
  }
  mutate(connection: Connection, name: string, args: Record<string, any>) {
    const fid = connection.fid;
    if (fid === undefined) throw new Error("verify the reader first");
    const account = { ...this.account(fid) };
    const now = Math.floor(Date.now() / 1000);
    if (name === "mark_notifications_seen") return;
    if (name === "mark_seen_batch") {
      if (Number(args.generation) !== account.generation) throw new Error("history generation changed");
      if (!args.entries.length || args.entries.length > 100) throw new Error("invalid seen batch");
      const changed: Post[] = [], unique = new Set<string>();
      for (const entry of args.entries) {
        const post = this.post(fid, entry.hash), expected = Number(entry.expectedRevision), time = Number(entry.at);
        if (unique.has(entry.hash) || !/^0x[0-9a-f]{40}$/.test(entry.hash) || time < 0 || time > now + 60) throw new Error("invalid seen input");
        unique.add(entry.hash);
        if (account.clearedAt > 0 && time <= account.clearedAt) continue;
        if (expected > post.seenRevision) throw new Error("seen revision changed");
        if ((!post.seen && expected !== post.seenRevision) || (post.seen && time <= post.seenAt)) continue;
        post.seen = true; post.seenAt = Math.max(post.seenAt, Math.min(time, now)); post.seenRevision++;
        changed.push(post);
      }
      if (changed.length) { for (const post of changed) this.save(fid, post); account.revision++; }
    } else if (name === "set_bookmark") {
      const post = this.post(fid, args.hash), desired = Boolean(args.desired);
      if (post.bookmarked === desired && (desired || post.bookmarkRevision > 0)) return;
      if (Number(args.expectedRevision) !== post.bookmarkRevision) throw new Error("bookmark revision changed");
      post.bookmarked = desired; post.bookmarkRevision++; post.bookmarkAt = desired ? now : 0;
      this.save(fid, post); account.revision++;
    } else if (name === "forget_seen") {
      if (Number(args.generation) !== account.generation) throw new Error("history generation changed");
      const post = this.post(fid, args.hash);
      if (!post.seen && post.seenRevision > 0) return;
      if (Number(args.expectedRevision) !== post.seenRevision) throw new Error("seen revision changed");
      post.seen = false; post.seenAt = 0; post.seenRevision++;
      this.save(fid, post); account.revision++;
    } else if (name === "clear_seen") {
      const expected = Number(args.expectedGeneration);
      if (account.lastClearId === args.operationId && expected + 1 === account.generation) return;
      if (expected !== account.generation) throw new Error("history generation changed");
      account.generation++; account.revision++; account.clearedAt = Math.max(account.clearedAt, now); account.lastClearId = args.operationId;
    } else throw new Error(`Unexpected mutation ${name}`);
    this.accounts.set(fid, account);
  }
  async attach(page: Page, label: string) {
    await page.route("https://maincloud.spacetimedb.com/**", async route => {
      const headers = { "access-control-allow-origin": "*", "access-control-allow-headers": "authorization", "access-control-allow-methods": "POST, OPTIONS" };
      if (route.request().method() === "OPTIONS") return route.fulfill({ status: 204, headers });
      const token = (await route.request().headerValue("authorization"))?.replace(/^Bearer /, "");
      if (new URL(route.request().url()).pathname !== "/v1/identity/websocket-token" || !token || !this.tokens.has(token)) return route.fulfill({ status: 401, json: { error: "Unknown fixture token" }, headers });
      this.exchanges.push(label);
      return this.offline.has(label) ? route.fulfill({ status: 503, json: { error: "Fixture offline" }, headers }) : route.fulfill({ json: { token }, headers });
    });
    await page.routeWebSocket("wss://maincloud.spacetimedb.com/**", socket => {
      if (this.offline.has(label)) { void socket.close({ code: 1013, reason: "Fixture offline" }); return; }
      const requested = new URL(socket.url()).searchParams.get("token");
      const token = requested && this.tokens.has(requested) ? requested : `library-token-${this.tokens.size + 1}`;
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
          this.frames.push(JSON.parse(key({ connection: connection.id, label, phase: "snapshot", querySetId: selection.querySetId, tables: selection.tables })));
          socket.send(wire.snapshot(message.value.requestId, selection.querySetId, selection.tables));
        } else if (message.tag === "Unsubscribe") {
          const selection = connection.selections.get(key(message.value.querySetId));
          connection.selections.delete(key(message.value.querySetId));
          this.frames.push(JSON.parse(key({ connection: connection.id, label, phase: "unsubscribe", querySetId: message.value.querySetId, queries: selection?.queries ?? [] })));
          socket.send(wire.unsubscribed(message.value.requestId, message.value.querySetId, selection?.tables ?? []));
        } else if (message.tag === "CallReducer") {
          const { reducer: name, args, requestId } = message.value;
          const call: Call = { label, name, fid: connection.fid, args: {} }; this.calls.push(call);
          try {
            const decoded = wire.reducer(name, args); call.args = JSON.parse(key(decoded));
            if (name === "verify_notification_fid") {
              const fid = Number(decoded.fid); call.fid = fid;
              if (this.authorized.get(connection.identity) !== fid || (connection.fid !== undefined && connection.fid !== fid)) throw new Error("attestation FID mismatch");
              connection.fid = fid;
            } else this.mutate(connection, name, decoded);
            this.broadcast();
            const acknowledge = () => { if (connection.active) socket.send(wire.ok(requestId)); };
            if (name === "mark_seen_batch" && this.holdSeen.has(label)) {
              const pending = this.heldSeen.get(label) ?? [];
              pending.push(acknowledge); this.heldSeen.set(label, pending);
            } else acknowledge();
          } catch (error) { call.rejected = String(error); socket.send(wire.error(requestId, String(error))); }
        } else throw new Error(`Unexpected Library protocol ${message.tag}`);
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
async function reader(browser: Browser, peer: Peer, label: string, who: typeof alice | null, options: { pool?: ReturnType<typeof cast>[]; holdAttestation?: number; blockStorage?: boolean; legacy?: { hash: string; at: number }[] } = {}) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  const page = await context.newPage(), errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.addInitScript(() => {
    Object.defineProperty(Document.prototype, "hidden", { configurable: true, get: () => false });
    Object.defineProperty(Document.prototype, "visibilityState", { configurable: true, get: () => "visible" });
    (window as any).__libraryPaint = [];
    new MutationObserver(() => {
      for (const card of document.querySelectorAll("#boot .cast-card[data-hash], #app .cast-card[data-hash]")) {
        const value = card.getAttribute("data-hash")!;
        if (!(window as any).__libraryPaint.includes(value)) (window as any).__libraryPaint.push(value);
      }
    }).observe(document, { childList: true, subtree: true });
  });
  if (options.blockStorage) await page.addInitScript(() => {
    (window as any).__libraryStorageBlocked = true;
    const open = IDBFactory.prototype.open;
    IDBFactory.prototype.open = function(name: string, version?: number) {
      if (name === "allthing-library-v1" && (window as any).__libraryStorageBlocked) throw new DOMException("Fixture private storage denied", "SecurityError");
      return version === undefined ? open.call(this, name) : open.call(this, name, version);
    };
  });
  if (who) await signIn(page, who);
  if (who && options.legacy) await page.addInitScript(({ fid, posts }) => localStorage.setItem("allthing.seen.v1", JSON.stringify({ version: 1, scopes: [{ name: `fid:${fid}`, touched: Math.max(...posts.map(post => post.at)), posts }] })), { fid: who.fid, posts: options.legacy });
  let release!: () => void; const held = new Promise<void>(resolve => { release = resolve; });
  const requests = { attest: [] as number[], batches: [] as string[][], publicWrites: [] as string[] };
  page.on("request", request => { if (request.method() === "POST" && /submitMessage|auth\/signer/.test(request.url())) requests.publicWrites.push(request.url()); });
  await page.route("**/api/v1/**", route => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/status", route => route.fulfill({ json: { spacetime: { attest: true, library: true } } }));
  await page.route("**/api/v1/home**", route => route.fulfill({ json: { pool: options.pool ?? [cast(1), cast(2)], notes: {}, meta: meta() } }));
  await page.route("**/api/v1/latest**", route => route.fulfill({ json: { pool: options.pool ?? [cast(1), cast(2)], cursors: [], notes: {}, meta: meta() } }));
  await page.route("**/api/v1/following**", route => route.fulfill({ json: { pool: options.pool ?? [cast(1), cast(2)], notes: {}, meta: meta() } }));
  await page.route("**/api/v1/notifications**", route => route.fulfill({ json: { groups: [], newest: 0, notes: {}, meta: meta() } }));
  await page.route("**/api/v1/cast?*", route => {
    const n = Number.parseInt(new URL(route.request().url()).searchParams.get("hash")!.slice(2), 16);
    return route.fulfill({ json: { thread: { cast: cast(n), children: [] }, ancestors: [], notes: {}, meta: meta() } });
  });
  await page.route("**/api/v1/auth/attest", async route => {
    const fid = cookieFid(await route.request().headerValue("cookie") ?? ""); requests.attest.push(fid);
    if (fid === options.holdAttestation) await held;
    const { identity } = route.request().postDataJSON(); peer.authorized.set(identity, fid);
    return route.fulfill({ json: { identity, fid, expiry: Math.floor(Date.now() / 1000) + 600, signature: "11".repeat(64) } });
  });
  await page.route("**/api/v1/library/casts?*", route => {
    const hashes = new URL(route.request().url()).searchParams.get("hashes")!.split(","); requests.batches.push(hashes);
    return route.fulfill({ json: { pool: hashes.map(value => cast(Number.parseInt(value.slice(2), 16))), unavailable: [], filtered: [], notes: {}, meta: meta() } });
  });
  await page.route("https://haatz.quilibrium.com/**", route => route.fulfill({ headers: { "access-control-allow-origin": "*" }, json: { messages: [] } }));
  await peer.attach(page, label);
  return { context, page, errors, requests, releaseAttestation: release };
}
type Reader = Awaited<ReturnType<typeof reader>>;
const card = (page: Page, n: number) => page.locator(`#app .cast-card[data-hash="${hash(n)}"]`);
const saved = (page: Page, n: number, on = false) => card(page, n).getByRole("button", { name: on ? "Remove bookmark" : "Save post", exact: true });
async function close(...readers: Reader[]) { await Promise.all(readers.map(value => value.context.close())); }
async function reconnect(peer: Peer, value: Reader, label: string) {
  peer.offline.delete(label);
  await value.page.evaluate(() => { window.dispatchEvent(new Event("online")); window.dispatchEvent(new Event("focus")); });
}
async function diskRecord(page: Page, key: string) {
  return page.evaluate(async key => {
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const open = indexedDB.open("allthing-library-v1", 1);
      open.onsuccess = () => resolve(open.result); open.onerror = () => reject(open.error);
    });
    try { return await new Promise<any>((resolve, reject) => {
      const tx = db.transaction("records", "readonly"), request = tx.objectStore("records").get(key);
      tx.oncomplete = () => resolve(typeof request.result === "string" ? JSON.parse(request.result) : null);
      tx.onabort = () => reject(tx.error);
    }); } finally { db.close(); }
  }, key);
}
const diskKey = (kind: string, fid: number, value = "") => `${kind}:${String(fid).padStart(20, "0")}:${value}`;
async function storageFaultAndActivity(page: Page) {
  await page.addInitScript(() => {
    const host = window as any;
    host.__libraryActiveStorage = 0; host.__libraryFailNextOpen = false;
    const open = IDBFactory.prototype.open;
    IDBFactory.prototype.open = function(name: string, version?: number) {
      if (name === "allthing-library-v1" && host.__libraryFailNextOpen) {
        host.__libraryFailNextOpen = false;
        throw new DOMException("Transient fixture read denial", "SecurityError");
      }
      const request = version === undefined ? open.call(this, name) : open.call(this, name, version);
      if (name === "allthing-library-v1") {
        host.__libraryActiveStorage++;
        let done = false; const finish = () => { if (!done) { done = true; host.__libraryActiveStorage--; } };
        request.addEventListener("success", finish, { once: true }); request.addEventListener("error", finish, { once: true });
      }
      return request;
    };
    const transaction = IDBDatabase.prototype.transaction;
    IDBDatabase.prototype.transaction = function(stores: string | string[], mode?: IDBTransactionMode, options?: IDBTransactionOptions) {
      const tx = transaction.call(this, stores, mode, options);
      if (this.name === "allthing-library-v1") {
        host.__libraryActiveStorage++;
        let done = false; const finish = () => { if (!done) { done = true; host.__libraryActiveStorage--; } };
        tx.addEventListener("complete", finish, { once: true }); tx.addEventListener("abort", finish, { once: true });
      }
      return tx;
    };
  });
}
async function diagnose(peer: Peer, ...readers: Reader[]) {
  await test.info().attach("library-sync-diagnostics", { contentType: "application/json", body: JSON.stringify({
    calls: peer.calls, queries: peer.queries, exchanges: peer.exchanges, frames: peer.frames.slice(-300),
    accounts: [...peer.accounts.values()], posts: [...peer.posts.values()],
    connections: peer.connections.map(connection => ({ id: connection.id, label: connection.label, active: connection.active, fid: connection.fid, selections: [...connection.selections.values()].map(selection => ({ id: selection.querySetId, queries: selection.queries })) })),
    readers: await Promise.all(readers.map(async reader => ({
      errors: reader.errors, requests: reader.requests, url: reader.page.url(),
      text: await reader.page.locator("#app").innerText().catch(() => "unavailable"),
      cards: await reader.page.locator("#app .cast-card[data-hash]").evaluateAll(cards => cards.map(card => ({ hash: card.getAttribute("data-hash"), buttons: [...card.querySelectorAll("button")].map(button => ({ label: button.getAttribute("aria-label"), pressed: button.getAttribute("aria-pressed"), disabled: button.disabled })) }))).catch(() => []),
    }))),
  }, null, 2) });
}

test("a fresh independent signed-in device never paints remotely seen Home cards, including after sort and reload", async ({ browser }) => {
  const peer = new Peer(); peer.seedSeen(alice.fid, 1);
  const a = await reader(browser, peer, "a", alice, { holdAttestation: alice.fid });
  try {
    await a.page.goto("/");
    await expect.poll(() => a.requests.attest.includes(alice.fid)).toBe(true);
    // A signed-in first frame cannot assume a fresh device has no history.
    // The real app is mounted, but both candidates await verified membership.
    await expect(a.page.locator("#app .cast-card[data-hash]")).toHaveCount(0);
    await expect(a.page.locator("#boot .cast-card")).toHaveCount(0);
    expect(await a.page.evaluate(() => (window as any).__libraryPaint)).toEqual([]);
    a.releaseAttestation();
    await expect(card(a.page, 2)).toBeVisible();
    await expect(card(a.page, 1)).toHaveCount(0);
    expect(await a.page.evaluate(() => (window as any).__libraryPaint)).not.toContain(hash(1));
    await a.page.getByRole("navigation", { name: "Sort", exact: true }).getByRole("link", { name: "top", exact: true }).click();
    await expect(card(a.page, 1)).toHaveCount(0);
    await a.page.reload();
    await expect.poll(() => peer.verified("a").length).toBe(1);
    await expect(card(a.page, 1)).toHaveCount(0);
    expect(await a.page.evaluate(() => (window as any).__libraryPaint)).not.toContain(hash(1));
    expect(a.errors).toEqual([]);
  } finally { a.releaseAttestation(); await diagnose(peer, a); await close(a); }
});

test("durable membership beyond30days and2000 reads is point-checked and Seen history hydrates bounded pages", async ({ browser }) => {
  const peer = new Peer();
  for (let n = 1; n <= 2102; n++) peer.seedSeen(alice.fid, n, at - 40 * 86400 - n);
  const a = await reader(browser, peer, "a", alice, { pool: [{ ...cast(1), ts: at - 45 * 86400 }, cast(9999)] });
  try {
    await a.page.goto("/?t=all");
    await expect(card(a.page, 9999)).toBeVisible();
    await expect(card(a.page, 1)).toHaveCount(0);
    await a.page.goto("/library?tab=seen");
    await expect(a.page.locator(".library-panel .cast-card")).toHaveCount(25);
    await a.page.getByRole("button", { name: "Load more", exact: true }).click();
    await expect.poll(() => a.requests.batches.flat().length).toBe(50);
    expect(a.requests.batches.every(batch => batch.length <= 25)).toBe(true);
    expect(peer.queries.filter(query => query.includes("my_library_membership")).every(query => (query.match(/0x/g) ?? []).length <= 100)).toBe(true);
    expect(peer.post(alice.fid, hash(1)).seen).toBe(true);
  } finally { await diagnose(peer, a); await close(a); }
});

test("private bookmarks propagate between independent sessions without a Farcaster signing key or public reaction", async ({ browser }) => {
  const peer = new Peer(), a = await reader(browser, peer, "a", alice), b = await reader(browser, peer, "b", alice);
  try {
    await Promise.all([a.page.goto("/"), b.page.goto("/")]);
    await expect(saved(a.page, 1)).toBeEnabled(); await expect(saved(b.page, 1)).toBeEnabled();
    await saved(a.page, 1).click();
    await expect(saved(b.page, 1, true)).toHaveAttribute("aria-pressed", "true");
    expect(peer.post(alice.fid, hash(1)).bookmarked).toBe(true);
    expect(a.requests.publicWrites.concat(b.requests.publicWrites)).toEqual([]);
    expect(await kept(a.page, alice.fid)).toBeNull();
    expect(await kept(b.page, alice.fid)).toBeNull();
    expect(await a.page.evaluate(async () => (await indexedDB.databases()).map(value => value.name))).not.toContain("allthing-stdb");
    await a.page.goto("/library");
    await expect(card(a.page, 1)).toBeVisible();
    await expect(a.page.getByRole("tab", { name: "Bookmarks", exact: true })).toHaveAttribute("aria-selected", "true");
    // revision() covers the whole private Library, so an unrelated real Seen
    // update may already offer refresh. The cached Back still has to observe
    // the later bookmark removal and retain its row until explicit refresh.
    // A remote change while this cached list is away must survive Back as a
    // refresh notice, while preserving its loaded rows and selected tab.
    await card(a.page, 1).locator("a.when").click();
    await expect(a.page).toHaveURL(new RegExp(`/p/${hash(1)}$`));
    await saved(b.page, 1, true).click();
    await expect.poll(() => peer.post(alice.fid, hash(1)).bookmarked).toBe(false);
    await a.page.goBack({ waitUntil: "commit" });
    await expect(card(a.page, 1)).toBeVisible();
    await expect(saved(a.page, 1)).toHaveAttribute("aria-pressed", "false");
    await expect(a.page.getByText("Your Library changed on this or another device.", { exact: false })).toBeVisible();
    await a.page.getByRole("button", { name: "Refresh this list", exact: true }).click();
    await expect(a.page.getByText("No bookmarks yet. Save a post to find it here.", { exact: true })).toBeVisible();
    expect(a.errors.concat(b.errors)).toEqual([]);
  } finally { await diagnose(peer, a, b); await close(a, b); }
});

test("saving during an in-flight automatic Seen write queues independently and survives reload", async ({ browser }) => {
  const peer = new Peer(); peer.holdSeen.add("a");
  const a = await reader(browser, peer, "a", alice, { pool: [cast(1)] });
  try {
    await a.page.goto("/");
    await expect(card(a.page, 1)).toBeVisible();
    // The real visible-card dwell reaches the server, which broadcasts the
    // committed Seen row but deliberately holds that reducer's result.
    await expect.poll(() => peer.calls.some(call => call.label === "a" && call.name === "mark_seen_batch" && (call.args.entries as { hash: string }[]).some(entry => entry.hash === hash(1)))).toBe(true);
    expect(peer.heldSeen.get("a")?.length).toBe(1);
    await saved(a.page, 1).click();
    await expect(saved(a.page, 1, true)).toHaveAttribute("aria-pressed", "true");
    await expect(card(a.page, 1).getByText("Syncing the previous change", { exact: false })).toHaveCount(0);
    peer.releaseSeen("a");
    await expect.poll(() => peer.post(alice.fid, hash(1)).bookmarked).toBe(true);
    await a.page.getByRole("navigation", { name: "Main", exact: true }).getByRole("link", { name: "Library", exact: true }).click();
    await expect(a.page).toHaveURL(/\/library$/);
    await expect(a.page.getByRole("tab", { name: "Bookmarks", exact: true })).toHaveAttribute("aria-selected", "true");
    await expect(a.page.locator(".library-panel .cast-card[data-hash]")).toHaveCount(1);
    await expect(saved(a.page, 1, true)).toBeVisible();
    await a.page.reload();
    await expect(saved(a.page, 1, true)).toHaveAttribute("aria-pressed", "true");
    expect(a.requests.publicWrites).toEqual([]);
    expect(a.errors).toEqual([]);
  } finally { peer.releaseSeen("a"); await diagnose(peer, a); await close(a); }
});

test("a queued offline bookmark cannot resurrect a newer removal and stays correct after reload", async ({ browser }) => {
  const peer = new Peer(), a = await reader(browser, peer, "a", alice), b = await reader(browser, peer, "b", alice);
  try {
    await Promise.all([a.page.goto("/"), b.page.goto("/")]);
    await expect(saved(a.page, 1)).toBeEnabled(); await expect(saved(b.page, 1)).toBeEnabled();
    await peer.disconnect("b");
    await saved(b.page, 1).click();
    await saved(a.page, 1).click(); await expect(saved(a.page, 1, true)).toBeEnabled();
    await saved(a.page, 1, true).click();
    await expect.poll(() => peer.post(alice.fid, hash(1)).bookmarked).toBe(false);
    await reconnect(peer, b, "b");
    await expect(saved(b.page, 1)).toHaveAttribute("aria-pressed", "false");
    expect(peer.post(alice.fid, hash(1)).bookmarked).toBe(false);
    // Reload the intentional saved-post view. Home correctly hides the
    // now-seen card, so it cannot be used as a bookmark-state oracle.
    await b.page.getByRole("navigation", { name: "Main", exact: true }).getByRole("link", { name: "Library", exact: true }).click();
    await expect(b.page).toHaveURL(/\/library$/);
    await expect(b.page.getByRole("tab", { name: "Bookmarks", exact: true })).toHaveAttribute("aria-selected", "true");
    await b.page.reload();
    await expect(b.page.getByText("No bookmarks yet. Save a post to find it here.", { exact: true })).toBeVisible();
    expect(peer.post(alice.fid, hash(1)).bookmarked).toBe(false);
    expect(peer.exchanges).toContain("b");
  } finally { await diagnose(peer, a, b); await close(a, b); }
});

test("an old attestation cannot hydrate the next account or mix its saved posts", async ({ browser }) => {
  const peer = new Peer(); peer.seedBookmark(alice.fid, 1); peer.seedBookmark(bob.fid, 9);
  const a = await reader(browser, peer, "a", alice, { holdAttestation: alice.fid });
  try {
    await a.page.goto("/library");
    await expect.poll(() => a.requests.attest.includes(alice.fid)).toBe(true);
    await a.page.evaluate(user => {
      document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax`;
      const channel = new BroadcastChannel("allthing-session"); channel.postMessage("changed"); channel.close();
    }, bob);
    await expect(card(a.page, 9)).toBeVisible();
    a.releaseAttestation();
    await a.page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    await expect(card(a.page, 1)).toHaveCount(0);
    expect(a.errors).toEqual([]);
  } finally { a.releaseAttestation(); await diagnose(peer, a); await close(a); }
});

test("explicit mark unread changes future Home eligibility while preserving the bookmark", async ({ browser }) => {
  const peer = new Peer(); peer.seedSeen(alice.fid, 1); peer.seedBookmark(alice.fid, 1);
  const a = await reader(browser, peer, "a", alice);
  let fresh: Reader | undefined;
  try {
    await a.page.goto("/library?tab=seen");
    await expect(card(a.page, 1)).toBeVisible();
    await card(a.page, 1).getByRole("button", { name: "Mark unread", exact: true }).click();
    await expect.poll(() => peer.post(alice.fid, hash(1)).seen).toBe(false);
    expect(peer.post(alice.fid, hash(1)).bookmarked).toBe(true);
    fresh = await reader(browser, peer, "fresh", alice);
    await fresh.page.goto("/");
    await expect(card(fresh.page, 1)).toBeVisible();
    expect(a.errors.concat(fresh.errors)).toEqual([]);
  } finally { await diagnose(peer, a, ...(fresh ? [fresh] : [])); await close(a, ...(fresh ? [fresh] : [])); }
});

test("clear history fences a legacy offline queue without deleting private bookmarks", async ({ browser }) => {
  const peer = new Peer(); peer.seedSeen(alice.fid, 1); peer.seedBookmark(alice.fid, 1); peer.offline.add("b");
  const a = await reader(browser, peer, "a", alice), b = await reader(browser, peer, "b", alice, { blockStorage: true, legacy: [{ hash: hash(3), at: at - 5 }] });
  try {
    await Promise.all([a.page.goto("/library?tab=seen"), b.page.goto("/")]);
    await expect(card(a.page, 1)).toBeVisible();
    await a.page.getByRole("button", { name: "Clear reading history…", exact: true }).click();
    await a.page.getByRole("button", { name: "Clear history on all devices", exact: true }).click();
    await expect.poll(() => peer.account(alice.fid).generation).toBe(1);
    // A denied first storage open must fail closed, then recover by explicit
    // Retry once storage returns; it cannot strand bootstrap or replay old reads.
    await expect(b.page.locator("#app .cast-card[data-hash]")).toHaveCount(0);
    await b.page.goto("/library");
    await expect(b.page.getByRole("button", { name: "Retry sync", exact: true })).toBeVisible();
    await b.page.evaluate(() => { (window as any).__libraryStorageBlocked = false; });
    peer.offline.delete("b");
    await b.page.getByRole("button", { name: "Retry sync", exact: true }).click();
    await expect.poll(() => peer.verified("b").length).toBe(1);
    await expect(saved(b.page, 1, true)).toBeEnabled();
    expect(peer.post(alice.fid, hash(3)).seen).toBe(false);
    expect(peer.post(alice.fid, hash(1)).bookmarked).toBe(true);
    expect(peer.calls.filter(call => call.label === "b" && call.name === "mark_seen_batch").every(call => !call.rejected || /generation/.test(call.rejected))).toBe(true);
  } finally { await diagnose(peer, a, b); await close(a, b); }
});

test("signed-out visits never start Library attestation, private subscriptions or message keys", async ({ browser }) => {
  const peer = new Peer(), a = await reader(browser, peer, "a", null);
  try {
    await a.page.goto("/library");
    await expect(a.page.getByText("Sign in to see your Library.", { exact: true })).toBeVisible();
    expect(a.requests.attest).toEqual([]); expect(peer.connections).toEqual([]); expect(peer.queries).toEqual([]);
    const databases = await a.page.evaluate(async () => (await indexedDB.databases()).map(value => value.name));
    expect(databases).not.toContain("allthing-stdb");
  } finally { await diagnose(peer, a); await close(a); }
});


test("a successful durable drain restores live membership after a transient local read failure", async ({ browser }) => {
  const peer = new Peer(), a = await reader(browser, peer, "a", alice, { pool: [cast(1)] }), b = await reader(browser, peer, "b", alice, { pool: [cast(1)] });
  await storageFaultAndActivity(a.page);
  try {
    await Promise.all([a.page.goto("/"), b.page.goto("/")]);
    await expect(saved(a.page, 1)).toBeEnabled(); await expect(saved(b.page, 1)).toBeEnabled();
    // One reader's accepted Seen update can suppress the other's redundant
    // write. Fence on authoritative state and A's committed, empty outbox so
    // the induced failure has no unrelated startup write to revive it.
    await expect.poll(() => peer.post(alice.fid, hash(1)).seen).toBe(true);
    await expect.poll(async () => {
      const item = await diskRecord(a.page, diskKey("cache", alice.fid, hash(1)));
      return item?.known === true && item.server.fid === alice.fid && item.server.hash === hash(1)
        && item.server.generation === peer.account(alice.fid).generation && item.server.seen === true
        && item.server.seen_revision > 0 && item.read === null && item.bookmark === null;
    }).toBe(true);
    await a.page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    await expect.poll(() => diskRecord(a.page, diskKey("pending", alice.fid, hash(1)))).toBeNull();
    await expect.poll(() => a.page.evaluate(() => (window as any).__libraryActiveStorage)).toBe(0);
    await a.page.evaluate(() => { (window as any).__libraryFailNextOpen = true; });
    await saved(a.page, 1).click();
    await expect(a.page.getByRole("button", { name: "Retry history sync", exact: true })).toBeVisible();
    await expect.poll(() => peer.verified("a").flatMap(c => [...c.selections.values()].flatMap(s => s.queries)).filter(q => q.includes("my_library_membership")).length).toBe(0);
    peer.pausedPushes.add("a");
    await saved(b.page, 1).click();
    await expect.poll(() => peer.post(alice.fid, hash(1)).bookmarked).toBe(true);
    // A still has its old false point. Saving now is durable but is settled by
    // an authoritative point response, without another reducer/account update.
    await saved(a.page, 1).click();
    await expect(saved(a.page, 1, true)).toHaveAttribute("aria-pressed", "true");
    await expect.poll(async () => { const item = await diskRecord(a.page, diskKey("cache", alice.fid, hash(1))); return item?.bookmark === null && item?.bookmark_settled > 0; }).toBe(true);
    await expect.poll(() => diskRecord(a.page, diskKey("pending", alice.fid, hash(1)))).toBeNull();
    await expect.poll(() => a.page.evaluate(() => (window as any).__libraryActiveStorage)).toBe(0);
    await a.page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    await expect(a.page.getByRole("button", { name: "Retry history sync", exact: true })).toHaveCount(0);
    await expect.poll(() => peer.verified("a").flatMap(c => [...c.selections.values()].flatMap(s => s.queries)).filter(q => q.includes("my_library_membership")).length).toBe(1);
    expect(peer.calls.filter(call => call.label === "a" && call.name === "set_bookmark")).toEqual([]);
    expect(a.requests.publicWrites).toEqual([]); expect(a.errors.concat(b.errors)).toEqual([]);
  } finally { peer.pausedPushes.delete("a"); await diagnose(peer, a, b); await close(a, b); }
});

test("a clear requested during a Seen flight precedes later durable bookmark intent without manual retry", async ({ browser }) => {
  const peer = new Peer(); peer.holdSeen.add("a");
  const a = await reader(browser, peer, "a", alice, { pool: [cast(1)] });
  await storageFaultAndActivity(a.page);
  try {
    await a.page.goto("/"); await expect(card(a.page, 1)).toBeVisible();
    await expect.poll(() => peer.heldSeen.get("a")?.length ?? 0).toBe(1);
    await a.page.getByRole("navigation", { name: "Main", exact: true }).getByRole("link", { name: "Library", exact: true }).click();
    await expect(a.page).toHaveURL(/\/library$/);
    await a.page.getByRole("tab", { name: "Seen", exact: true }).click();
    await expect(card(a.page, 1)).toBeVisible();
    await a.page.getByRole("button", { name: "Clear reading history…", exact: true }).click();
    await a.page.getByRole("button", { name: "Clear history on all devices", exact: true }).click();
    await expect.poll(async () => (await diskRecord(a.page, diskKey("account", alice.fid)))?.pending_clear?.expected_generation).toBe(0);
    await saved(a.page, 1).click();
    await expect.poll(async () => (await diskRecord(a.page, diskKey("pending", alice.fid, hash(1))))?.bookmark?.desired).toBe(true);
    await expect.poll(() => a.page.evaluate(() => (window as any).__libraryActiveStorage)).toBe(0);
    await a.page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    peer.releaseSeen("a");
    await expect.poll(() => peer.account(alice.fid).generation).toBe(1);
    await expect.poll(() => peer.post(alice.fid, hash(1)).bookmarked).toBe(true);
    await expect.poll(() => diskRecord(a.page, diskKey("pending", alice.fid, hash(1)))).toBeNull();
    const calls = peer.calls.filter(call => call.label === "a").map(call => call.name);
    expect(calls.indexOf("clear_seen")).toBeGreaterThan(calls.indexOf("mark_seen_batch"));
    expect(calls.indexOf("set_bookmark")).toBeGreaterThan(calls.indexOf("clear_seen"));
    expect(peer.calls.filter(call => call.label === "a" && call.rejected)).toEqual([]);
    expect(peer.post(alice.fid, hash(1)).seen).toBe(false);
    await expect(a.page.getByText("Private Library connected.", { exact: true })).toBeVisible();
    await expect(a.page.getByRole("button", { name: "Retry sync", exact: true })).toHaveCount(0);
    expect(a.errors).toEqual([]); expect(a.requests.publicWrites).toEqual([]);
  } finally { peer.releaseSeen("a"); await diagnose(peer, a); await close(a); }
});
