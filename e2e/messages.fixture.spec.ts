// SPDX-License-Identifier: AGPL-3.0-or-later
import axe from "axe-core";
import { expect, test, type Page, type TestInfo } from "@playwright/test";
import { signIn } from "./helpers/farcaster";

/*
 * Messages: the reader's Direct Casts, with a Farcaster API key that only
 * their browser keeps. The Farcaster app's API is a stand-in here, in
 * memory, shaped as its beta docs show it (docs/DIRECT_CASTS.md); nothing
 * reaches api.farcaster.xyz. Reading works where the API answers this
 * origin (the stand-in says so with CORS headers); where it doesn't, as the
 * real one doesn't, the page sends only, with Programmable DCs, and says
 * why. People's names come from a stand-in Hypersnap. With DC_SHOTS set,
 * screenshots are saved there.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");
test.use({ timezoneId: "Europe/Berlin", locale: "en-GB" });

const API = "https://api.farcaster.xyz";
const NODE = "https://haatz.quilibrium.com";
const KEY = "wc_secret_fixture0123456789abcdefKEY";
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const bob = { fid: 77, username: "bob", display_name: "Bob" };
const people = [me, alice, bob];
const cors = { "access-control-allow-origin": "*" };
const now = () => Math.floor(Date.now() / 1000);
const SHOTS = process.env.DC_SHOTS;

type Message = {
  conversationId: string;
  messageId: string;
  senderFid: number;
  message: string;
  creationTimestamp: number;
  isDeleted: boolean;
  isProgrammatic: boolean;
  mentions: unknown[];
};
type Conversation = { conversationId: string; participantFids: number[]; creationTimestamp: number; messages: Message[] };

/** What the stand-ins were asked. */
type Calls = {
  keys: string[];
  reads: string[];
  sends: { body: { conversationId?: string; recipientFid?: number; message: string }; idempotency: string }[];
  ext: { recipientFid: number; message: string; idempotencyKey: string }[];
  allthing: { url: string; headers: Record<string, string> }[];
};

let serial = 0;
const id = () => (++serial).toString(16).padStart(32, "0");

function message(conversationId: string, senderFid: number, text: string, at: number): Message {
  return { conversationId, messageId: id(), senderFid, message: text, creationTimestamp: at, isDeleted: false, isProgrammatic: false, mentions: [] };
}

/** Alice wrote last; the reader wrote to Bob last. Alice's has 35 messages: two pages. */
function conversations(): Conversation[] {
  const t = now() - 3_600;
  const withAlice = "636f6e7669643a31323334342d3432";
  const withBob = "636f6e7669643a31323334342d3737";
  const aliceMessages = Array.from({ length: 35 }, (_, i) =>
    message(withAlice, i % 2 ? me.fid : alice.fid, i === 34 ? "Are you coming to the meetup?" : `Message number ${i + 1}`, t + i * 60),
  );
  return [
    { conversationId: withAlice, participantFids: [me.fid, alice.fid], creationTimestamp: t, messages: aliceMessages },
    { conversationId: withBob, participantFids: [me.fid, bob.fid], creationTimestamp: t - 86_400, messages: [message(withBob, me.fid, "See you tomorrow", t - 7_200)] },
  ];
}

const listed = (c: Conversation) => {
  const last = c.messages[c.messages.length - 1];
  const { conversationId, participantFids, creationTimestamp } = c;
  return { conversationId, participantFids, creationTimestamp, lastModifiedTimestamp: last?.creationTimestamp ?? creationTimestamp, lastMessage: last };
};

/**
 * The Direct Cast API and Programmable DCs, in memory. With `reads`, the
 * API answers this origin, as it would once it allowed allthing.social;
 * without, its answers carry no CORS header, so the browser can't read
 * them, as now. Programmable DCs always answer any origin. `failSends`
 * sends fail with 503 first; `refuse` recipients' settings refuse the
 * reader (403).
 */
