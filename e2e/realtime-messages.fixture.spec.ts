// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type BrowserContext, type Page } from "@playwright/test";
import { signIn } from "./helpers/farcaster";

/*
 * Chat and Encrypted on /messages, on Allthing's SpacetimeDB database,
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
  pages = new Map<string, Page>();
  tokens = new Map<string, string>();
  calls: { identity: string; op: string; args: unknown[] }[] = [];

  connect(page: Page, token: string) {
    let identity = this.tokens.get(token);
    if (!identity) {
      identity = id(this.tokens.size + 1);
      token = `token-${identity}`;
      this.tokens.set(token, identity);
    }
    this.pages.set(identity, page);
    return { identity, token };
  }

  async push(identity: string, event: object) {
    const page = this.pages.get(identity);
    if (page && !page.isClosed()) await page.evaluate((e) => (window as unknown as { __stdbEmit: (j: string) => void }).__stdbEmit(e), JSON.stringify(event)).catch(() => {});
  }

  async broadcast(table: string, row: Row, op = "insert") {
    for (const identity of this.pages.keys()) await this.push(identity, { t: op, table, row });
  }

  async snapshot(identity: string) {
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
        this.user(identity);
        const [recipient, skv, rkv, nonce, ciphertext] = args as [string, number, number, string, string];
        if (recipient === identity) throw new Error("cannot DM self");
        if (!this.keys.has(recipient)) throw new Error("recipient has no key");
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
const call = (op, args) => window.__stdb(op, JSON.stringify(args));
const settle = (op, args) => call(op, args).then(() => JSON.stringify({ ok: true }), (e) => JSON.stringify({ ok: false, error: String(e && e.message || e).replace(/^Error: /, "") }));
window.allthingStdb = {
  version: 1,
  connect(uri, database, token, emit) {
    window.__stdbEmit = emit;
    window.__stdbTarget = { uri, database };
    const bridge = {
      connected: call("connect", [token]),
      subscribe: (queries) => call("subscribe", [JSON.parse(queries)]),
      disconnect() {},
    };
    for (const op of ["register", "verifyFid", "setPublicKey", "sendMessage", "setTyping", "sendDm", "deleteDm"]) {
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
  let identity = "";
  await page.exposeBinding("__stdb", async (_source, op: string, json: string) => {
    const args = JSON.parse(json) as unknown[];
    if (op === "connect") {
      const c = db.connect(page, args[0] as string);
      identity = c.identity;
      return JSON.stringify(c);
    }
    if (op === "subscribe") return db.snapshot(identity);
    return db.call(identity, op, args);
  });
  await page.route("**/assets/stdb-*.js", (route) => route.fulfill({ contentType: "text/javascript", body: FAKE_BRIDGE }));
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

for (const width of [1280, 390]) {
  test.describe(`at ${width}px`, () => {
    test("Direct Casts stays the first tab, and nothing of SpacetimeDB loads until Chat or Encrypted opens", async ({ browser }) => {
      const db = new Database();
      const { context, page, requests } = await reader(browser, db, alice, width);
      await page.goto("/");
      await page.goto("/messages");
      const tabs = page.getByRole("navigation", { name: "Messages" });
      await expect(tabs.getByRole("link", { name: "Direct Casts" })).toHaveAttribute("aria-current", "page");
      await expect(page.getByText("Your Direct Casts, Allthing's room and encrypted messages.")).toBeVisible();
      expect(requests.some((u) => /\/assets\/stdb-/.test(u))).toBe(false);
      await tabs.getByRole("link", { name: "Chat" }).click();
      await expect(page).toHaveURL(/\/messages\?tab=chat$/);
      await expect(page.getByRole("heading", { name: "#general" })).toBeVisible();
      await expect.poll(() => requests.filter((u) => /\/assets\/stdb-[0-9a-f]+\.js$/.test(u)).length).toBe(1);
      // It connects to Allthing's database on maincloud.
      expect(await page.evaluate(() => (window as unknown as { __stdbTarget: object }).__stdbTarget)).toEqual({ uri: "wss://maincloud.spacetimedb.com", database: "allthing" });
      await context.close();
    });

    test("the general room is live between two readers, who are verified by the Worker's attestation", async ({ browser }) => {
      const db = new Database();
      db.verifyAttestations = true;
      const a = await reader(browser, db, alice, width);
      const b = await reader(browser, db, bob, width);
      await a.page.goto("/messages?tab=chat");
      await b.page.goto("/messages?tab=chat");
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
      const a = await reader(browser, db, alice, width);
      const b = await reader(browser, db, bob, width);
      // Bob opens Encrypted once, so his browser has a key.
      await b.page.goto("/messages?tab=encrypted");
      await expect(b.page.getByRole("heading", { name: "Encrypted messages" })).toBeVisible();
      await expect.poll(() => db.keys.size).toBe(1);
      // Without the Worker's attestation key, names are as people claim them, and the page says so.
      await expect(b.page.getByText("Accounts can't be verified here yet")).toBeVisible();

      await a.page.goto(`/messages?tab=encrypted&to=${bob.fid}`);
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
      await b.page.goto(`/messages?tab=encrypted&to=${alice.fid}`);
      const bobsThread = b.page.getByRole("list", { name: "Encrypted messages with @alice" });
      await expect(bobsThread.getByText(secret)).toBeVisible();
      await b.page.getByLabel("Encrypted message to @alice").fill("copy that");
      await b.page.getByLabel("Encrypted message to @alice").press("Enter");
      await expect(a.page.getByRole("list", { name: "Encrypted messages with @bob" }).getByText("copy that")).toBeVisible();
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
      await page.goto("/messages?tab=encrypted");
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
  });
}
