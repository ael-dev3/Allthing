// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type Route, type TestInfo } from "@playwright/test";
import { approvedKey, cast as castOf, envelope, signIn, type Envelope } from "./helpers/farcaster";

/*
 * Emoji reactions. Farcaster's own reactions are only likes and recasts, so
 * an emoji reaction is a reply that is one Pepe and nothing else: React,
 * after Reply and Quote, opens the Pepe picker, and a pick goes out as that
 * reply with the reader's key. A cast's replies that are one Pepe show
 * under it as chips. The node is a stand-in, and every message handed to it
 * is decoded and its signature checked.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const user = (fid: number, username: string) => ({ fid, username, display_name: username[0].toUpperCase() + username.slice(1) });
const alice = user(42, "alice");
const bob = user(7, "bob");
const carol = user(8, "carol");
const dave = user(9, "dave");
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const NODE = "https://haatz.quilibrium.com/v1";
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" };

const post = { hash: hex(1), author: alice, text: "Pick a Pepe", ts, likes: 3, replies: 4 };
const reply = (n: number, author: object, text: string, at: number, parent = post.hash) =>
  ({ cast: { hash: hex(n), author, text, ts: ts + at, likes: 0, replies: 0, parent_hash: parent }, children: [] as object[] });
// Two hearts and a NO sign under the post, words that aren't a reaction,
// and a dancing Pepe under those words.
const words = reply(5, dave, "Words, not a reaction :pepe_no:", 40);
words.children = [reply(6, alice, ":pepe_pls:", 50, hex(5))];
const thread = { cast: post, children: [reply(2, bob, ":pepe_heart_hug:", 10), reply(3, carol, ":pepe_heart_hug:", 20), reply(4, bob, ":pepe_no:", 30), words] };

async function serve(page: Page) {
  const sent: Envelope[] = [];
  const preflight = (route: Route) => route.request().method() === "OPTIONS" && route.fulfill({ status: 204, headers: cors });
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: { thread, notes: {}, meta } }));
  await page.route("**/api/v1/signer", (route) => route.fulfill({ json: { app_fid: 9152 } }));
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  await page.route(`${NODE}/reactionById?*`, (route) => route.fulfill({ status: 400, headers: cors, json: { errCode: "not_found" } }));
  await page.route(`${NODE}/submitMessage`, async (route) => {
    if (await preflight(route)) return;
    sent.push(envelope(route.request().postDataBuffer()!));
    return route.fulfill({ headers: cors, json: {} });
  });
  return sent;
}

/** On the post, signed in, with a key approved an hour ago. */
async function ready(page: Page) {
  await signIn(page, me);
  const sent = await serve(page);
  await page.goto(`/p/${post.hash}`);
  await expect(page.locator(".post")).toBeVisible();
  await approvedKey(page, me.fid, 3_600_000);
  await page.reload();
  await expect(page.locator(".post")).toBeVisible();
  return sent;
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  return { context, page: await context.newPage() };
}

const chips = (page: Page) => page.locator(".post > .reaction-chips .reaction-chip");

test("a post's emoji replies gather as chips, each account once, most first", async ({ page }) => {
  await serve(page);
  await page.goto(`/p/${post.hash}`);
  await expect(chips(page)).toHaveCount(2);
  await expect(chips(page).first()).toHaveAccessibleName("Pepe hugs a big heart: 2 reactions, by @bob, @carol. React with it too");
  await expect(chips(page).first().locator(".n")).toHaveText("2");
  await expect(chips(page).nth(1)).toHaveAccessibleName("Pepe holds up a NO sign: 1 reaction, by @bob. React with it too");
  await expect(chips(page).first().locator("img")).toHaveAttribute("src", /\/assets\/emoji\/v1\/pepe_heart_hug\.webp$/);
  // A reply's own: the dancing Pepe under the words.
  const words = page.locator(".comment", { hasText: "Words, not a reaction" }).first();
  await expect(words.locator(":scope > .comment-body .reaction-chip").first()).toHaveAccessibleName(/^Pepe dances: 1 reaction, by @alice/);
  // The replies stay in the thread too, as replies.
  await expect(page.locator(".comment")).toHaveCount(5);
  // Signed out: React is there, and a chip hands its Pepe to the Farcaster
  // app, as a reply ready to post, with no one asked to sign in.
  await expect(page.locator(".post").getByRole("button", { name: "React to @alice's cast with an emoji" })).toBeVisible();
  await chips(page).first().click();
  const handOff = page.getByRole("dialog", { name: "React in Farcaster" });
  await expect(handOff).toContainText("The Farcaster app opens a reply with :pepe_heart_hug:, ready to post.");
  const url = new URL((await handOff.getByRole("link", { name: /Open on farcaster\.xyz/ }).getAttribute("href"))!);
  expect(url.origin + url.pathname).toBe("https://farcaster.xyz/~/compose");
  expect(url.searchParams.get("text")).toBe(":pepe_heart_hug:");
  expect(url.searchParams.get("parentCastHash")).toBe(post.hash);
  await expect(page.getByRole("dialog", { name: "Sign in with Farcaster" })).toHaveCount(0);
});

