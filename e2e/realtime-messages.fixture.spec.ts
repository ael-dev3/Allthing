// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type BrowserContext, type Page } from "@playwright/test";
import axe from "axe-core";
import { signIn } from "./helpers/farcaster";

/*
 * The Messages inbox (#general and encrypted conversations, in one list),
 * on Allthing's SpacetimeDB database,
 * from end to end in the browser: two readers, Alice and Bob, each in a
 * browser of their own, with the app's real code (the Rust that seals and
 * opens messages, WebCrypto's X25519, IndexedDB).
 *
 * The database is a stand-in in this test: the bridge script the app adds
 * (`assets/stdb-<hash>.js`, the SpacetimeDB TypeScript SDK in the real
 * build) is answered with one that has the same interface and asks this
 * test, which keeps the rows as the module does (spacetime/spacetimedb) and
 * shows each reader only their own encrypted messages, as its views do. So
 * the test sees exactly what the database would be sent and keep.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

const NODE = "https://haatz.quilibrium.com";
const cors = { "access-control-allow-origin": "*" };
const alice = { fid: 101, username: "alice", display_name: "Alice" };
const bob = { fid: 202, username: "bob", display_name: "Bob" };
const people = [alice, bob];
const id = (n: number) => `c200${n.toString(16).padStart(60, "0")}`;

type Row = Record<string, unknown>;

/** The stand-in database: the module's tables, rules and views. */
class Database {
  verifyAttestations = false;
  users = new Map<string, Row>();
  keys = new Map<string, Row>();
  chat: Row[] = [];
  dms: Row[] = [];
  next = 1;
  pages = new Map<string, { page: Page; slot: number }>();
  tokens = new Map<string, string>();
  moderation = false;
  readers = new Map<string, number>();
  filters = new Map<string, Row>();
  filterSubscribers = new Set<string>();
  calls: { identity: string; op: string; args: unknown[] }[] = [];

  connect(page: Page, token: string, slot: number) {
    let identity = this.tokens.get(token);
    if (!identity) {
      identity = id(this.tokens.size + 1);
      token = `token-${identity}`;
      this.tokens.set(token, identity);
    }
    this.pages.set(identity, { page, slot });
    return { identity, token };
  }

  async push(identity: string, event: object) {
    const target = this.pages.get(identity);
    if (target && !target.page.isClosed()) await target.page.evaluate(({ slot, event }) => {
      (window as any).__stdbEmits?.[slot]?.(event);
    }, { slot: target.slot, event: JSON.stringify(event) }).catch(() => {});
  }

  async broadcast(table: string, row: Row, op = "insert") {
    for (const identity of this.pages.keys()) await this.push(identity, { t: op, table, row });
  }

  async snapshot(identity: string, queries: string[]) {
    if (queries.every(query => /FROM my_(account_filters|notification_state)\b/i.test(query))) {
      if (queries.some(query => /FROM my_account_filters\b/i.test(query))) {
        this.filterSubscribers.add(identity);
        for (const row of this.filters.values()) {
          if (row.fid === this.readers.get(identity)) await this.push(identity, { t: "insert", table: "my_account_filters", row });
        }
      }
      return;
    }
    const rows: [string, Row][] = [
      ...[...this.users.values()].map((r) => ["user", r] as [string, Row]),
      ...[...this.keys.values()].map((r) => ["user_key", r] as [string, Row]),
      ["room", { id: 1, name: "general", created_by: id(0), created_at: 0 }],
      ...this.chat.map((r) => ["chat_message", r] as [string, Row]),
      ...this.dms.filter((d) => d.recipient === identity).map((r) => ["my_dm_inbox", r] as [string, Row]),
      ...this.dms.filter((d) => d.sender === identity).map((r) => ["my_dm_outbox", r] as [string, Row]),
    ];
    for (const [table, row] of rows) await this.push(identity, { t: "insert", table, row });
  }

