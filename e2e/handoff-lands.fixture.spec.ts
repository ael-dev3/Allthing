// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { signIn } from "./helpers/farcaster";

/*
 * A write handed to the Farcaster app, as on allthing.social (where the
 * Worker endorses no keys): signed in, a like or a follow shows at once,
 * pending, and as done once the reader has done it there. The browser asks
 * the node every few seconds; once it has it, the pending mark goes, the
 * dialog closes, and a notice says so. Closing the dialog without going to
 * the app takes it back. A delete shows once done. The node and
 * farcaster.xyz are stubbed; what the reader "does in the app" is a flag
 * the stubs read.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no Worker to hand off with");

const NODE = "https://haatz.quilibrium.com/v1";
const cors = { "access-control-allow-origin": "*" };
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const theirs = { hash: hex(1), author: alice, text: "Hand this off", ts, likes: 3, recasts: 1, replies: 1 };
const older = { hash: hex(3), author: alice, text: "Liked in the app before", ts: ts - 60, likes: 5, recasts: 0, replies: 0 };
const mine = { hash: hex(2), author: me, text: "Mine to delete", ts: ts + 60, likes: 0, recasts: 0, replies: 0, parent_hash: theirs.hash };
const notFound = (what: string) => ({
  status: 400,
  headers: cors,
  json: { error: `Failed to get ${what}`, error_detail: 'status: NotFound, message: "not found"' },
});

/** What the reader has done in the Farcaster app, as the node will say. */
type Done = { liked: boolean; following: boolean; deleted: boolean };

/** The pages, with the Worker endorsing no keys, and the node. */
async function serve(page: Page, done: Done) {
  const asked = { reaction: 0, link: 0, cast: 0 };
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [theirs, older], notes: {}, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash");
    const thread = hash === mine.hash ? { cast: mine, children: [] } : { cast: theirs, children: [{ cast: mine, children: [] }] };
    return route.fulfill({ json: { thread, notes: {}, meta } });
  });
  await page.route("**/api/v1/user?*", (route) =>
    route.fulfill({ json: { user: { ...alice, follower_count: 12, following_count: 3 }, pool: [theirs], notes: {}, meta } }),
  );
  await page.route("**/api/v1/signer", (route) => route.fulfill({ status: 404, json: { error: "not set up" } }));
  await page.route("**/api/v1/auth/signer", (route) => route.fulfill({ status: 404, json: { error: "not set up" } }));
  // The Worker draws the code for the reader's phone.
  await page.route("**/api/v1/auth/qr", (route) => route.continue());
  await page.route("https://farcaster.xyz/**", (route) => route.fulfill({ body: "<title>Farcaster</title>", contentType: "text/html" }));
  // The reader's latest likes: `older`, liked in the app before this visit.
  await page.route(`${NODE}/reactionsByFid?*`, (route) => {
    const kind = new URL(route.request().url()).searchParams.get("reaction_type");
    const messages = kind === "Like"
      ? [{ data: { type: "MESSAGE_TYPE_REACTION_ADD", fid: me.fid, reactionBody: { type: "REACTION_TYPE_LIKE", targetCastId: { fid: alice.fid, hash: older.hash } } } }]
      : [];
    return route.fulfill({ headers: cors, json: { messages } });
  });
  await page.route(`${NODE}/reactionById?*`, (route) => {
    asked.reaction += 1;
    const target = new URL(route.request().url()).searchParams.get("target_hash");
    const liked = target === older.hash || (target === theirs.hash && done.liked);
    return route.fulfill(liked
      ? { headers: cors, json: { data: { type: "MESSAGE_TYPE_REACTION_ADD", fid: me.fid, reactionBody: { type: "REACTION_TYPE_LIKE" } } } }
      : notFound("reaction"));
  });
  await page.route(`${NODE}/linkById?*`, (route) => {
    asked.link += 1;
    return route.fulfill(done.following
      ? { headers: cors, json: { data: { type: "MESSAGE_TYPE_LINK_ADD", fid: me.fid, linkBody: { type: "follow", targetFid: alice.fid } } } }
      : notFound("link"));
  });
  await page.route(`${NODE}/castById?*`, (route) => {
    asked.cast += 1;
    return route.fulfill(done.deleted
      ? notFound("cast")
      : { headers: cors, json: { data: { type: "MESSAGE_TYPE_CAST_ADD", fid: me.fid, castAddBody: { text: mine.text } } } });
  });
  return asked;
}

const card = (page: Page, hash: string) => page.locator(`.cast-card[data-hash="${hash}"]`);
const likes = (page: Page, hash: string) => card(page, hash).locator(".stat-likes");

