// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type Route, type TestInfo } from "@playwright/test";
import { expectDraft } from "./helpers/composer";
import { approvedKey, cast as castOf, signIn, type Envelope } from "./helpers/farcaster";
import { standIn, type Answer } from "./helpers/node";

/*
 * Casting, replying and quoting from Allthing with the reader's key. The
 * browser's WebCrypto and IndexedDB are real; KLIPY, the Farcaster app's API
 * and the node are stand-ins, and nothing is posted. Every message handed
 * to the node is decoded, its hash and signature checked, and read back
 * from the stand-in node's store (`helpers/node.ts`).
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const bob = { fid: 7, username: "bob", display_name: "Bob" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const post = { hash: hex(1), author: alice, text: "What should I read next?", ts, likes: 3, replies: 1 };
const reply = { hash: hex(2), author: bob, text: "Hyperion", ts: ts - 60, likes: 0, replies: 0, parent_hash: post.hash };
const CHANNEL_URL = "https://farcaster.xyz/~/channel/books";
const NODE = "https://haatz.quilibrium.com/v1";
const KLIPY = "https://api.klipy.com/api/v1/fixture-key/gifs";
const FARCASTER = "https://api.farcaster.xyz/v2";
const TOKEN = "0xfc0185a4609110983be9ace986f913c28e50092821297f9d";
const LINK = `farcaster://signed-key-request?token=${TOKEN}`;
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" };
const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64");

/** A KLIPY result, in its documented shape. */
const gif = (slug: string) => {
  const base = `https://static.klipy.com/ii/fixture/${slug}`;
  const format = (size: string, w: number) => ({
    gif: { url: `${base}/${size}.gif`, width: w, height: w, size: 1000 },
    webp: { url: `${base}/${size}.webp`, width: w, height: w, size: 500 },
  });
  return { id: slug.length, slug, title: slug.replace("-", " "), type: "gif", tags: [], file: { hd: format("hd", 498), md: format("md", 320), sm: format("sm", 200), xs: format("xs", 90) } };
};

type Calls = { sent: Envelope[]; names: string[]; endorse: number };

async function serve(page: Page, { submit }: { submit?: (n: number) => Answer } = {}) {
  let key = "";
  // The node: it checks each message, and answers its read-backs.
  const node = await standIn(page, { fid: me.fid, key: () => key, answer: submit });
  const calls: Calls = { sent: node.sent, names: [], endorse: 0 };
  let polls = 0;
  const preflight = (route: Route) => route.request().method() === "OPTIONS" && route.fulfill({ status: 204, headers: cors });
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [post], notes: {}, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) =>
    route.fulfill({ json: { thread: { cast: post, children: [{ cast: reply, children: [] }] }, notes: {}, meta } }),
  );
  await page.route("**/api/v1/channel?*", (route) =>
    route.fulfill({ json: { channel: { id: "books", name: "Books", parent_url: CHANNEL_URL, follower_count: 10 }, pool: [post], notes: {}, meta } }),
  );
  await page.route("**/api/v1/signer", (route) => route.fulfill({ json: { app_fid: 9152 } }));
  await page.route("**/api/v1/gifs/key", (route) => route.fulfill({ json: { key: "fixture-key" } }));
  await page.route(`${KLIPY}/**`, (route) =>
    route.fulfill({
      headers: { "access-control-allow-origin": "*" },
      json: { result: true, data: { data: [gif("trending-1"), gif("trending-2")], current_page: 1, per_page: 24, has_next: false } },
    }),
  );
  await page.route("https://static.klipy.com/**", (route) => route.fulfill({ body: png, contentType: "image/png" }));
  await page.route("https://farcaster.xyz/**", (route) => route.fulfill({ body: "<title>Farcaster</title>", contentType: "text/html" }));
  // Approving a key, for the reader who has none yet.
  await page.route("**/api/v1/auth/signer", (route) => {
    key = route.request().postDataJSON().key;
    calls.endorse += 1;
    return route.fulfill({ json: { key, requestFid: 9152, signature: `0x${"5a".repeat(65)}`, deadline: Math.floor(Date.now() / 1000) + 3600 } });
  });
  await page.route("**/api/v1/auth/qr", (route) => route.continue());
  await page.route(`${FARCASTER}/signed-key-requests`, async (route) => {
    if (await preflight(route)) return;
    return route.fulfill({ headers: cors, json: { result: { signedKeyRequest: { token: TOKEN, deeplinkUrl: LINK, key, state: "pending" } } } });
  });
  await page.route(`${FARCASTER}/signed-key-request?*`, (route) => {
    polls += 1;
    const state = polls < 2 ? { state: "pending" } : { state: "completed", userFid: me.fid };
    return route.fulfill({ headers: cors, json: { result: { signedKeyRequest: { token: TOKEN, deeplinkUrl: LINK, key, ...state } } } });
  });
  await page.route(`${NODE}/signersByFid?*`, (route) => route.fulfill({ headers: cors, json: { signers: [{ key }] } }));
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  await page.route(`${NODE}/userNameProofByName?*`, (route) => {
    const name = new URL(route.request().url()).searchParams.get("name")!;
    calls.names.push(name);
    return name === "alice"
      ? route.fulfill({ headers: cors, json: { name, fid: alice.fid, type: "USERNAME_TYPE_FNAME" } })
      : route.fulfill({ status: 400, headers: cors, json: { error: "Failed to get username proof", error_detail: 'status: NotFound, message: "Username proof not found"' } });
  });
  return calls;
}