async function farcaster(page: Page, { reads = true, failSends = 0, refuse = [] as number[] } = {}) {
  const state = conversations();
  const calls: Calls = { keys: [], reads: [], sends: [], ext: [], allthing: [] };
  let failing = failSends;
  await page.route(`${API}/**`, async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    calls.keys.push(request.headers()["authorization"] ?? "");
    const answer = (status: number, json: unknown, headers = cors) => route.fulfill({ status, headers, json });
    if (request.headers()["authorization"] !== `Bearer ${KEY}`) return answer(401, { errors: [{ message: "Authentication required" }] });
    if (url.pathname === "/v2/ext-send-direct-cast") {
      expect(request.method()).toBe("PUT");
      const body = request.postDataJSON();
      calls.ext.push(body);
      if (refuse.includes(body.recipientFid)) return answer(403, { errors: [{ message: "Recipient does not accept direct casts" }] });
      return answer(200, { result: { success: true } });
    }
    calls.reads.push(`${request.method()} ${url.pathname}${url.search}`);
    // As the real API refuses this origin (CORS), the browser reads nothing.
    if (!reads) return route.abort("failed");
    switch (url.pathname) {
      case "/fc/conversation-list": {
        const limit = Number(url.searchParams.get("limit") ?? 25);
        const sorted = [...state].sort((a, b) => listed(b).lastModifiedTimestamp - listed(a).lastModifiedTimestamp);
        return answer(200, { result: { conversations: sorted.slice(0, limit).map(listed) }, next: {} });
      }
      case "/fc/group-list":
        return answer(200, { result: { groups: [] }, next: {} });
      case "/fc/conversation": {
        const c = state.find((c) => c.conversationId === url.searchParams.get("conversationId"));
        return c ? answer(200, { result: { conversation: listed(c) } }) : answer(404, { errors: [{ message: "Not found" }] });
      }
      case "/fc/message-list": {
        const c = state.find((c) => c.conversationId === url.searchParams.get("conversationId"));
        if (!c) return answer(404, { errors: [{ message: "Not found" }] });
        const newest = [...c.messages].reverse();
        const from = Number(url.searchParams.get("cursor") ?? 0);
        const limit = Number(url.searchParams.get("limit") ?? 30);
        const next = from + limit < newest.length ? { cursor: String(from + limit) } : {};
        return answer(200, { result: { messages: newest.slice(from, from + limit) }, next });
      }
      case "/fc/message": {
        expect(request.method()).toBe("PUT");
        const body = request.postDataJSON();
        calls.sends.push({ body, idempotency: request.headers()["idempotency-key"] ?? "" });
        if (failing > 0) {
          failing -= 1;
          return answer(503, { errors: [{ message: "Unavailable" }] });
        }
        let c = state.find((c) => c.conversationId === body.conversationId);
        if (!c && body.recipientFid) {
          c = state.find((c) => c.participantFids.includes(body.recipientFid));
          if (!c) {
            c = { conversationId: `636f6e76${id()}`, participantFids: [me.fid, body.recipientFid], creationTimestamp: now(), messages: [] };
            state.push(c);
          }
        }
        if (!c) return answer(404, { errors: [{ message: "Not found" }] });
        const sent = message(c.conversationId, me.fid, body.message, now());
        c.messages.push(sent);
        return answer(200, { result: { messageId: sent.messageId, ...(body.recipientFid ? { conversationId: c.conversationId } : {}) } });
      }
    }
    return answer(404, { errors: [{ message: `Path ${url.pathname} does not exist` }] });
  });
  await page.route(`${NODE}/v2/farcaster/user/bulk?*`, (route) => {
    const fids = new URL(route.request().url()).searchParams.get("fids")!.split(",").map(Number);
    const users = people.filter((p) => fids.includes(p.fid)).map((p) => ({ ...p, pfp_url: `https://media.example/${p.username}.png` }));
    return route.fulfill({ headers: cors, json: { users } });
  });
  await page.route("https://media.example/**", (route) => {
    const name = new URL(route.request().url()).pathname.slice(1, 2).toUpperCase();
    const hue = { A: 262, B: 190, R: 32 }[name] ?? 0;
    const body = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40"><rect width="40" height="40" fill="hsl(${hue} 55% 62%)"/><text x="20" y="26" font-family="sans-serif" font-size="17" fill="#fff" text-anchor="middle">${name}</text></svg>`;
    return route.fulfill({ contentType: "image/svg+xml", body });
  });
  // Allthing's own API: the key must never be in what goes there.
  await page.route("**/api/v1/**", (route) => {
    calls.allthing.push({ url: route.request().url(), headers: route.request().headers() });
    return route.fulfill({ status: 404, json: { error: "Fixture endpoint" } });
  });
  await page.route("**/api/v1/home**", (route) => route.fulfill({ json: { pool: [], meta } }));
  await page.route("**/api/v1/search?*", (route) => {
    calls.allthing.push({ url: route.request().url(), headers: route.request().headers() });
    const q = new URL(route.request().url()).searchParams.get("q")!.toLowerCase();
    const users = [alice, bob].filter((p) => p.username.startsWith(q) || p.display_name.toLowerCase().startsWith(q));
    return route.fulfill({ json: { query: q, users, channels: [], casts: [], meta } });
  });
  return { state, calls };
}