  user(identity: string) {
    const u = this.users.get(identity);
    if (!u) throw new Error("register first");
    if (this.verifyAttestations && !u.verified) throw new Error("verify your Farcaster account first");
    return u;
  }

  async setFilter(fid: number, target: number, kind: number, desired: boolean, expected: number, username: string) {
    const key = `${fid}:${target}`;
    const old = this.filters.get(key);
    const row = { key, fid, target_fid: target, target_username: username, muted: false, blocked: false,
      mute_revision: 0, block_revision: 0, updated_at: 0, ...old };
    const field = kind === 0 ? "muted" : "blocked", revision = kind === 0 ? "mute_revision" : "block_revision";
    if (row[field] === desired && Number(row[revision]) > 0) return;
    if (Number(row[revision]) !== expected) throw new Error("account filter revision changed");
    row[field] = desired; row[revision] = Number(row[revision]) + 1;
    row.updated_at = Math.floor(Date.now() / 1000);
    this.filters.set(key, row);
    for (const identity of this.filterSubscribers) {
      if (this.readers.get(identity) === fid) await this.push(identity, { t: old ? "update" : "insert", table: "my_account_filters", row });
    }
  }

  async call(identity: string, op: string, args: unknown[]): Promise<void> {
    this.calls.push({ identity, op, args });
    const now = Date.now() * 1000;
    switch (op) {
      case "register": {
        const old = this.users.get(identity);
        const row = { identity, fid: args[0] as number, verified: Boolean(old?.verified && old.fid === args[0]), online: true, last_seen: now };
        this.users.set(identity, row);
        return this.broadcast("user", row, old ? "update" : "insert");
      }
      case "verifyFid": {
        const [fid, , signature] = args as [number, number, string];
        if (signature !== `fixture-attestation-${identity}-${fid}`) throw new Error("attestation signature doesn't verify");
        const row = { identity, fid, verified: true, online: true, last_seen: now };
        this.users.set(identity, row);
        return this.broadcast("user", row, "update");
      }
      case "verifyNotificationFid": {
        const [fid, , signature] = args as [number, number, string];
        if (signature !== `fixture-attestation-${identity}-${fid}`) throw new Error("attestation signature doesn't verify");
        const old = this.readers.get(identity);
        if (old !== undefined && old !== fid) throw new Error("notification fid locked (verified)");
        this.readers.set(identity, fid);
        return;
      }
      case "markNotificationsSeen": return;
      case "setAccountFilter": {
        const fid = this.readers.get(identity);
        if (fid === undefined) throw new Error("verify your Farcaster account first");
        const [target, username, kind, desired, expected] = args as [number, string, number, boolean, number];
        return this.setFilter(fid, target, kind, desired, expected, username);
      }
      case "setPublicKey": {
        const u = this.user(identity);
        const key = args[0] as string;
        if (!/^[0-9a-f]{64}$/.test(key)) throw new Error("x25519 key must be 32 bytes");
        const old = this.keys.get(identity);
        const row = { identity, fid: u.fid, x25519_pub: key, key_version: old ? (old.key_version as number) + 1 : 1, updated_at: now };
        this.keys.set(identity, row);
        return this.broadcast("user_key", row, old ? "update" : "insert");
      }
      case "sendMessage": {
        const u = this.user(identity);
        const text = (args[1] as string).trim();
        if (!text || text.length > 4000) throw new Error("message 1-4000 bytes");
        const row = { id: this.next++, room_id: args[0], sender: identity, sender_fid: u.fid, text, sent_at: now };
        this.chat.push(row);
        return this.broadcast("chat_message", row);
      }
      case "setTyping":
        return;
      case "sendDm": {
        const sender = this.user(identity);
        const [recipient, skv, rkv, nonce, ciphertext] = args as [string, number, number, string, string];
        if (recipient === identity) throw new Error("cannot DM self");
        if (!this.keys.has(recipient)) throw new Error("recipient has no key");
        const recipientUser = this.user(recipient);
        if (!sender.verified || !recipientUser.verified) throw new Error("conversation unavailable");
        const recipientFid = recipientUser.fid;
        if (this.filters.get(`${sender.fid}:${recipientFid}`)?.blocked || this.filters.get(`${recipientFid}:${sender.fid}`)?.blocked) throw new Error("conversation unavailable");
        if (nonce.length !== 48) throw new Error("nonce must be 24 bytes");
        if (ciphertext.length < 32 || ciphertext.length > 32768) throw new Error("ciphertext 16..16384 bytes");
        const row = { id: this.next++, sender: identity, recipient, sender_key_version: skv, recipient_key_version: rkv, nonce, ciphertext, sent_at: now };
        this.dms.push(row);
        await this.push(identity, { t: "insert", table: "my_dm_outbox", row });
        return this.push(recipient, { t: "insert", table: "my_dm_inbox", row });
      }
      default:
        throw new Error(`no reducer ${op}`);
    }
  }
}