/** Signed in, with a key approved an hour ago. */
async function ready(page: Page, path = "/") {
  await signIn(page, me);
  const calls = await serve(page);
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

test("with a key, a cast with a mention and a GIF goes out from the composer", async ({ page }) => {
  const calls = await ready(page);
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await dialog.getByRole("textbox", { name: "What's happening?" }).fill("gm @alice and @nobody");
  await dialog.getByRole("button", { name: "Add a GIF" }).click();
  await dialog.getByRole("button", { name: "trending 1" }).click();
  // It posts from here, with no way round to the Farcaster app and no note.
  await expect(dialog.getByRole("button", { name: "Cast", exact: true })).toBeEnabled();
  await expect(dialog.getByRole("link", { name: /Farcaster/ })).toHaveCount(0);
  await expect(dialog.locator(".compose-note")).toHaveCount(0);
  await expect(dialog).not.toContainText(/Farcaster app|doesn't post/);
  await dialog.getByRole("button", { name: "Cast", exact: true }).click();
  await expect(dialog).toBeHidden();
  // @alice is someone's, @nobody no one's: a mention, and text.
  expect(calls.names).toEqual(["alice", "nobody"]);
  expect(calls.sent).toHaveLength(1);
  const [sent] = calls.sent;
  expect([sent.type, sent.fid, sent.network]).toEqual([1, me.fid, 1]);
  expect(castOf(sent)).toEqual({
    text: "gm  and @nobody",
    mentions: [alice.fid],
    positions: [3],
    embeds: [{ url: "https://static.klipy.com/ii/fixture/trending-1/md.gif" }],
    parent: null,
    type: 0,
  });
  // Posted: the notice links to it, and the next draft starts empty.
  const posted = page.getByRole("status").filter({ hasText: "Posted." });
  await expect(posted.getByRole("link", { name: "View" })).toHaveAttribute("href", `/p/${sent.hash}`);
  await page.getByRole("button", { name: "Write a cast" }).click();
  await expectDraft(dialog.getByRole("textbox"), "");
});

test("a reply, a quote, and a cast in a channel go where they belong", async ({ page }) => {
  const calls = await ready(page, `/p/${post.hash}`);
  await page.getByRole("button", { name: "Reply to @alice" }).click();
  const replying = page.getByRole("dialog", { name: "Reply" });
  await replying.getByRole("textbox").fill("Read Hyperion");
  await replying.getByRole("button", { name: "Reply", exact: true }).click();
  await expect(replying).toBeHidden();
  expect(castOf(calls.sent[0])).toMatchObject({ text: "Read Hyperion", parent: { cast: { fid: alice.fid, hash: post.hash } }, embeds: [] });

  // A quote needs no words of its own.
  await page.getByRole("button", { name: "Quote @bob's cast" }).click();
  const quoting = page.getByRole("dialog", { name: "Quote" });
  await expect(quoting).toContainText("Quoting @bob");
  await quoting.getByRole("button", { name: "Quote", exact: true }).click();
  await expect(quoting).toBeHidden();
  expect(castOf(calls.sent[1])).toEqual({ text: "", mentions: [], positions: [], embeds: [{ cast: { fid: bob.fid, hash: reply.hash } }], parent: null, type: 0 });

  // On a channel page, a cast carries the channel's parent URL.
  await page.goto("/c/books");
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  await page.getByRole("button", { name: "Write a cast" }).click();
  const casting = page.getByRole("dialog", { name: "New cast" });
  await expect(casting).toContainText("In /books");
  await casting.getByRole("textbox").fill("Shrike season");
  await casting.getByRole("button", { name: "Cast", exact: true }).click();
  await expect(casting).toBeHidden();
  expect(castOf(calls.sent[2])).toMatchObject({ text: "Shrike season", parent: { url: CHANNEL_URL } });
});

test("Ctrl or Cmd and Enter posts a reply, which shows in the thread at once; in the GIF search, Enter only searches", async ({ page }) => {
  const calls = await ready(page, `/p/${post.hash}`);
  // The node holds the reply until let go, so how it shows meanwhile can be seen.
  let release = () => {};
  const held = new Promise<void>((resolve) => (release = resolve));
  await page.route(`${NODE}/submitMessage`, async (route) => {
    if (route.request().method() === "POST") await held;
    return route.fallback();
  });
  await page.getByRole("button", { name: "Reply to @alice" }).click();
  const dialog = page.getByRole("dialog", { name: "Reply" });
  const box = dialog.getByRole("textbox", { name: "Cast your reply" });
  await box.fill("Read Hyperion");
  await dialog.getByRole("button", { name: "Add a GIF" }).click();
  const search = dialog.getByRole("searchbox", { name: "Search GIFs" });
  await search.fill("cats");
  await search.press("Control+Enter");
  await expect(dialog.locator(".gif-heading")).toHaveText("GIFs for “cats”");
  expect(calls.sent).toHaveLength(0);
  await dialog.getByRole("button", { name: "Add a GIF" }).click();
  // No newline: the reply goes, as Reply would send it.
  // The composer closes at once, and the reply shows in the thread as posting.
  await box.press("Control+Enter");
  await expect(dialog).toBeHidden();
  const mine = page.locator(".replies .comment.mine");
  await expect(mine.locator(".comment-text")).toHaveText("Read Hyperion");
  await expect(mine.locator(".comment-head .when")).toHaveText("Posting…");
  release();
  await expect.poll(() => calls.sent.length).toBe(1);
  expect(castOf(calls.sent[0])).toMatchObject({ text: "Read Hyperion", parent: { cast: { fid: alice.fid, hash: post.hash } } });
  // An empty draft has nothing to post: Cmd and Enter does nothing.
  await page.getByRole("button", { name: "Reply to @alice" }).click();
  await box.press("Meta+Enter");
  await expect(dialog).toBeVisible();
  expect(calls.sent).toHaveLength(1);
});

test("long casts, too-long ones, and one the network refuses", async ({ page }) => {
  await signIn(page, me);
  const calls = await serve(page, { submit: (n) => (n === 2 ? { refuse: "text too long" } : "merge") });
  await page.goto("/");
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  await approvedKey(page, me.fid, 3_600_000);
  await page.reload();
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  const text = dialog.getByRole("textbox");
  const post = dialog.getByRole("button", { name: "Cast", exact: true });
  // The ring fills toward a cast's 320 bytes; over them, a long cast, it
  // fills again toward 1,024, which a screen reader hears once.
  const ring = dialog.locator(".length-ring");
  const fill = ring.locator(".length-fill");
  const heard = dialog.locator(".length [role=status]");
  await text.fill("a".repeat(160));
  await expect(fill).toHaveAttribute("stroke-dasharray", "50.0 100");
  await expect(heard).toHaveText("");
  await text.fill("a".repeat(400));
  await expect(fill).toHaveAttribute("stroke-dasharray", "39.1 100");
  await expect(heard).toHaveText("A long cast: up to 1,024 bytes.");
  // Amber in its last 75 bytes; then what's left, as a number.
  await text.fill("a".repeat(960));
  await expect(ring).toHaveClass(/alert/);
  await expect(heard).toHaveText("Under 75 bytes left.");
  await text.fill("a".repeat(1010));
  await expect(ring).toBeHidden();
  await expect(dialog.locator(".length-left")).toHaveText("14");
  await expect(heard).toHaveText("Under 25 bytes left.");
  await text.fill("a".repeat(400));
  await post.click();
  await expect(dialog).toBeHidden();
  expect(castOf(calls.sent[0]).type).toBe(1);
  // Over 1,024 bytes: below zero, in red, and no post.
  await page.getByRole("button", { name: "Write a cast" }).click();
  await text.fill("b".repeat(1100));
  await expect(dialog.locator(".length-left.over")).toHaveText("-76");
  await expect(heard).toHaveText("Too long to post.");
  await expect(post).toBeDisabled();
  // Refused: the draft stays, with why.
  await text.fill("refused, but kept");
  await post.click();
  await expect(dialog.getByRole("alert")).toHaveText("Farcaster didn't take it: text too long");
  await expectDraft(text, "refused, but kept");
  await expect(post).toBeEnabled();
  expect(calls.sent).toHaveLength(2);
});

test("the first cast asks for a key, then goes out; at 390 it all fits", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await signIn(page, me);
  const calls = await serve(page);
  await page.goto("/");
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  await page.getByRole("button", { name: "Write a cast" }).tap();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await dialog.getByRole("textbox").fill("first post from Allthing");
  const post = dialog.getByRole("button", { name: "Cast", exact: true });
  await expect(post).toBeInViewport({ ratio: 1 });
  await expect(dialog).toBeInViewport({ ratio: 1 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await post.tap();
  // No key yet: the dialog to approve one, over the composer; on the phone
  // itself, a link to the Farcaster app rather than a code to scan.
  const approve = page.getByRole("dialog", { name: "Approve Allthing" });
  await expect(approve.getByRole("link", { name: "Approve in Farcaster" })).toBeVisible();
  await expect(approve).toBeInViewport({ ratio: 1 });
  await expect(approve).toBeHidden();
  await expect(dialog).toBeHidden();
  expect(calls.endorse).toBe(1);
  expect(calls.sent).toHaveLength(1);
  expect(castOf(calls.sent[0])).toMatchObject({ text: "first post from Allthing", parent: null });
  const notice = page.getByRole("status").filter({ hasText: "Posted." });
  await expect(notice).toBeVisible();
  // The notice sits above the floating Cast button, not under it.
  const [toast, fab] = await Promise.all([notice.boundingBox(), page.getByRole("button", { name: "Write a cast" }).boundingBox()]);
  expect(toast!.y + toast!.height).toBeLessThanOrEqual(fab!.y);
  await context.close();
});