test("a like done in the Farcaster app shows here: the heart fills, the count goes up, the dialog closes", async ({ page }, info) => {
  await signIn(page, me);
  const done: Done = { liked: false, following: false, deleted: false };
  const asked = await serve(page, done);
  await page.goto("/");
  // What the reader liked in the app before shows as theirs, from the node.
  await expect(likes(page, older.hash)).toHaveClass(/\bmine\b/);
  await expect(likes(page, theirs.hash)).not.toHaveClass(/\bmine\b/);
  await expect(likes(page, theirs.hash)).toContainText("3");

  // Signed in, the heart is a toggle: what the reader did, in any app.
  const like = card(page, theirs.hash).getByRole("button", { name: "Like", exact: true });
  await expect(like).toHaveAttribute("aria-pressed", "false");
  await expect(card(page, older.hash).getByRole("button", { name: "Like", exact: true })).toHaveAttribute("aria-pressed", "true");

  await like.click();
  const dialog = page.getByRole("dialog", { name: "Like in Farcaster" });
  await expect(dialog).toBeVisible();
  // At once: the heart is the reader's, one more, pending.
  await expect(likes(page, theirs.hash)).toHaveClass(/\bmine\b/);
  await expect(likes(page, theirs.hash)).toHaveClass(/\bpending\b/);
  await expect(likes(page, theirs.hash)).toContainText("4");
  // Not yet done in the app: the node is asked, and it stays pending.
  await expect.poll(() => asked.reaction, { timeout: 10_000 }).toBeGreaterThan(1);
  await expect(dialog).toBeVisible();
  await expect(likes(page, theirs.hash)).toHaveClass(/\bpending\b/);
  await expect(dialog.getByRole("img", { name: /QR code/ })).toBeVisible();
  await page.screenshot({ path: info.outputPath("handoff-lands-1280-waiting.png") });

  // The reader likes it in the app.
  done.liked = true;
  await expect(dialog).toHaveCount(0, { timeout: 10_000 });
  await expect(likes(page, theirs.hash)).not.toHaveClass(/\bpending\b/);
  await expect(likes(page, theirs.hash)).toHaveClass(/\bmine\b/);
  await expect(likes(page, theirs.hash)).toContainText("4");
  await expect(page.getByRole("status").filter({ hasText: "Liked." })).toBeVisible();
  // The pointer off the card, as the reader's would be, back from their phone.
  await page.mouse.move(640, 600);
  await expect(page.locator(".popover-panel")).toHaveCount(0);
  await page.screenshot({ path: info.outputPath("handoff-lands-1280-liked.png") });
  await expect(like).toHaveAttribute("aria-pressed", "true");
  // And then no more asking.
  const after = asked.reaction;
  await page.waitForTimeout(4_000);
  expect(asked.reaction).toBe(after);

  // Taking it back goes the same way: at once, then done.
  await like.click();
  const unlike = page.getByRole("dialog", { name: "Unlike in Farcaster" });
  await expect(unlike).toBeVisible();
  await expect(like).toHaveAttribute("aria-pressed", "false");
  await expect(likes(page, theirs.hash)).toContainText("3");
  await expect(likes(page, theirs.hash)).toHaveClass(/\bpending\b/);
  done.liked = false;
  await expect(unlike).toHaveCount(0, { timeout: 10_000 });
  await expect(likes(page, theirs.hash)).not.toHaveClass(/\bmine\b/);
  await expect(likes(page, theirs.hash)).not.toHaveClass(/\bpending\b/);
  await expect(likes(page, theirs.hash)).toContainText("3");
  await expect(page.getByRole("status").filter({ hasText: "Unliked." })).toBeVisible();
});

test("closing the dialog without going to the app takes the like back; done there after all, it shows", async ({ page }) => {
  await signIn(page, me);
  const done: Done = { liked: false, following: false, deleted: false };
  await serve(page, done);
  await page.goto("/");
  const like = card(page, theirs.hash).getByRole("button", { name: "Like", exact: true });
  await like.click();
  await expect(likes(page, theirs.hash)).toHaveClass(/\bpending\b/);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog", { name: "Like in Farcaster" })).toHaveCount(0);
  await expect(likes(page, theirs.hash)).not.toHaveClass(/\bmine\b/);
  await expect(likes(page, theirs.hash)).not.toHaveClass(/\bpending\b/);
  await expect(likes(page, theirs.hash)).toContainText("3");
  // Scanned the code and liked it on the phone, then closed the dialog.
  done.liked = true;
  await expect(likes(page, theirs.hash)).toHaveClass(/\bmine\b/, { timeout: 10_000 });
  await expect(likes(page, theirs.hash)).toContainText("4");
  await expect(page.getByRole("status").filter({ hasText: "Liked." })).toBeVisible();
});