/** The bridge the app loads, as the test serves it: the real one's interface, asking the test. */
const FAKE_BRIDGE = `
let nextSlot = 0;
window.__stdbEmits = {};
window.allthingStdb = {
  version: 1,
  connect(uri, database, token, emit) {
    const slot = ++nextSlot, cache = new Map();
    const call = (op, args) => window.__stdb(slot, op, JSON.stringify(args));
    const settle = (op, args) => call(op, args).then(() => JSON.stringify({ ok: true }), (e) => JSON.stringify({ ok: false, error: String(e && e.message || e).replace(/^Error: /, "") }));
    window.__stdbEmits[slot] = json => {
      const event = JSON.parse(json);
      if (event.table === 'my_account_filters') {
        if (event.t === 'delete') cache.delete(event.row.key);
        else cache.set(event.row.key, event.row);
      }
      emit(json);
    };
    window.__stdbTarget = { uri, database };
    const bridge = {
      connected: call("connect", [token]),
      subscribe: (queries) => call("subscribe", [JSON.parse(queries)]),
      watch: (_tag, queries) => call("subscribe", [JSON.parse(queries)]),
      unwatch() {},
      rows: accessor => JSON.stringify(accessor === 'myAccountFilters' ? [...cache.values()] : []),
      moderationState: () => JSON.stringify([...cache.values()]),
      disconnect() { delete window.__stdbEmits[slot]; },
    };
    for (const op of ["register", "verifyFid", "verifyNotificationFid", "markNotificationsSeen", "setAccountFilter", "setPublicKey", "sendMessage", "setTyping", "sendDm", "deleteDm"]) {
      bridge[op] = (...args) => settle(op, args);
    }
    return bridge;
  },
};
`;

