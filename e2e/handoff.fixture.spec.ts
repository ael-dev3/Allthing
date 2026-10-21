// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";
import { expectDraft } from "./helpers/composer";
import { signIn } from "./helpers/farcaster";

/*
 * Every write handed to the Farcaster app, as on allthing.social, where the
 * Worker endorses no keys: Allthing has no Farcaster account or key of its
 * own. Like, recast, cast, reply, quote, follow, delete and a Pepe reaction
 * each open the cast, the profile or a draft ready to post in the app: on a
 * computer with a QR code for the reader's phone, drawn by the real Worker,
 * and on a phone with the link alone. farcaster.xyz is stubbed.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no Worker to hand off with");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const theirs = { hash: hex(1), author: alice, text: "Hand this off", ts, likes: 3, recasts: 1, replies: 1 };
const mine = { hash: hex(2), author: me, text: "Mine to delete", ts: ts + 60, likes: 0, recasts: 0, replies: 0, parent_hash: theirs.hash };
const castUrl = (c: { hash: string; author: { username: string } }) => `https://farcaster.xyz/${c.author.username}/${c.hash.slice(0, 10)}`;
const COMPOSE = "https://farcaster.xyz/~/compose";

/** The pages, with the Worker endorsing no keys; codes are drawn by the real Worker. */
async function serve(page: Page, { following = false } = {}) {
  const calls = { endorse: 0, qr: 0 };
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [theirs], notes: {}, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash");
    const thread = hash === mine.hash ? { cast: mine, children: [] } : { cast: theirs, children: [{ cast: mine, children: [] }] };
    return route.fulfill({ json: { thread, notes: {}, meta } });
  });
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: { user: { ...alice, follower_count: 12, following_count: 3 }, pool: [theirs], notes: {}, meta } }));
  await page.route("**/api/v1/signer", (route) => route.fulfill({ status: 404, json: { error: "Liking and recasting from Allthing isn't set up on this server" } }));
  await page.route("**/api/v1/auth/signer", (route) => {
    calls.endorse += 1;
    return route.fulfill({ status: 404, json: { error: "not set up" } });
  });
  await page.route("**/api/v1/scheduled", (route) => route.fulfill({ json: { casts: [] } }));
  await page.route("**/api/v1/auth/qr", (route) => {
    calls.qr += 1;
    return route.continue();
  });
  await page.route("**/linkById?*", (route) => route.fulfill({
    status: following ? 200 : 400,
    headers: { "access-control-allow-origin": "*" },
    json: following
      ? { data: { type: "MESSAGE_TYPE_LINK_ADD", fid: me.fid, linkBody: { type: "follow", targetFid: alice.fid } } }
      : { error: "Failed to get link", error_detail: 'status: NotFound, message: "not found"' },
  }));
  await page.route("https://farcaster.xyz/**", (route) => route.fulfill({ body: "<title>Farcaster</title>", contentType: "text/html" }));
  // Signed in, the reader's likes and recasts, and whether a hand-off has
  // landed, are read from the node: none, here.
  const node = "https://haatz.quilibrium.com/v1";
  const cors = { "access-control-allow-origin": "*" };
  const notFound = { status: 400, headers: cors, json: { error: "Failed to get it", error_detail: 'status: NotFound, message: "not found"' } };
  await page.route(`${node}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  await page.route(`${node}/reactionById?*`, (route) => route.fulfill(notFound));
  await page.route(`${node}/castById?*`, (route) => route.fulfill({ headers: cors, json: { data: { type: "MESSAGE_TYPE_CAST_ADD" } } }));
  return calls;
}

/** The hand-off dialog `title` opens, its link, and whether it shows a code. */
async function handedOff(page: Page, title: string, link: RegExp) {
  const dialog = page.getByRole("dialog", { name: title });
  await expect(dialog).toBeVisible();
  const open = dialog.getByRole("link", { name: link });
  await expect(open).toBeFocused();
  return { dialog, href: (await open.getAttribute("href"))! };
}

test("signed out, every write goes to the Farcaster app, with a code for the reader's phone", async ({ page }) => {
  const calls = await serve(page);
  await page.goto("/");
  const card = page.locator(`.cast-card[data-hash="${theirs.hash}"]`);
  // Like, then recast: the cast in the app, as a QR code and a link.
  const like = card.getByRole("button", { name: "Like", exact: true });
  await like.click();
  let { dialog, href } = await handedOff(page, "Like in Farcaster", /Open on farcaster\.xyz/);
  expect(href).toBe(castUrl(theirs));
  // The title says it all: no line under it, and few words around the code.
  await expect(dialog.locator(".handoff-line")).toHaveCount(0);
  await expect(dialog).toContainText("Scan with your phone.");
  // No note that Allthing doesn't act for the reader.
  await expect(dialog).not.toContainText("doesn't act for you");
  await page.screenshot({ path: test.info().outputPath("handoff-1280.png") });
  await expect(dialog.getByRole("img", { name: "QR code that opens it in the Farcaster app" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(like).toBeFocused();
  await card.getByRole("button", { name: "Recast", exact: true }).click();
  ({ href } = await handedOff(page, "Recast in Farcaster", /Open on farcaster\.xyz/));
  expect(href).toBe(castUrl(theirs));
  await page.keyboard.press("Escape");

  // A cast: the draft, ready to post. Done clears it and closes the composer.
  await page.getByRole("button", { name: "Write a cast" }).click();
  const composer = page.getByRole("dialog", { name: "New cast" });
  await page.keyboard.type("gm & hello");
  await expect(composer.getByRole("button", { name: "Schedule for later" })).toHaveCount(0);
  await composer.getByRole("button", { name: "Post in Farcaster" }).click();
  ({ dialog, href } = await handedOff(page, "Post in Farcaster", /Open on farcaster\.xyz/));
  expect(href).toBe(`${COMPOSE}?text=gm%20%26%20hello`);
  await expect(dialog.getByRole("img", { name: "QR code that opens it in the Farcaster app" })).toBeVisible();
  // Escape goes back to the draft; Done is done with it.
  await page.keyboard.press("Escape");
  await expectDraft(composer.getByRole("textbox"), "gm & hello");
  await composer.getByRole("button", { name: "Post in Farcaster" }).click();
  await page.getByRole("dialog", { name: "Post in Farcaster" }).getByRole("button", { name: "Done" }).click();
  await expect(composer).toHaveCount(0);
  await page.getByRole("button", { name: "Write a cast" }).click();
  await expectDraft(page.getByRole("dialog", { name: "New cast" }).getByRole("textbox"), "");
  await page.keyboard.press("Escape");

  // A reply and a quote, under the post.
  await page.goto(`/p/${theirs.hash}`);
  const post = page.locator(".post");
  await post.getByRole("button", { name: "Reply to @alice" }).click();
  await page.keyboard.type("so true");
  await page.getByRole("dialog", { name: "Reply" }).getByRole("button", { name: "Post in Farcaster" }).click();
  ({ href } = await handedOff(page, "Post in Farcaster", /Open on farcaster\.xyz/));
  let url = new URL(href);
  expect([url.searchParams.get("text"), url.searchParams.get("parentCastHash")]).toEqual(["so true", theirs.hash]);
  // Opening it hands the draft over: the composer closes, and the next one is empty.
  const opened = page.context().waitForEvent("page");
  await page.getByRole("dialog", { name: "Post in Farcaster" }).getByRole("link", { name: /Open on farcaster\.xyz/ }).click();
  const tab = await opened;
  expect(tab.url()).toBe(href);
  await tab.close();
  await expect(page.getByRole("dialog", { name: "Reply" })).toHaveCount(0);
  await post.getByRole("button", { name: "Quote @alice's cast" }).click();
  await page.getByRole("dialog", { name: "Quote" }).getByRole("button", { name: "Post in Farcaster" }).click();
  ({ href } = await handedOff(page, "Post in Farcaster", /Open on farcaster\.xyz/));
  url = new URL(href);
  expect(url.searchParams.getAll("embeds[]")).toEqual([castUrl(theirs)]);
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");

  // A Pepe: a reply that is the emoji, ready to post.
  await post.getByRole("button", { name: "React to @alice's cast with an emoji" }).click();
  await page.getByRole("dialog", { name: "React with a Pepe" }).getByRole("button", { name: /^Pepe/ }).first().click();
  ({ dialog, href } = await handedOff(page, "React in Farcaster", /Open on farcaster\.xyz/));
  url = new URL(href);
  expect(url.searchParams.get("text")).toMatch(/^:pepe_[a-z_]+:$/);
  expect(url.searchParams.get("parentCastHash")).toBe(theirs.hash);
  await page.keyboard.press("Escape");

  // Follow, on a profile: the profile in the app.
  await page.goto("/u/alice");
  const follow = page.getByRole("button", { name: "Follow @alice" });
  await expect(follow).not.toHaveAttribute("aria-pressed");
  await follow.click();
  ({ dialog, href } = await handedOff(page, "Follow @alice in Farcaster", /Open on farcaster\.xyz/));
  expect(href).toBe("https://farcaster.xyz/alice");

  // Nobody was asked to sign in, and no key was asked for.
  await expect(page.getByRole("dialog", { name: "Sign in with Farcaster" })).toHaveCount(0);
  expect(calls.endorse).toBe(0);
  expect(calls.qr).toBeGreaterThanOrEqual(5);
});

test("signed in where the Worker endorses no keys: Delete, Unfollow and nothing to schedule", async ({ page }) => {
  await signIn(page, me);
  const calls = await serve(page, { following: true });
  // One's own reply: Delete opens it in the app, with no second ask here.
  await page.goto(`/p/${mine.hash}`);
  const remove = page.locator(".post").getByRole("button", { name: "Delete" });
  // Once the Worker has said it endorses no keys.
  await expect(remove).toHaveAttribute("aria-haspopup", "dialog");
  await remove.click();
  const { dialog, href } = await handedOff(page, "Delete in Farcaster", /Open on farcaster\.xyz/);
  expect(href).toBe(castUrl(mine));
  await expect(dialog).toContainText("Delete your cast from its menu there.");
  await expect(page.getByRole("group", { name: /^Delete this/ })).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(remove).toBeFocused();
  // Someone else's has none.
  await page.goto(`/p/${theirs.hash}`);
  await expect(page.locator(".post").getByRole("button", { name: "Delete" })).toHaveCount(0);
  // Following already: Unfollow, in the app.
  await page.goto("/u/alice");
  const follow = page.getByRole("button", { name: "Follow @alice" });
  await expect(follow).toHaveAttribute("aria-pressed", "true");
  await follow.click();
  await handedOff(page, "Unfollow @alice in Farcaster", /Open on farcaster\.xyz/);
  await page.keyboard.press("Escape");
  // No clock, no Scheduled casts in the menu, and the list says why.
  await page.goto("/");
  await page.getByRole("button", { name: "Write a cast" }).click();
  await expect(page.getByRole("dialog", { name: "New cast" }).getByRole("button", { name: "Post in Farcaster" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Schedule for later" })).toHaveCount(0);
  await page.keyboard.press("Escape");
  await page.getByRole("link", { name: "Signed in as @reader: your profile" }).hover();
  const account = page.getByRole("dialog", { name: "Your account" });
  await expect(account.getByRole("button", { name: "Sign out" })).toBeVisible();
  await expect(account.getByRole("link", { name: "Scheduled casts" })).toHaveCount(0);
  await page.goto("/scheduled");
  await expect(page.locator(".scheduled-empty")).toHaveText("Nothing scheduled.");
  expect(calls.endorse).toBe(0);
});

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  return { context, page: await context.newPage() };
}

test("at 390, each write opens the Farcaster app: a link, no code, inside the screen", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  const calls = await serve(page);
  await page.goto("/");
  const card = page.locator(`.cast-card[data-hash="${theirs.hash}"]`);
  await card.getByRole("button", { name: "Like", exact: true }).tap();
  const { dialog, href } = await handedOff(page, "Like in Farcaster", /Open in Farcaster/);
  expect(href).toBe(castUrl(theirs));
  await expect(dialog).toBeInViewport({ ratio: 1 });
  await expect(dialog.getByRole("img")).toHaveCount(0);
  // Measured once the dialog has popped in (it scales up as it opens).
  await dialog.evaluate((el) => Promise.all(el.getAnimations().map((a) => a.finished)));
  const open = (await dialog.getByRole("link", { name: /Open in Farcaster/ }).boundingBox())!;
  expect(open.height).toBeGreaterThanOrEqual(44);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await expect(dialog).not.toContainText("doesn't act for you");
  await page.screenshot({ path: testInfo.outputPath("handoff-390.png") });
  await dialog.getByRole("button", { name: "Close" }).tap();
  // Cast: the round button, and the draft's link straight to the app.
  await page.getByRole("button", { name: "Write a cast" }).tap();
  await page.keyboard.type("gm from my phone");
  const post = page.getByRole("dialog", { name: "New cast" }).getByRole("link", { name: /Post in Farcaster/ });
  await expect(post).toBeInViewport();
  await expect(post).toHaveAttribute("href", `${COMPOSE}?text=gm%20from%20my%20phone`);
  await expect(page.getByRole("dialog", { name: "New cast" })).not.toContainText("doesn't post");
  await page.screenshot({ path: testInfo.outputPath("composer-390.png") });
  await page.keyboard.press("Escape");
  // Follow.
  await page.goto("/u/alice");
  await page.getByRole("button", { name: "Follow @alice" }).tap();
  const followed = await handedOff(page, "Follow @alice in Farcaster", /Open in Farcaster/);
  expect(followed.href).toBe("https://farcaster.xyz/alice");
  await expect(followed.dialog).toBeInViewport({ ratio: 1 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  // A phone draws no codes.
  expect(calls.qr).toBe(0);
  await context.close();
});