test("a like from before, older than the reader's latest, is found at once: the heart fills and the count stays", async ({ page }) => {
  await signIn(page, me);
  // Liked long ago: the node has it, but not among the latest likes.
  const done: Done = { liked: true, following: false, deleted: false };
  const asked = await serve(page, done);
  await page.goto("/");
  const like = card(page, theirs.hash).getByRole("button", { name: "Like", exact: true });
  await expect(like).toHaveAttribute("aria-pressed", "false");
  await like.click();
  // Asked before the reader can have done anything: nothing to do.
  await expect(page.getByRole("dialog", { name: "Like in Farcaster" })).toHaveCount(0, { timeout: 2_000 });
  await expect(like).toHaveAttribute("aria-pressed", "true");
  await expect(likes(page, theirs.hash)).not.toHaveClass(/\bpending\b/);
  await expect(likes(page, theirs.hash)).toContainText("3");
  await expect(page.getByRole("status").filter({ hasText: "Already done." })).toBeVisible();
  const after = asked.reaction;
  await page.waitForTimeout(4_000);
  expect(asked.reaction).toBe(after);
});

test("a follow and a delete done in the Farcaster app show here", async ({ page }) => {
  await signIn(page, me);
  const done: Done = { liked: false, following: false, deleted: false };
  await serve(page, done);
  await page.goto("/u/alice");
  const follow = page.getByRole("button", { name: "Follow @alice" });
  await expect(follow).toHaveAttribute("aria-pressed", "false");
  await follow.click();
  const dialog = page.getByRole("dialog", { name: "Follow @alice in Farcaster" });
  await expect(dialog).toBeVisible();
  // At once, pending.
  await expect(follow).toHaveAttribute("aria-pressed", "true");
  await expect(follow).toHaveClass(/\bpending\b/);
  await expect(page.getByText("13 followers")).toBeVisible();
  done.following = true;
  await expect(follow).not.toHaveClass(/\bpending\b/, { timeout: 10_000 });
  await expect(follow).toContainText("Following");
  await expect(dialog).toHaveCount(0);
  await expect(page.getByText("13 followers")).toBeVisible();
  await expect(page.getByRole("status").filter({ hasText: "Following." })).toBeVisible();

  await page.goto(`/p/${mine.hash}`);
  await page.locator(".post").getByRole("button", { name: "Delete" }).click();
  await expect(page.getByRole("dialog", { name: "Delete in Farcaster" })).toBeVisible();
  // A delete shows once it's done, not before.
  await page.waitForTimeout(1_000);
  await expect(page.locator(".post")).not.toHaveClass(/\bdeleted\b/);
  done.deleted = true;
  await expect(page.locator(".post")).toHaveClass(/\bdeleted\b/, { timeout: 10_000 });
  await expect(page.locator(".post .deleted-note")).toBeVisible();
  await expect(page.getByRole("status").filter({ hasText: "Deleted." })).toBeVisible();
});

test("on a phone, a like done in the app shows on coming back", async ({ browser }, info) => {
  const context = await browser.newContext({ baseURL: info.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  const page = await context.newPage();
  await signIn(page, me);
  const done: Done = { liked: false, following: false, deleted: false };
  await serve(page, done);
  await page.goto("/");
  await card(page, theirs.hash).getByRole("button", { name: "Like", exact: true }).tap();
  const dialog = page.getByRole("dialog", { name: "Like in Farcaster" });
  // To the app (a new tab here), where the reader likes it.
  const [app] = await Promise.all([context.waitForEvent("page"), dialog.getByRole("link", { name: /Open in Farcaster/ }).tap()]);
  done.liked = true;
  await app.close();
  await page.bringToFront();
  await expect(likes(page, theirs.hash)).toHaveClass(/\bmine\b/, { timeout: 10_000 });
  await expect(likes(page, theirs.hash)).toContainText("4");
  await expect(page.getByRole("status").filter({ hasText: "Liked." })).toBeVisible();
  await page.screenshot({ path: info.outputPath("handoff-lands-390-liked.png") });
  await context.close();
});

test("signed out, a hand-off asks the node nothing", async ({ page }) => {
  const done: Done = { liked: false, following: false, deleted: false };
  const asked = await serve(page, done);
  await page.goto("/");
  await card(page, theirs.hash).getByRole("button", { name: "Like", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Like in Farcaster" })).toBeVisible();
  done.liked = true;
  await page.waitForTimeout(4_000);
  expect(asked.reaction).toBe(0);
  await expect(likes(page, theirs.hash)).not.toHaveClass(/\bmine\b/);
});