async function reader(browser: Browser, db: Database, who: typeof alice, width: number): Promise<{ context: BrowserContext; page: Page; requests: string[] }> {
  const context = await browser.newContext({ viewport: { width, height: width < 600 ? 844 : 900 } });
  await context.grantPermissions(["clipboard-read", "clipboard-write"]).catch(() => {});
  const page = await context.newPage();
  const requests: string[] = [];
  page.on("request", (r) => requests.push(r.url()));
  const identities = new Map<number, string>();
  await page.exposeBinding("__stdb", async (_source, slot: number, op: string, json: string) => {
    const args = JSON.parse(json) as unknown[];
    if (op === "connect") {
      const c = db.connect(page, args[0] as string, slot);
      identities.set(slot, c.identity);
      return JSON.stringify(c);
    }
    const identity = identities.get(slot)!;
    if (op === "subscribe") return db.snapshot(identity, args[0] as string[]);
    return db.call(identity, op, args);
  });
  await page.route("**/assets/stdb-*.js", (route) => route.fulfill({ contentType: "text/javascript", body: FAKE_BRIDGE }));
  if (db.moderation) {
    await page.route("**/api/v1/status", route => route.fulfill({ json: { spacetime: { attest: true, moderation: true, library: false } } }));
    await page.route("**/api/v1/notifications*", route => route.fulfill({ json: { groups: [], unread: [], meta: { backend: "hypersnap", fetched_at: Date.now() } } }));
  }
  await page.route(`${NODE}/v2/farcaster/user/bulk?*`, (route) => {
    const fids = new URL(route.request().url()).searchParams.get("fids")!.split(",").map(Number);
    return route.fulfill({ headers: cors, json: { users: people.filter((p) => fids.includes(p.fid)) } });
  });
  await page.route("**/api/v1/search?*", (route) =>
    route.fulfill({ json: { query: "", users: [alice, bob], channels: [], casts: [], meta: { backend: "hypersnap", fetched_at: Date.now() } } }),
  );
  await page.route("**/api/v1/auth/attest", async (route) => {
    if (!db.verifyAttestations) return route.fulfill({ status: 404, json: { error: "Messages aren't set up on this server" } });
    const { identity: asked } = route.request().postDataJSON() as { identity: string };
    return route.fulfill({ json: { identity: asked, fid: who.fid, expiry: Math.floor(Date.now() / 1000) + 600, signature: `fixture-attestation-${asked}-${who.fid}` } });
  });
  await signIn(page, who);
  return { context, page, requests };
}

/** The message key this browser keeps for `fid`, as IndexedDB holds it. */
function keptMessageKey(page: Page, fid: number) {
  return page.evaluate(
    (fid) =>
      new Promise<null | { algorithm: string; extractable: boolean; usages: string[]; public: string; token: string | null }>((resolve, reject) => {
        const open = indexedDB.open("allthing-stdb", 1);
        open.onupgradeneeded = () => open.result.createObjectStore("identities", { keyPath: "fid" });
        open.onerror = () => reject(open.error);
        open.onsuccess = () => {
          const get = open.result.transaction("identities").objectStore("identities").get(fid);
          get.onsuccess = () => {
            const r = get.result;
            open.result.close();
            resolve(r && r.private ? { algorithm: r.private.algorithm.name, extractable: r.private.extractable, usages: r.private.usages, public: r.public, token: r.token ?? null } : null);
          };
        };
      }),
    fid,
  );
}

