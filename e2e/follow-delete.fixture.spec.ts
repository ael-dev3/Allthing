// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";
import { approvedKey, link, removed, signIn, type Envelope } from "./helpers/farcaster";
import { standIn } from "./helpers/node";
import { NOW } from "./fixture-time";

/*
 * Following, unfollowing and deleting one's own casts from Allthing, with
 * the reader's key. WebCrypto and IndexedDB are real; the node is a
 * stand-in, and nothing is posted. Every message handed to the node is
 * decoded, its hash and signature checked, and read back from the
 * stand-in node's store (`helpers/node.ts`).
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = (NOW - 19);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice", follower_count: 120, following_count: 30, bio: "Reading" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const mine = { hash: hex(1), author: me, text: "A cast I regret", ts, likes: 0, replies: 1 };
const reply = { hash: hex(2), author: me, text: "And a reply to it", ts: ts - 60, likes: 0, replies: 0, parent_hash: mine.hash };
const theirs = { hash: hex(3), author: alice, text: "Not mine to delete", ts, likes: 2, replies: 0 };
const NODE = "https://haatz.quilibrium.com/v1";
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" };

type Calls = { sent: Envelope[]; readonly lookups: number };

/** `slow`: how long the node takes to answer a message, in ms. */
async function serve(page: Page, { following = false, slow = 0 }: { following?: boolean; slow?: number } = {}) {
  // The node: the reader's casts, and their follow of Alice if `following`.
  const node = await standIn(page, { fid: me.fid, slow });
  node.had.cast(me.fid, mine.hash);
  node.had.cast(me.fid, reply.hash);
  if (following) node.had.follow(me.fid, alice.fid);
  const lookup = `linkById?fid=${me.fid}&target_fid=${alice.fid}&link_type=follow`;
  const calls: Calls = {
    sent: node.sent,
    // Whether the reader follows Alice, asked of the node.
    get lookups() {
      return node.reads.filter((read) => read === lookup).length;
    },
  };
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => route.fulfill({ json: { pool: [theirs, mine], notes: {}, meta } }));
  await page.route("**/api/v1/user?*", (route) => {
    const name = new URL(route.request().url()).searchParams.get("name");
    const user = name === me.username ? { ...me, follower_count: 5, following_count: 7 } : alice;
    return route.fulfill({ json: { user, pool: [name === me.username ? mine : theirs], notes: {}, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash");
    const thread = hash === theirs.hash ? { cast: theirs, children: [] } : { cast: mine, children: [{ cast: reply, children: [] }] };
    return route.fulfill({ json: { thread, notes: {}, meta } });
  });
  await page.route("**/api/v1/signer", (route) => route.fulfill({ json: { app_fid: 9152 } }));
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  return calls;
}

/** Signed in, with a key approved an hour ago, on `path`. */
async function ready(page: Page, path: string, options = {}) {
  await signIn(page, me);
  const calls = await serve(page, options);
  await page.goto(path);
  await expect(page.locator("main")).toBeVisible();
  // Someone's profile asks the node whether the reader follows them: once
  // it has, so a reload doesn't cut the lookup short.
  if (path.startsWith("/u/")) await expect.poll(() => calls.lookups).toBe(1);
  await approvedKey(page, me.fid, 3_600_000);
  await page.reload();
  return calls;
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  return { context, page: await context.newPage() };
}

const followers = (page: Page) => page.locator(".profile .counts .count").first();

test("follow and unfollow from a profile, its count following along", async ({ page }) => {
  const calls = await ready(page, "/u/alice");
  const follow = page.getByRole("button", { name: "Follow @alice" });
  await expect(follow.locator(".follow-text")).toHaveText("Follow");
  await expect(follow).toHaveAttribute("aria-pressed", "false");
  // One lookup per view of the profile: before the key was kept, and after.
  // The button shows as Follow while its lookup is on its way.
  await expect.poll(() => calls.lookups).toBe(2);
  await expect(followers(page)).toHaveText("120 followers");
  await follow.click();
  await expect(follow).toHaveAttribute("aria-pressed", "true");
  await expect(follow.locator(".follow-text")).toHaveText("Following");
  await expect(followers(page)).toHaveText("121 followers");
  await expect.poll(() => calls.sent.length).toBe(1);
  expect([calls.sent[0].type, calls.sent[0].fid]).toEqual([5, me.fid]);
  expect(link(calls.sent[0])).toEqual({ type: "follow", target: alice.fid });
  // Once the node shows it, hovering Following offers Unfollow.
  await expect(follow).not.toHaveAttribute("aria-busy");
  await follow.hover();
  await expect(follow.locator(".unfollow-text")).toBeVisible();
  await follow.click();
  await expect(follow).toHaveAttribute("aria-pressed", "false");
  await expect(followers(page)).toHaveText("120 followers");
  await expect.poll(() => calls.sent.length).toBe(2);
  expect([calls.sent[1].type, link(calls.sent[1]).target]).toEqual([6, alice.fid]);
  expect(calls.sent[1].at).toBeGreaterThan(calls.sent[0].at);
});

test("someone already followed shows Following; one's own profile has no button", async ({ page }) => {
  await ready(page, "/u/alice", { following: true });
  const follow = page.getByRole("button", { name: "Follow @alice" });
  await expect(follow).toHaveAttribute("aria-pressed", "true");
  await expect(follow.locator(".follow-text")).toHaveText("Following");
  await page.goto("/u/reader");
  await expect(page.locator(".profile-name")).toHaveText("Reader");
  await expect(page.getByRole("button", { name: /^Follow @/ })).toHaveCount(0);
});

test("delete one's own cast, after a second ask; others' casts can't be", async ({ page }) => {
  const calls = await ready(page, `/p/${mine.hash}`, { slow: 500 });
  const post = page.locator(".post");
  const remove = post.getByRole("button", { name: "Delete" });
  // Keep takes it back, with nothing sent.
  await remove.click();
  const ask = post.getByRole("group", { name: "Delete this cast?" });
  await expect(ask.getByRole("button", { name: "Keep" })).toBeFocused();
  await ask.getByRole("button", { name: "Keep" }).click();
  await expect(ask).toHaveCount(0);
  expect(calls.sent).toHaveLength(0);
  // Delete: the cast's hash, taken back; the page says so. While the node
  // answers, there is no second Delete to press.
  await remove.click();
  await ask.getByRole("button", { name: "Delete" }).click();
  await expect(post.getByRole("button", { name: "Deleting…" })).toBeDisabled();
  await expect(post.locator(".deleted-note")).toBeVisible();
  await expect(post.locator(".post-text")).toBeHidden();
  await expect(page.getByRole("status").filter({ hasText: "Deleted." })).toBeVisible();
  expect(calls.sent).toHaveLength(1);
  expect([calls.sent[0].type, removed(calls.sent[0])]).toEqual([2, mine.hash]);
  // The reply below, also the reader's, has its own Delete; still there.
  await expect(page.locator(".comment").getByRole("button", { name: "Delete" })).toBeVisible();
  // Someone else's post has none.
  await page.goto(`/p/${theirs.hash}`);
  await expect(page.locator(".post")).toContainText("Not mine to delete");
  await expect(page.getByRole("button", { name: "Delete" })).toHaveCount(0);
  // In the feed, even after a reload, it shows as deleted for this tab,
  // while cached pages may still carry it.
  await page.goto("/");
  await expect(page.locator(`.cast-card[data-hash="${mine.hash}"] .deleted-note`)).toBeVisible();
  await expect(page.locator(`.cast-card[data-hash="${mine.hash}"] .cast-text`)).toBeHidden();
  await expect(page.locator(`.cast-card[data-hash="${theirs.hash}"] .deleted-note`)).toBeHidden();
  await expect(page.locator(`.cast-card[data-hash="${theirs.hash}"] .cast-text`)).toBeVisible();
});

test("at 390, Follow and Delete fit and work by tap", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  const calls = await ready(page, "/u/alice");
  const follow = page.getByRole("button", { name: "Follow @alice" });
  await expect(follow).toBeInViewport({ ratio: 1 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await follow.tap();
  await expect(follow).toHaveAttribute("aria-pressed", "true");
  await page.goto(`/p/${mine.hash}`);
  await page.locator(".post").getByRole("button", { name: "Delete" }).tap();
  const ask = page.locator(".post").getByRole("group", { name: "Delete this cast?" });
  await expect(ask).toBeInViewport({ ratio: 1 });
  await ask.getByRole("button", { name: "Delete" }).tap();
  await expect(page.locator(".post .deleted-note")).toBeVisible();
  expect(calls.sent.map((e) => e.type)).toEqual([5, 2]);
  // A reply's question names it as one, and fits beside its buttons.
  await page.locator(".comment").getByRole("button", { name: "Delete" }).tap();
  const askReply = page.locator(".comment").getByRole("group", { name: "Delete this reply?" });
  await expect(askReply).toBeInViewport({ ratio: 1 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await askReply.getByRole("button", { name: "Keep" }).tap();
  await expect(askReply).toHaveCount(0);
  expect(calls.sent).toHaveLength(2);
  await context.close();
});