/** The key as this browser keeps it, for `fid`. */
function keptKey(page: Page, fid: number) {
  return page.evaluate(
    (fid) =>
      new Promise<string | null>((resolve, reject) => {
        const open = indexedDB.open("allthing-dc", 1);
        open.onupgradeneeded = () => open.result.createObjectStore("keys", { keyPath: "fid" });
        open.onerror = () => reject(open.error);
        open.onsuccess = () => {
          const get = open.result.transaction("keys").objectStore("keys").get(fid);
          get.onsuccess = () => {
            open.result.close();
            resolve(get.result ? get.result.key : null);
          };
          get.onerror = () => reject(get.error);
        };
      }),
    fid,
  );
}

/** Keep the key for the reader, as pasting it does, before the page loads. */
async function withKey(page: Page) {
  await page.addInitScript(
    ({ fid, key }) => {
      const open = indexedDB.open("allthing-dc", 1);
      open.onupgradeneeded = () => open.result.createObjectStore("keys", { keyPath: "fid" });
      open.onsuccess = () => {
        open.result.transaction("keys", "readwrite").objectStore("keys").put({ fid, key });
        open.result.close();
      };
    },
    { fid: me.fid, key: KEY },
  );
}

/** What the reader saw of their conversations, long ago: what came since is unread. */
async function seenLongAgo(page: Page) {
  await page.addInitScript((fid) => {
    if (!localStorage.getItem(`allthing.dc-seen.${fid}`)) {
      localStorage.setItem(`allthing.dc-seen.${fid}`, JSON.stringify({ since: 1_600_000_000, at: {} }));
    }
  }, me.fid);
}

async function shot(page: Page, name: string, info: TestInfo) {
  if (!SHOTS) return;
  await page.evaluate(() => document.fonts.ready);
  await page.screenshot({ path: `${SHOTS}/${name}.png` });
  await info.attach(name, { path: `${SHOTS}/${name}.png`, contentType: "image/png" });
}

