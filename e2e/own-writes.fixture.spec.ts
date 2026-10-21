// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type Route, type TestInfo } from "@playwright/test";
import { approvedKey, cast as castOf, envelope, reaction, signIn, type Envelope } from "./helpers/farcaster";

/*
 * What the reader writes from Allthing with their key shows at once where
 * it belongs, and after a reload too: a cast above the feed and on their
 * own profile, a reply under its post and on their profile's Replies tab,
 * a like in its count. The Worker is asked to refresh the pages a write
 * changes (POST /api/v1/refresh), and the browser reads them again past
 * its cache. The node and the Worker are stand-ins, and nothing is posted.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = Math.floor(Date.now() / 1000) - 600;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const post = { hash: hex(1), author: alice, text: "What should I read next?", ts, likes: 3, recasts: 0, replies: 0 };
const older = { hash: hex(9), author: me, text: "An older cast of mine", ts: ts - 3600, likes: 0, recasts: 0, replies: 0 };
const NODE = "https://haatz.quilibrium.com/v1";
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" };

type Calls = { sent: Envelope[]; refreshed: object[] };

/**
 * The Worker and the node. Until `fresh()` says so, the Worker's pages
 * don't have the reader's new casts or likes, as a copy cached before
 * them wouldn't.
 */
async function serve(page: Page, fresh: () => boolean) {
  const calls: Calls = { sent: [], refreshed: [] };
  let key = "";
  const preflight = (route: Route) => route.request().method() === "OPTIONS" && route.fulfill({ status: 204, headers: cors });
  // The reader's casts as the node has them, and what they like.
  const posted = () =>
    calls.sent
      .filter((sent) => sent.type === 1)
      .map((sent) => {
        const { text, parent } = castOf(sent);
        return { hash: sent.hash, author: me, text, ts: sent.at, likes: 0, recasts: 0, replies: 0, ...(parent?.cast ? { parent_hash: parent.cast.hash } : {}) };
      });
  const liked = () => {
    const on = new Set<string>();
    for (const sent of calls.sent.filter((s) => s.type === 3 || s.type === 4)) {
      const { target } = reaction(sent);
      if (sent.type === 3) on.add(target.hash);
      else on.delete(target.hash);
    }
    return on;
  };
  // The API paths the app reads again past the browser's cache.
  await page.addInitScript(() => {
    const fetch = window.fetch.bind(window);
    (window as unknown as { reloads: string[] }).reloads = [];
    window.fetch = (input: RequestInfo | URL, init?: RequestInit) => {
      if (init?.cache === "reload") (window as unknown as { reloads: string[] }).reloads.push(String(input));
      return fetch(input, init);
    };
  });
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    const likes = post.likes + (fresh() && liked().has(post.hash) ? 1 : 0);
    return route.fulfill({ json: { pool: later ? [] : [{ ...post, likes }], notes: {}, meta } });
  });
  await page.route("**/api/v1/user?*", (route) => {
    const pool = [...(fresh() ? posted().reverse() : []), older];
    const user = { ...me, bio: "", follower_count: 5, following_count: 3 };
    return route.fulfill({ json: { user, pool, notes: {}, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => {
    const children = fresh() ? posted().filter((c) => "parent_hash" in c).map((cast) => ({ cast, children: [] })) : [];
    return route.fulfill({ json: { thread: { cast: { ...post, replies: children.length }, children }, notes: {}, meta } });
  });
  await page.route("**/api/v1/replies?*", (route) => {
    return route.fulfill({ json: { hash: post.hash, replies: [], more: false, meta } });
  });
  await page.route("**/api/v1/reactions?*", (route) => {
    const url = new URL(route.request().url());
    const hash = url.searchParams.get("hash")!;
    const kind = url.searchParams.get("type")!;
    const users = kind === "likes" && fresh() && liked().has(hash) ? [me] : [];
    return route.fulfill({ json: { hash, kind, users, more: false, meta } });
  });
  await page.route("**/api/v1/refresh", (route) => {
    expect(route.request().method()).toBe("POST");
    calls.refreshed.push(route.request().postDataJSON());
    return route.fulfill({ json: { expired: 1 } });
  });
  await page.route("**/api/v1/signer", (route) => route.fulfill({ json: { app_fid: 3356318 } }));
  await page.route("**/api/v1/auth/signer", (route) => {
    key = route.request().postDataJSON().key;
    return route.fulfill({ json: { key, requestFid: 3356318, signature: `0x${"5a".repeat(65)}`, deadline: Math.floor(Date.now() / 1000) + 3600 } });
  });
  await page.route(`${NODE}/signersByFid?*`, (route) => route.fulfill({ headers: cors, json: { signers: [{ key }] } }));
  await page.route(`${NODE}/reactionsByFid?*`, (route) => {
    const kind = new URL(route.request().url()).searchParams.get("reaction_type");
    const messages =
      kind === "Like"
        ? [...liked()].map((hash) => ({ data: { type: "MESSAGE_TYPE_REACTION_ADD", fid: me.fid, reactionBody: { type: "REACTION_TYPE_LIKE", targetCastId: { fid: alice.fid, hash } } } }))
        : [];
    return route.fulfill({ headers: cors, json: { messages } });
  });
  await page.route(`${NODE}/reactionById?*`, (route) =>
    route.fulfill({ status: 400, headers: cors, json: { error: "Failed to get reaction", error_detail: 'status: NotFound, message: "not found"' } }),
  );
  await page.route(`${NODE}/userNameProofByName?*`, (route) =>
    route.fulfill({ status: 400, headers: cors, json: { error: "Failed to get username proof", error_detail: 'status: NotFound, message: "Username proof not found"' } }),
  );
  await page.route(`${NODE}/submitMessage`, async (route) => {
    if (await preflight(route)) return;
    calls.sent.push(envelope(route.request().postDataBuffer()!));
    return route.fulfill({ headers: cors, json: {} });
  });
  return calls;
}

/** Signed in, with a key approved an hour ago, on `path`. */
async function ready(page: Page, path: string, fresh: () => boolean) {
  await signIn(page, me);
  const calls = await serve(page, fresh);
  await page.goto(path);
  await expect(page.locator(".cast-card:not(.skeleton), .post").first()).toBeVisible();
  await approvedKey(page, me.fid, 3_600_000);
  await page.reload();
  await expect(page.locator(".cast-card:not(.skeleton), .post").first()).toBeVisible();
  return calls;
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  return { context, page: await context.newPage() };
}

const outgoing = (page: Page) => page.locator(".cast-card.outgoing");
/** The API paths this page load read again past the browser's cache. */
const reloads = (page: Page) => page.evaluate(() => (window as unknown as { reloads: string[] }).reloads);

/** Post `text` from the composer the header's (or, on a phone, the floating) button opens. */
async function castFromHome(page: Page, text: string) {
  await page.getByRole("button", { name: "Write a cast" }).first().click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await dialog.getByRole("textbox").fill(text);
  await dialog.getByRole("button", { name: "Cast", exact: true }).click();
  await expect(dialog).toBeHidden();
}

for (const atPhone of [false, true]) {
  test(`${atPhone ? "at 390" : "at 1280"}, a cast shows at once above the feed and on the reader's profile, and after a reload`, async ({ page: desktop, browser }, testInfo) => {
    const { context, page } = atPhone ? await phone(browser, testInfo) : { context: null, page: desktop };
    let fresh = false;
    const calls = await ready(page, "/", () => fresh);
    await castFromHome(page, "gm from Allthing");
    const hash = calls.sent[0].hash;
    // Above the feed at once, as the reader's, with a link to it.
    await expect(outgoing(page)).toHaveCount(1);
    await expect(outgoing(page)).toContainText("gm from Allthing");
    await expect(outgoing(page).getByRole("link", { name: "Posted" })).toHaveAttribute("href", `/p/${hash}`);
    await page.screenshot({ path: testInfo.outputPath(`feed-${atPhone ? 390 : 1280}.png`) });

    // On their own profile at once, above the casts it read before, on
    // the Casts tab and not the Replies tab.
    await outgoing(page).locator("a.author").click();
    await expect(page.locator(".profile")).toBeVisible();
    await expect(outgoing(page)).toContainText("gm from Allthing");
    await expect(page.locator(`.cast-card[data-hash="${older.hash}"]`)).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath(`profile-${atPhone ? 390 : 1280}.png`) });
    await page.getByRole("link", { name: "Replies", exact: true }).click();
    await expect(outgoing(page)).toHaveCount(0);
    await page.getByRole("link", { name: "Casts", exact: true }).click();
    await expect(outgoing(page)).toHaveCount(1);

    // The Worker is asked to refresh the reader's profile, which is read
    // again past the browser's cache, and again while it doesn't have it.
    await expect.poll(() => calls.refreshed, { timeout: 10_000 }).toContainEqual({ username: me.username });
    await expect.poll(async () => (await reloads(page)).filter((path) => path === `/api/v1/user?name=${me.username}`).length, { timeout: 15_000 }).toBeGreaterThanOrEqual(2);

    // Reloaded while the Worker's copy doesn't have it yet: still there.
    await page.reload();
    await expect(outgoing(page)).toContainText("gm from Allthing");
    // Once the profile lists it, it shows once, in the list.
    fresh = true;
    await page.reload();
    await expect(page.locator(`.cast-card[data-hash="${hash}"]`)).toHaveCount(1);
    await expect(outgoing(page)).toHaveCount(0);
    await context?.close();
  });
}

test("a reply shows under its post and on the reader's Replies tab; its post's page is read again", async ({ page }) => {
  let fresh = false;
  const calls = await ready(page, `/p/${post.hash}`, () => fresh);
  await page.getByRole("button", { name: "Reply to @alice" }).click();
  const dialog = page.getByRole("dialog", { name: "Reply" });
  await dialog.getByRole("textbox").fill("Read Hyperion");
  await dialog.getByRole("button", { name: "Reply", exact: true }).click();
  await expect(dialog).toBeHidden();
  const hash = calls.sent[0].hash;
  await expect(page.locator(".replies").locator(".cast-card.outgoing")).toContainText("Read Hyperion");
  // The Worker refreshes the reader's profile and the post, with its
  // replies, and the browser reads them again.
  await expect.poll(() => calls.refreshed, { timeout: 10_000 }).toContainEqual({ username: me.username, reply_to: post.hash, page: post.hash });
  await expect.poll(() => reloads(page), { timeout: 10_000 }).toEqual(expect.arrayContaining([`/api/v1/cast?hash=${post.hash}`, `/api/v1/replies?hash=${post.hash}`]));

  // On the reader's profile: on the Replies tab, not the Casts tab.
  await page.goto(`/u/${me.username}?tab=replies`);
  await expect(outgoing(page)).toContainText("Read Hyperion");
  await page.getByRole("link", { name: "Casts", exact: true }).click();
  await expect(outgoing(page)).toHaveCount(0);

  // Once the post's page has it, it is there once.
  fresh = true;
  await page.goto(`/p/${post.hash}`);
  await expect(page.locator(`.replies #c-${hash.slice(2, 10)}`)).toBeVisible();
  await expect(page.locator(".replies .cast-card.outgoing")).toHaveCount(0);
  await expect(page.getByText("Read Hyperion")).toHaveCount(1);
});

test("a like keeps its count after a reload, while the feed's copy doesn't have it", async ({ page }) => {
  const calls = await ready(page, "/", () => false);
  const card = page.locator(`.cast-card[data-hash="${post.hash}"]`);
  const like = card.getByRole("button", { name: "Like", exact: true });
  const count = card.locator(".stat-likes .stat-count");
  await expect(count).toHaveText("3 likes");
  await like.click();
  await expect(like).toHaveAttribute("aria-pressed", "true");
  await expect(count).toHaveText("4 likes");
  // The cast and who liked it are refreshed, and read again.
  await expect.poll(() => calls.refreshed, { timeout: 10_000 }).toContainEqual({ reacted_to: post.hash });
  await expect
    .poll(() => reloads(page), { timeout: 10_000 })
    .toEqual(expect.arrayContaining([`/api/v1/cast?hash=${post.hash}`, `/api/v1/reactions?hash=${post.hash}&type=likes&limit=10`]));

  // The feed's copy still says 3: the like, which the node has, counts.
  await page.reload();
  await expect(like).toHaveAttribute("aria-pressed", "true");
  await expect(count).toHaveText("4 likes");
  // Taken back, it's 3 again, and stays so after a reload.
  await like.click();
  await expect(like).toHaveAttribute("aria-pressed", "false");
  await expect(count).toHaveText("3 likes");
  await expect.poll(() => calls.sent.filter((sent) => sent.type === 4).length).toBe(1);
  await page.reload();
  await expect(like).toHaveAttribute("aria-pressed", "false");
  await expect(count).toHaveText("3 likes");
});

test("the real Worker refreshes only for a session, from Allthing itself", async ({ request, baseURL }) => {
  const origin = new URL(baseURL!).origin;
  const wrote = { username: me.username, reacted_to: post.hash };
  expect((await request.get("/api/v1/refresh")).status()).toBe(405);
  const foreign = await request.post("/api/v1/refresh", { data: wrote, headers: { origin: "https://evil.example" } });
  expect(foreign.status()).toBe(403);
  const signedOut = await request.post("/api/v1/refresh", { data: wrote, headers: { origin } });
  expect(signedOut.status()).toBe(401);
  const forged = await request.post("/api/v1/refresh", { data: wrote, headers: { origin, cookie: "__Host-allthing-session=1234.9999999999.00" } });
  expect(forged.status()).toBe(401);
  expect(forged.headers()["cache-control"]).toBe("no-store");
});