test("a pressed conversation row keeps its secondary text readable", async ({ browser }) => {
  const { context, page } = await reader(browser, new Database(), alice, 1280);
  try {
    await page.goto("/messages?room=general");
    const row = page.getByRole("navigation", { name: "Conversations" }).getByRole("link", { name: /#general/ });
    await expect(row.locator(".dm-row-last")).toBeVisible();
    await row.hover();
    await page.mouse.down();
    await expect(row).toHaveAttribute("data-press", "down");
    // Check the actual dimmed press state, rather than waiting for the
    // release animation to end before measuring contrast.
    await page.evaluate(axe.source);
    const violations = await page.evaluate(async () => {
      // @ts-expect-error axe is the injected script's global
      const result = await window.axe.run(".dm", { resultTypes: ["violations"] });
      return result.violations.map((v: { id: string; nodes: unknown[] }) => `${v.id} (${v.nodes.length})`);
    });
    expect(violations).toEqual([]);
  } finally {
    await page.mouse.up();
    await context.close();
  }
});

for (const width of [1280, 390]) {
  test.describe(`at ${width}px`, () => {
    test("one inbox lists #general and encrypted conversations, asks for no API key, and passes axe", async ({ browser }) => {
      const db = new Database();
      const { context, page, requests } = await reader(browser, db, alice, width);
      await page.goto("/");
      expect(requests.some((u) => /\/assets\/stdb-/.test(u))).toBe(false);
      await page.goto("/messages");
      const list = page.getByRole("navigation", { name: "Conversations" });
      await expect(list.getByRole("link", { name: /#general/ })).toBeVisible();
      await expect(list.getByRole("link", { name: "New message" })).toBeVisible();
      await expect(page.getByText(/API key|wc_secret|Direct Cast/i)).toHaveCount(0);
      await list.getByRole("link", { name: /#general/ }).click();
      await expect(page).toHaveURL(/\/messages\?room=general$/);
      await expect(page.getByRole("heading", { name: "#general" })).toBeVisible();
      await expect(page.getByLabel("Message #general")).toBeVisible();
      await expect.poll(() => requests.filter((u) => /\/assets\/stdb-[0-9a-f]+\.js$/.test(u)).length).toBe(1);
      // It connects to Allthing's database on maincloud, and never to Farcaster's Direct Cast API.
      expect(await page.evaluate(() => (window as unknown as { __stdbTarget: object }).__stdbTarget)).toEqual({ uri: "wss://maincloud.spacetimedb.com", database: "allthing" });
      expect(requests.filter((u) => /api\.farcaster\.xyz\/(fc|v2\/ext-send)|\/api\/v1\/dc\b/.test(u))).toEqual([]);
      await page.evaluate(axe.source);
      const violations = await page.evaluate(async () => {
        // @ts-expect-error axe is the injected script's global
        const result = await window.axe.run(".dm", { resultTypes: ["violations"] });
        return result.violations.map((v: { id: string; nodes: unknown[] }) => `${v.id} (${v.nodes.length})`);
      });
      expect(violations).toEqual([]);
      await page.screenshot({ path: test.info().outputPath(`inbox-${width}.png`) });
      await context.close();
    });

    test("the general room is live between two readers, who are verified by the Worker's attestation", async ({ browser }) => {
      const db = new Database();
      db.verifyAttestations = true;
      const a = await reader(browser, db, alice, width);
      const b = await reader(browser, db, bob, width);
      await a.page.goto("/messages?room=general");
      await b.page.goto("/messages?room=general");
      const box = a.page.getByLabel("Message #general");
      await expect(box).toBeVisible();
      await box.fill("gm from Alice");
      await box.press("Enter");
      const bobsList = b.page.getByRole("list", { name: "Messages in #general" });
      await expect(bobsList.getByText("gm from Alice")).toBeVisible();
      await expect(bobsList.getByRole("link", { name: "Alice" })).toBeVisible();
      // Verified: no "unverified" badge, and the database took the calls in order.
      await expect(bobsList.getByText("unverified")).toHaveCount(0);
      const alicesIdentity = [...db.users.values()].find((u) => u.fid === alice.fid)!.identity;
      const alicesCalls = db.calls.filter((c) => c.identity === alicesIdentity).map((c) => c.op);
      expect(alicesCalls.slice(0, 3)).toEqual(["register", "verifyFid", "setPublicKey"]);
      // Bob's own verification follows his subscription; it comes too.
      await expect.poll(() => [...db.users.values()].map((u) => u.verified)).toEqual([true, true]);
      await b.page.screenshot({ path: test.info().outputPath(`chat-${width}.png`) });
      await a.context.close();
      await b.context.close();
    });

    test("encrypted messages go both ways, and the database keeps only ciphertext", async ({ browser }) => {
      const db = new Database();
      db.verifyAttestations = true;
      const a = await reader(browser, db, alice, width);
      const b = await reader(browser, db, bob, width);
      // Bob opens Messages once, so his browser has a key.
      await b.page.goto("/messages");
      await expect(b.page.getByRole("navigation", { name: "Conversations" })).toBeVisible();
      await expect.poll(() => db.keys.size).toBe(1);
      // New encrypted messages require canonical verified FIDs, so a
      // mutable development claim cannot evade another account's block.
      await expect(b.page.getByText("Accounts can't be verified here yet")).toHaveCount(0);

      await a.page.goto(`/messages?to=${bob.fid}`);
      const box = a.page.getByLabel("Encrypted message to @bob");
      await expect(box).toBeVisible();
      const secret = "the eagle lands at noon 🦅";
      await box.fill(secret);
      await box.press("Enter");
      await expect(a.page.getByRole("list", { name: "Encrypted messages with @bob" }).getByText(secret)).toBeVisible();

      // What the database got: ciphertext, a 24-byte nonce and the key versions; never the text.
      expect(db.dms).toHaveLength(1);
      const stored = JSON.stringify([...db.dms, ...db.calls]);
      expect(stored).not.toContain("eagle");
      expect(stored).not.toContain(Buffer.from("eagle").toString("hex"));
      const [dm] = db.dms;
      expect(dm.nonce).toMatch(/^[0-9a-f]{48}$/);
      // The text padded to 32 bytes, with Poly1305's 16-byte tag.
      expect((dm.ciphertext as string).length / 2).toBe(32 + 16);
      expect([dm.sender_key_version, dm.recipient_key_version]).toEqual([1, 1]);

      // Bob reads it, and answers.
      await b.page.goto(`/messages?to=${alice.fid}`);
      const bobsThread = b.page.getByRole("list", { name: "Encrypted messages with @alice" });
      await expect(bobsThread.getByText(secret)).toBeVisible();
      await b.page.getByLabel("Encrypted message to @alice").fill("copy that");
      await b.page.getByLabel("Encrypted message to @alice").press("Enter");
      await expect(a.page.getByRole("list", { name: "Encrypted messages with @bob" }).getByText("copy that")).toBeVisible();
      // Bob's conversation is in Alice's one inbox, beside #general.
      if (width >= 1280) await expect(a.page.getByRole("navigation", { name: "Conversations" }).getByRole("link", { name: /Bob/ })).toBeVisible();
      await a.page.screenshot({ path: test.info().outputPath(`encrypted-${width}.png`) });

      // The private keys stay in each browser, unexportable; only public halves went out.
      for (const [page, who] of [[a.page, alice], [b.page, bob]] as const) {
        const kept = await keptMessageKey(page, who.fid);
        expect(kept).toMatchObject({ algorithm: "X25519", extractable: false, usages: ["deriveBits"] });
        expect(kept!.public).toMatch(/^[0-9a-f]{64}$/);
        expect([...db.keys.values()].map((k) => k.x25519_pub)).toContain(kept!.public);
        expect(kept!.token).toMatch(/^token-c200/);
      }

      // A message changed on the way can't be read: the AEAD refuses it.
      const tampered = { ...db.dms[0], id: 99, ciphertext: (db.dms[0].ciphertext as string).replace(/^../, (h) => ((parseInt(h, 16) ^ 1) | 0x100).toString(16).slice(1)) };
      await db.push(tampered.recipient as string, { t: "insert", table: "my_dm_inbox", row: tampered });
      await expect(bobsThread.getByText(/can't be decrypted/)).toBeVisible();
      await a.context.close();
      await b.context.close();
    });

    test("the encryption explainer says how, and copies the full explanation for an agent", async ({ browser }) => {
      const db = new Database();
      const { context, page } = await reader(browser, db, alice, width);
      await page.goto("/messages");
      const lock = page.getByRole("button", { name: "End-to-end encrypted: how" });
      await expect(lock).toBeVisible();
      await lock.click();
      const panel = page.getByRole("dialog", { name: "How encrypted messages are encrypted" }).or(page.locator(".e2e-panel"));
      await expect(panel.first()).toBeVisible();
      for (const text of ["X25519", "HKDF-SHA256", "XChaCha20-Poly1305", "IndexedDB", "SpacetimeDB stores", "Only in your browser"]) {
        await expect(panel.first().getByText(text, { exact: false }).first()).toBeVisible();
      }
      await expect(panel.first().getByText("Paste this into your agent to have it walked through and verified.")).toBeVisible();
      await panel.first().getByRole("button", { name: "Copy technical explanation" }).click();
      await expect(panel.first().getByRole("button", { name: "Copied" })).toBeVisible();
      const copied = await page.evaluate(() => navigator.clipboard.readText());
      expect(copied).toContain("allthing-dm-v1");
      expect(copied).toContain("https://github.com/ael-dev3/Allthing");
      expect(copied).toContain("spacetime/spacetimedb/src/lib.rs");
      expect(copied).toContain("https://www.rfc-editor.org/rfc/rfc7748");
      expect(copied).toContain("Please walk me through this design and verify it");
      await page.screenshot({ path: test.info().outputPath(`explainer-${width}.png`) });
      await context.close();
    });

    test("remote account filters hide room mentions and blocked encrypted history without deleting messages", async ({ browser }) => {
      const db = new Database(); db.moderation = true; db.verifyAttestations = true;
      const a = await reader(browser, db, alice, width);
      const b = await reader(browser, db, bob, width);
      const errors: string[] = [];
      a.page.on("pageerror", error => errors.push(error.message));
      b.page.on("pageerror", error => errors.push(error.message));
      await a.page.addInitScript(() => {
        (window as any).__deriveCount = 0;
        const prototype = Object.getPrototypeOf(crypto.subtle), derive = prototype.deriveBits;
        prototype.deriveBits = function (...args: unknown[]) {
          (window as any).__deriveCount++;
          return Reflect.apply(derive, this, args);
        };
      });
      try {
        await b.page.goto("/messages");
        await expect.poll(() => db.keys.size).toBe(1);
        await a.page.goto(`/messages?to=${bob.fid}`);
        const composer = a.page.getByLabel("Encrypted message to @bob");
        await expect(composer).toBeVisible();
        await expect.poll(() => [...db.readers.values()].sort()).toEqual([alice.fid, bob.fid].sort());
        const secret = "Private history survives a temporary account block";
        await composer.fill(secret); await composer.press("Enter");
        await expect(a.page.getByRole("list", { name: "Encrypted messages with @bob" }).getByText(secret)).toBeVisible();
        expect(db.dms).toHaveLength(1);

        const aIdentity = [...db.users.values()].find(user => user.fid === alice.fid)!.identity as string;
        const bIdentity = [...db.users.values()].find(user => user.fid === bob.fid)!.identity as string;
        await db.call(bIdentity, "sendMessage", [1, "Room message by Bob"]);
        await db.call(aIdentity, "sendMessage", [1, "Room message mentioning @bob"]);
        await db.call(aIdentity, "sendMessage", [1, "An unrelated room message"]);
        await db.setFilter(alice.fid, bob.fid, 0, true, 0, bob.username);
        // Muting content does not prevent an encrypted conversation.
        await expect(composer).toBeVisible();
        await a.page.goto("/messages?room=general");
        const room = a.page.getByRole("list", { name: "Messages in #general" });
        await expect(room.getByText("An unrelated room message")).toBeVisible();
        await expect(room.getByText("Room message by Bob")).toHaveCount(0);
        await expect(room.getByText("Room message mentioning @bob")).toHaveCount(0);
        await a.page.goto("/messages?new=1");
        await a.page.getByRole("combobox", { name: "To", exact: true }).fill("bob");
        await expect(a.page.getByRole("option", { name: /Alice/ })).toBeVisible();
        await expect(a.page.getByRole("option", { name: /Bob/ })).toHaveCount(0);
        await db.setFilter(alice.fid, bob.fid, 0, false, 1, bob.username);
        await a.page.goto(`/messages?to=${bob.fid}`);
        await expect(a.page.getByRole("list", { name: "Encrypted messages with @bob" }).getByText(secret)).toBeVisible();
        await expect(a.page.getByLabel("Encrypted message to @bob")).toBeVisible();
        await db.setFilter(alice.fid, bob.fid, 1, true, 0, bob.username);
        await expect(a.page.getByText("Messages hidden by your account preferences.", { exact: true })).toBeVisible();
        await expect(a.page.getByText(secret, { exact: true })).toHaveCount(0);
        await expect(a.page.getByLabel("Encrypted message to @bob")).toHaveCount(0);
        await expect(a.page.getByRole("navigation", { name: "Conversations" }).getByRole("link", { name: /Bob/ })).toHaveCount(0);
        expect(db.dms).toHaveLength(1);

        await a.page.reload();
        await expect(a.page.getByText("Messages hidden by your account preferences.", { exact: true })).toBeVisible();
        await expect(a.page.getByText(secret, { exact: true })).toHaveCount(0);
        await db.setFilter(alice.fid, bob.fid, 1, false, 1, bob.username);
        await expect(a.page.getByRole("list", { name: "Encrypted messages with @bob" }).getByText(secret)).toBeVisible();
        expect(db.dms).toHaveLength(1);

        // A third-party mention changes privacy, not this conversation's
        // encrypted rows. Hide already decrypted text without navigation,
        // while preserving unrelated history, the composer and ciphertext.
        const mentioned = "An already decrypted message mentioning @carol";
        await composer.fill(mentioned); await composer.press("Enter");
        const thread = a.page.getByRole("list", { name: "Encrypted messages with @bob" });
        await expect(thread.getByText(mentioned, { exact: true })).toBeVisible();
        const encryptedHistory = db.dms.map(row => ({ ...row }));
        expect(encryptedHistory).toHaveLength(2);
        const unsent = "This unsent message keeps its text and focus";
        await composer.fill(unsent);
        await db.setFilter(alice.fid, 303, 0, true, 0, "carol");
        await expect(thread.getByText(mentioned, { exact: true })).toHaveCount(0);
        await expect(thread.getByText(secret, { exact: true })).toBeVisible();
        await expect(composer).toBeVisible();
        await expect(composer).toHaveValue(unsent);
        await expect(composer).toBeFocused();
        expect(new URL(a.page.url()).searchParams.get("to")).toBe(String(bob.fid));
        expect(db.dms).toEqual(encryptedHistory);
        await db.setFilter(alice.fid, 303, 0, false, 1, "carol");
        await expect(thread.getByText(mentioned, { exact: true })).toBeVisible();
        await expect(thread.getByText(secret, { exact: true })).toBeVisible();
        await expect(composer).toHaveValue(unsent);
        await expect(composer).toBeFocused();
        expect(db.dms).toEqual(encryptedHistory);
        await composer.fill("");

        // A newly arriving key/message from a blocked peer must never start
        // an X25519 derivation, even when it is not in the secret cache.
        await db.setFilter(alice.fid, bob.fid, 1, true, 2, bob.username);
        await expect(a.page.getByText("Messages hidden by your account preferences.", { exact: true })).toBeVisible();
        const count = await a.page.evaluate(() => (window as any).__deriveCount);
        const oldKey = db.keys.get(bIdentity)!;
        const newKey = { ...oldKey, key_version: 2, x25519_pub: "09".padEnd(64, "0") };
        db.keys.set(bIdentity, newKey); await db.broadcast("user_key", newKey, "update");
        await db.push(aIdentity, { t: "insert", table: "my_dm_inbox", row: {
          id: 999, sender: bIdentity, recipient: aIdentity, sender_key_version: 2, recipient_key_version: 1,
          nonce: "00".repeat(24), ciphertext: "00".repeat(32), sent_at: Date.now() * 1000,
        } });
        await a.page.waitForTimeout(150);
        expect(await a.page.evaluate(() => (window as any).__deriveCount)).toBe(count);
        await expect(a.page.getByText("Decrypting…", { exact: true })).toHaveCount(0);
        expect(errors).toEqual([]);
      } finally {
        await a.context.close(); await b.context.close();
      }
    });
  });
}