/** Accessibility problems axe finds in the page's `scope`. */
async function axeViolations(page: Page, scope = ".dm") {
  // Once the page has come in: it rises from transparent, and axe weighs
  // contrast through opacity, so text caught on the way in reads too faint.
  // (Endless animations, a spinner's, never finish and aren't waited for.)
  await page.evaluate(() =>
    Promise.all(
      document
        .getAnimations()
        .filter((a) => a.effect?.getComputedTiming().endTime !== Infinity)
        .map((a) => a.finished.catch(() => undefined)),
    ),
  );
  await page.evaluate(axe.source);
  return page.evaluate(async (scope) => {
    // @ts-expect-error axe is the injected script's global
    const result = await window.axe.run(scope, { resultTypes: ["violations"] });
    return result.violations.map((v: { id: string; nodes: { target: string[] }[] }) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`);
  }, scope);
}

/** The smallest side of `selector`'s targets, as laid out. */
const minSize = async (page: Page, selector: string) => {
  const boxes = await page.locator(selector).evaluateAll((els) => els.filter((e) => e.getBoundingClientRect().width > 0).map((e) => e.getBoundingClientRect()));
  expect(boxes.length).toBeGreaterThan(0);
  return Math.min(...boxes.map((b) => Math.min(b.width, b.height)));
};

/** The least height a tap on `selector`'s targets has: their own, or their tap band's (::after). */
const tapHeight = async (page: Page, selector: string) => {
  const heights = await page.locator(selector).evaluateAll((els) =>
    els
      .filter((e) => e.getBoundingClientRect().width > 0)
      .map((e) => {
        const band = getComputedStyle(e, "::after");
        const banded = band.content !== "none" && band.position === "absolute" ? parseFloat(band.height) : 0;
        return Math.max(e.getBoundingClientRect().height, banded);
      }),
  );
  expect(heights.length).toBeGreaterThan(0);
  return Math.min(...heights);
};

test("a pasted key is kept in this browser only, and forgotten from the page, the menu and by signing out", async ({ page }) => {
  await signIn(page, me);
  const { calls } = await farcaster(page);
  await page.goto("/messages");
  const setup = page.getByRole("region", { name: "Read and send your Direct Casts here" });
  await expect(setup).toBeVisible();
  await expect(setup.getByRole("link", { name: "farcaster.xyz/~/developers/api-keys" })).toHaveAttribute("href", "https://farcaster.xyz/~/developers/api-keys");
  await expect(setup.getByRole("link", { name: "Settings → Developer tools" })).toHaveAttribute("href", "https://farcaster.xyz/~/settings/developer-tools");
  await expect(setup).toContainText("“Allthing”");
  await expect(setup).toContainText("shows only once");
  await expect(setup).toContainText("It can read and send your Direct Casts");
  await expect(setup).toContainText("It goes from here to Farcaster's API, and nowhere else.");

  // Not a key: said so, and nothing kept.
  const field = setup.getByLabel("Your Farcaster API key");
  await field.fill("sk_live_not_farcaster");
  await setup.getByRole("button", { name: "Keep key" }).click();
  await expect(setup.getByRole("alert")).toHaveText("That isn't a Farcaster API key: a key starts with wc_secret_.");
  expect(await keptKey(page, me.fid)).toBeNull();

  // A key: kept, and the conversations come.
  await field.fill(`  ${KEY}\n`);
  await setup.getByRole("button", { name: "Keep key" }).click();
  await expect(page.getByRole("navigation", { name: "Conversations" }).getByRole("link", { name: /Alice/ })).toBeVisible();
  expect(await keptKey(page, me.fid)).toBe(KEY);
  // It went to Farcaster's API alone, as a bearer token, and never to Allthing.
  expect(new Set(calls.keys)).toEqual(new Set([`Bearer ${KEY}`]));
  expect(JSON.stringify(calls.allthing)).not.toContain(KEY);
  expect(JSON.stringify(calls.allthing)).not.toContain(KEY.slice(10));

  // Forget key, on the page.
  await page.getByRole("button", { name: "Forget key" }).click();
  await expect(setup).toBeVisible();
  await expect.poll(() => keptKey(page, me.fid)).toBeNull();

  // The account menu's Forget Direct Cast key.
  await field.fill(KEY);
  await setup.getByRole("button", { name: "Keep key" }).click();
  await expect(page.getByRole("button", { name: "Forget key" })).toBeVisible();
  await page.locator(".account-button").hover();
  await page.getByRole("button", { name: "Forget Direct Cast key" }).click();
  await expect(setup).toBeVisible();
  await expect.poll(() => keptKey(page, me.fid)).toBeNull();

  // Signing out forgets it too.
  await field.fill(KEY);
  await setup.getByRole("button", { name: "Keep key" }).click();
  await expect(page.getByRole("button", { name: "Forget key" })).toBeVisible();
  await page.locator(".account-button").hover();
  await page.getByRole("link", { name: "Messages" }).waitFor();
  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page.getByRole("heading", { name: "Your Direct Casts are in the Farcaster app" })).toBeVisible();
  await expect.poll(() => keptKey(page, me.fid)).toBeNull();
});

for (const width of [1280, 390]) {
  test.describe(`at ${width}`, () => {
    // At 390, a phone: a touch screen, whose targets take a fingertip.
    test.use(width === 390 ? { viewport: { width, height: 844 }, isMobile: true, hasTouch: true } : { viewport: { width, height: 900 } });

    test("signed out, the page hands Direct Casts to the Farcaster app", async ({ page }) => {
      const { calls } = await farcaster(page);
      await page.goto("/messages");
      await expect(page.getByRole("heading", { level: 1, name: "Messages" })).toBeVisible();
      await expect(page.getByRole("heading", { name: "Your Direct Casts are in the Farcaster app" })).toBeVisible();
      await expect(page.getByRole("link", { name: "Open your inbox in Farcaster" })).toHaveAttribute("href", "https://farcaster.xyz/~/inbox");
      await expect(page.locator(".dm").getByRole("button", { name: "Sign in" })).toBeVisible();
      expect(calls.keys).toEqual([]);
      expect(await axeViolations(page)).toEqual([]);
      if (width === 390) expect(await minSize(page, ".dm-actions .button")).toBeGreaterThanOrEqual(44);
    });

    test("the key: how to make one, pasted, and forgotten", async ({ page }, info) => {
      await signIn(page, me);
      await farcaster(page);
      await page.goto("/messages");
      const setup = page.getByRole("region", { name: "Read and send your Direct Casts here" });
      await expect(setup).toBeVisible();
      await shot(page, `key-setup-${width}`, info);
      expect(await axeViolations(page)).toEqual([]);
      if (width === 390) {
        expect(await minSize(page, ".dm-key-row input, .dm-key-row .button")).toBeGreaterThanOrEqual(44);
      }
      await setup.getByLabel("Your Farcaster API key").fill(KEY);
      await setup.getByRole("button", { name: "Keep key" }).click();
      await expect(page.getByRole("navigation", { name: "Conversations" }).locator(".dm-row")).toHaveCount(2);
      expect(await keptKey(page, me.fid)).toBe(KEY);
      if (width === 390) expect(await tapHeight(page, ".dm-forget")).toBeGreaterThanOrEqual(44);

      // The account menu, opened as a mouse or a finger does: Messages
      // under Your profile, and, with a key kept, Forget Direct Cast key.
      const picture = page.getByRole("link", { name: "Signed in as @reader: your profile" });
      if (width === 390) {
        const box = (await picture.boundingBox())!;
        await page.touchscreen.tap(box.x + box.width / 2, box.y + box.height / 2);
      } else {
        await picture.hover();
      }
      const menu = page.getByRole("dialog", { name: "Your account" });
      await expect(menu).toBeInViewport({ ratio: 1 });
      // Measured once it has finished opening.
      await menu.evaluate((el) => Promise.all(el.getAnimations().map((a) => a.finished)));
      await expect(menu.getByRole("button", { name: "Forget Direct Cast key" })).toBeVisible();
      const items = await menu.locator(".account-link").allTextContents();
      expect(items.slice(0, 2)).toEqual(["Your profile", "Messages"]);
      expect(items.at(-1)).toBe("Sign out");
      await expect(menu.getByRole("link", { name: "Messages" })).toHaveAttribute("href", "/messages");
      if (width === 390) {
        expect(await minSize(page, ".account-menu a.account-link[href='/messages'], .account-menu .forget-dc-key")).toBeGreaterThanOrEqual(44);
      }
      await shot(page, `account-menu-${width}`, info);
      await page.keyboard.press("Escape");
      await expect(menu).toBeHidden();

      await page.getByRole("button", { name: "Forget key" }).click();
      await expect(setup).toBeVisible();
      await expect.poll(() => keptKey(page, me.fid)).toBeNull();
    });

    test("the conversations, a thread, and a message sent, retried and read back", async ({ page }, info) => {
      await signIn(page, me);
      await withKey(page);
      await seenLongAgo(page);
      const { state, calls } = await farcaster(page, { failSends: 1 });
      await page.goto("/messages");
      const list = page.getByRole("navigation", { name: "Conversations" });
      const rows = list.locator(".dm-row");
      await expect(rows).toHaveCount(2);
      // Newest first, with names, last messages and when; Alice's unread.
      await expect(rows.nth(0)).toContainText("Alice");
      await expect(rows.nth(0)).toContainText("Are you coming to the meetup?");
      await expect(rows.nth(0)).toHaveClass(/dm-unread/);
      await expect(rows.nth(0).locator(".dm-dot")).toBeVisible();
      await expect(rows.nth(1)).toContainText("Bob");
      await expect(rows.nth(1)).toContainText("You: See you tomorrow");
      await expect(rows.nth(1)).not.toHaveClass(/dm-unread/);
      // A fingertip's size on a phone.
      if (width === 390) expect(await minSize(page, ".dm-row")).toBeGreaterThanOrEqual(44);

      await rows.nth(0).click();
      await expect(page).toHaveURL(/\/messages\?c=636f6e7669643a31323334342d3432$/);
      const thread = page.getByRole("region", { name: "Messages with Alice" });
      await expect(thread.getByRole("heading", { name: "Alice" })).toBeVisible();
      await expect(thread.getByRole("link", { name: "@alice" })).toHaveAttribute("href", "/u/alice");
      await expect(thread.getByRole("link", { name: "Open in Farcaster" })).toHaveAttribute("href", "https://farcaster.xyz/~/inbox/636f6e7669643a31323334342d3432");
      const bubbles = thread.locator(".dm-message");
      await expect(bubbles).toHaveCount(30);
      await expect(bubbles.last()).toContainText("Are you coming to the meetup?");
      // Alice's on the left, the reader's on the right.
      const scroller = await thread.locator(".dm-scroll").boundingBox();
      const theirs = await bubbles.last().locator(".dm-bubble").boundingBox();
      const mine = await thread.locator(".dm-message.dm-mine").last().locator(".dm-bubble").boundingBox();
      expect(theirs!.x - scroller!.x).toBeLessThan(40);
      expect(scroller!.x + scroller!.width - (mine!.x + mine!.width)).toBeLessThan(40);
      // Seen now: no dot.
      if (width === 1280) await expect(rows.nth(0)).not.toHaveClass(/dm-unread/);

      // Older messages, a page at a time.
      await thread.getByRole("button", { name: "Load older messages" }).click();
      await expect(bubbles).toHaveCount(35);
      await expect(bubbles.first()).toContainText("Message number 1");
      await expect(thread.getByRole("button", { name: "Load older messages" })).toHaveCount(0);

      // Send: shown at once, failed once, then sent with the same idempotency key.
      const box = thread.getByRole("textbox", { name: "Message @alice" });
      await box.fill("Yes! I'll bring the zines.");
      await box.press("Enter");
      const outgoing = thread.locator(".dm-outgoing");
      await expect(outgoing).toContainText("Yes! I'll bring the zines.");
      await expect(outgoing.getByRole("alert")).toHaveText("Farcaster's API isn't answering just now. Try again shortly.");
      await expect(box).toHaveValue("");
      await shot(page, `thread-retry-${width}`, info);
      await outgoing.getByRole("button", { name: "Try again" }).click();
      await expect(outgoing).toHaveCount(0);
      await expect(bubbles.last()).toContainText("Yes! I'll bring the zines.");
      await expect(bubbles.last()).toHaveClass(/dm-mine/);
      expect(calls.sends).toHaveLength(2);
      expect(calls.sends[0].idempotency).toMatch(/^[0-9a-f-]{36}$/);
      expect(calls.sends[1].idempotency).toBe(calls.sends[0].idempotency);
      expect(calls.sends[1].body).toEqual({ conversationId: "636f6e7669643a31323334342d3432", message: "Yes! I'll bring the zines." });

      // Shift+Enter is a new line; Enter sends.
      await box.pressSequentially("line one");
      await box.press("Shift+Enter");
      await box.pressSequentially("line two");
      await expect(box).toHaveValue("line one\nline two");
      await box.press("Enter");
      await expect(bubbles.last()).toContainText("line one");
      await expect.poll(() => calls.sends.at(-1)!.body.message).toBe("line one\nline two");

      // 1,024 characters at most.
      await box.fill("x".repeat(1025));
      await expect(thread.locator(".dm-left")).toHaveText("1 over");
      await expect(thread.getByRole("button", { name: "Send" })).toBeDisabled();
      await box.fill("x".repeat(1000));
      await expect(thread.locator(".dm-left")).toHaveText("24 left");
      await box.fill("");

      // Alice answers: it comes when the page asks again, and is read out.
      const alices = state[0];
      alices.messages.push(message(alices.conversationId, alice.fid, "Great, see you there", now()));
      await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange", { bubbles: true })));
      await expect(bubbles.last()).toContainText("Great, see you there");
      await expect(page.locator(".dm > [role=status][aria-live=polite]")).toHaveText("New message from @alice: Great, see you there");

      await shot(page, `list-thread-${width}`, info);
      if (width === 390) {
        // One at a time: the thread, then back to the list.
        await expect(list).toBeHidden();
        expect(await minSize(page, ".dm-back, .dm-send")).toBeGreaterThanOrEqual(44);
        expect(await tapHeight(page, ".dm-thread-handle, .dm-open-app")).toBeGreaterThanOrEqual(44);
        await thread.getByRole("link", { name: "Back to your conversations" }).click();
        await expect(list).toBeVisible();
        await expect(page).toHaveURL(/\/messages$/);
      } else {
        await expect(list).toBeVisible();
      }
      expect(await axeViolations(page)).toEqual([]);
      expect(JSON.stringify(calls.allthing)).not.toContain(KEY);
    });

    test("where nothing reads, it says why and sends with Programmable DCs, saying clearly when a recipient's settings refuse", async ({ page }, info) => {
      await signIn(page, me);
      await withKey(page);
      const { calls } = await farcaster(page, { reads: false, refuse: [bob.fid] });
      await page.goto("/messages");
      const panel = page.getByRole("region", { name: "Reading your Direct Casts in Allthing needs Farcaster's API to allow it" });
      await expect(panel).toBeVisible();
      await expect(panel).toContainText("so allthing.social can't show yours");
      await expect(panel.getByRole("link", { name: "Open your inbox in Farcaster" })).toHaveAttribute("href", "https://farcaster.xyz/~/inbox");
      // The read was tried once, and refused: the relay is off here.
      expect(calls.reads).toEqual(["GET /fc/conversation-list?limit=1"]);

      // A new message, to someone found by name.
      const writing = page.getByRole("region", { name: "New message" });
      const to = writing.getByRole("combobox", { name: "To" });
      await to.pressSequentially("bo", { delay: 30 });
      const options = writing.getByRole("listbox", { name: "People" }).getByRole("option");
      await expect(options).toHaveCount(1);
      await expect(options.first()).toContainText("@bob");
      await to.press("ArrowDown");
      await to.press("Enter");
      await expect(writing).toContainText("To Bob@bob");
      const box = writing.getByRole("textbox", { name: "Message @bob" });
      await expect(box).toBeFocused();
      await box.fill("Hey Bob");
      await box.press("Enter");
      const refused = writing.locator(".dm-outgoing.dm-failed");
      await expect(refused.getByRole("alert")).toHaveText(
        "@bob's Direct Cast settings don't take messages from you: they may take messages only from people they follow.",
      );
      await shot(page, `send-only-${width}`, info);

      // To Alice, it goes.
      await writing.getByRole("button", { name: "Change" }).click();
      await writing.getByRole("combobox", { name: "To" }).pressSequentially("ali", { delay: 30 });
      await writing.getByRole("option", { name: /Alice/ }).click();
      await writing.getByRole("textbox", { name: "Message @alice" }).fill("Hello from Allthing");
      await writing.getByRole("textbox", { name: "Message @alice" }).press("Enter");
      await expect(writing.locator(".dm-outgoing")).toContainText("Sent");
      expect(calls.ext).toHaveLength(2);
      expect(calls.ext[1]).toMatchObject({ recipientFid: alice.fid, message: "Hello from Allthing" });
      expect(calls.ext[1].idempotencyKey).toMatch(/^[0-9a-f-]{36}$/);
      expect(calls.reads).toEqual(["GET /fc/conversation-list?limit=1"]);
      expect(await axeViolations(page)).toEqual([]);
      if (width === 390) {
        expect(await minSize(page, ".dm-send")).toBeGreaterThanOrEqual(44);
        expect(await tapHeight(page, ".dm-forget, .dm-to .link-button, .dm-failure-actions .link-button")).toBeGreaterThanOrEqual(44);
      }
    });

    test("a new message to someone found by name opens its conversation", async ({ page }) => {
      await signIn(page, me);
      await withKey(page);
      const { calls } = await farcaster(page);
      await page.goto("/messages");
      await page.getByRole("link", { name: "New message" }).click();
      await expect(page).toHaveURL(/\/messages\?new=1$/);
      const writing = page.getByRole("region", { name: "New message" });
      await writing.getByRole("combobox", { name: "To" }).pressSequentially("alice", { delay: 30 });
      await expect(writing.getByRole("option", { name: /Alice/ })).toBeVisible();
      // Enter on a name typed in full picks it.
      await writing.getByRole("combobox", { name: "To" }).press("Enter");
      const box = writing.getByRole("textbox", { name: "Message @alice" });
      await box.fill("A new thought");
      await box.press("Enter");
      await expect(page).toHaveURL(/\/messages\?c=636f6e7669643a31323334342d3432$/);
      const thread = page.getByRole("region", { name: "Messages with Alice" });
      await expect(thread.locator(".dm-message").last()).toContainText("A new thought");
      expect(calls.sends.at(-1)!.body).toEqual({ recipientFid: alice.fid, message: "A new thought" });
    });
  });
}