test("React opens the Pepe picker, and a pick goes out as a reply that is the emoji", async ({ page }) => {
  const sent = await ready(page);
  const react = page.locator(".post").getByRole("button", { name: "React to @alice's cast with an emoji" });
  await react.click();
  const panel = page.getByRole("dialog", { name: "React with a Pepe" });
  const picker = panel.getByRole("region", { name: "Pepe emoji" });
  await expect(picker).toBeVisible();
  // Emoji only: a sticker isn't a reaction.
  await expect(picker.getByRole("tab")).toHaveCount(0);
  await picker.getByRole("searchbox", { name: "Search emoji" }).fill("pls");
  await picker.getByRole("button", { name: "Pepe dances", exact: true }).first().click();
  await expect(panel).toBeHidden();
  await expect.poll(() => sent.length).toBe(1);
  expect([sent[0].type, sent[0].fid]).toEqual([1, me.fid]);
  expect(castOf(sent[0])).toMatchObject({ text: ":pepe_pls:", embeds: [], mentions: [], parent: { cast: { fid: 42, hash: post.hash } } });
  // Said, with a link to the reply, and shown as the reader's own chip.
  await expect(page.getByRole("status")).toContainText("Reacted with :pepe_pls:, as a reply.");
  await expect(page.getByRole("status").getByRole("link", { name: "View" })).toHaveAttribute("href", /\/p\/0x[0-9a-f]{40}$/);
  const mine = chips(page).nth(2);
  await expect(mine).toHaveAttribute("aria-pressed", "true");
  await expect(mine).toHaveAccessibleName("Pepe dances: 1 reaction, by you");
  // It's in the picker's recently used now.
  await react.click();
  await expect(panel.locator(".emoji-recent .emoji-tile").first()).toHaveAccessibleName("Pepe dances");
  await page.keyboard.press("Escape");
  await expect(panel).toBeHidden();
});

test("a chip reacts with its emoji, once", async ({ page }) => {
  const sent = await ready(page);
  const heart = chips(page).first();
  await expect(heart).toHaveAttribute("aria-pressed", "false");
  await heart.click();
  await expect.poll(() => sent.length).toBe(1);
  expect(castOf(sent[0]).text).toBe(":pepe_heart_hug:");
  await expect(heart.locator(".n")).toHaveText("3");
  await expect(heart).toHaveAttribute("aria-pressed", "true");
  await expect(heart).toHaveAccessibleName("Pepe hugs a big heart: 3 reactions, by you, @bob, @carol");
  // Pressed: another click sends nothing.
  await heart.click();
  await page.waitForTimeout(300);
  expect(sent).toHaveLength(1);
});

test("at 390, React's picker fits the screen and a tap reacts", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await signIn(page, me);
  const sent = await serve(page);
  await page.goto(`/p/${post.hash}`);
  await expect(page.locator(".post")).toBeVisible();
  await approvedKey(page, me.fid, 3_600_000);
  await page.reload();
  const react = page.locator(".post").getByRole("button", { name: "React to @alice's cast with an emoji" });
  // React is its icon, and the post's counts and buttons keep to one line:
  // their middles line up, measured together once the page has settled.
  await expect(react).toBeVisible();
  await expect(react.locator(".react-label")).toBeHidden();
  const spread = () =>
    page.locator(".post > .cast-foot > *").evaluateAll((items) => {
      const middles = items.map((item) => item.getBoundingClientRect()).map((box) => box.top + box.height / 2);
      return Math.max(...middles) - Math.min(...middles);
    });
  await expect.poll(spread).toBeLessThan(2);
  await react.tap();
  const panel = page.getByRole("dialog", { name: "React with a Pepe" });
  await expect(panel).toBeVisible();
  const box = (await panel.boundingBox())!;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(390);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await panel.locator(".emoji-tile").first().tap();
  await expect.poll(() => sent.length).toBe(1);
  expect(castOf(sent[0]).text).toBe(":pepe_no:");
  await expect(panel).toBeHidden();
  await context.close();
});
