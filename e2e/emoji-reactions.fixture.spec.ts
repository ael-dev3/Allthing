// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";
import { approvedKey, cast as castOf, removed, signIn, type Envelope } from "./helpers/farcaster";
import { standIn } from "./helpers/node";
import { NOW } from "./fixture-time";

/*
 * Emoji reactions. Farcaster's own reactions are only likes and recasts, so
 * an emoji reaction is a reply that is one Pepe and nothing else: React,
 * after Reply and Quote, opens the Pepe picker, and a pick goes out as that
 * reply with the reader's key. A cast's replies that are one Pepe show
 * under it as chips. The node is a stand-in: every message handed to it is
 * decoded, its hash and signature checked, and read back from its store
 * (`helpers/node.ts`).
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = (NOW - 19);
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

async function serve(page: Page): Promise<Envelope[]> {
  // The node: it checks each message, and answers its read-backs.
  const { sent } = await standIn(page, { fid: me.fid });
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: { thread, notes: {}, meta } }));
  await page.route("**/api/v1/signer", (route) => route.fulfill({ json: { app_fid: 9152 } }));
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
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

/** Holds :hover on an element, as the browser's developer tools can. */
async function holdHover(page: Page, element: Locator) {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("DOM.enable");
  await cdp.send("CSS.enable");
  await element.evaluate((el) => el.setAttribute("data-held", ""));
  const { root } = await cdp.send("DOM.getDocument");
  const { nodeId } = await cdp.send("DOM.querySelector", { nodeId: root.nodeId, selector: "[data-held]" });
  await cdp.send("CSS.forcePseudoState", { nodeId, forcedPseudoClasses: ["hover"] });
}

/** How far apart the middles of the post's counts and buttons are. */
const footSpread = (page: Page) =>
  page.locator(".post > .cast-foot > *").evaluateAll((items) => {
    const middles = items.map((item) => item.getBoundingClientRect()).map((box) => box.top + box.height / 2);
    return Math.max(...middles) - Math.min(...middles);
  });

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
  await expect(words.locator(":scope > .comment-body .reaction-chip").first()).toHaveAccessibleName(/^Pepe dances on the spot: 1 reaction, by @alice/);
  // The replies stay in the thread too, as replies.
  await expect(page.locator(".comment")).toHaveCount(5);
  // Signed out: React is there, and a chip hands its Pepe to the Farcaster
  // app, as a reply ready to post, with no one asked to sign in.
  await expect(page.locator(".post").getByRole("button", { name: "React to @alice's cast with an emoji" })).toBeVisible();
  await chips(page).first().click();
  const handOff = page.getByRole("dialog", { name: "React in Farcaster" });
  await expect(handOff).toContainText("A reply with :pepe_heart_hug:, ready to post.");
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
  await picker.getByRole("button", { name: "Pepe dances on the spot", exact: true }).click();
  await expect(panel).toBeHidden();
  await expect.poll(() => sent.length).toBe(1);
  expect([sent[0].type, sent[0].fid]).toEqual([1, me.fid]);
  expect(castOf(sent[0])).toMatchObject({ text: ":pepe_pls:", embeds: [], mentions: [], parent: { cast: { fid: 42, hash: post.hash } } });
  // Said, with a link to the reply, and shown as the reader's own chip.
  await expect(page.getByRole("status")).toContainText("Reacted with :pepe_pls:, as a reply.");
  await expect(page.getByRole("status").getByRole("link", { name: "View" })).toHaveAttribute("href", /\/p\/0x[0-9a-f]{40}$/);
  const mine = chips(page).nth(2);
  await expect(mine).toHaveAttribute("aria-pressed", "true");
  await expect(mine).toHaveAccessibleName("Pepe dances on the spot: 1 reaction, by you. Take yours back");
  // It's in the picker's recently used now.
  await react.click();
  await expect(panel.locator(".emoji-recent .emoji-tile").first()).toHaveAccessibleName("Pepe dances on the spot");
  await page.keyboard.press("Escape");
  await expect(panel).toBeHidden();
});

test("a chip reacts with its emoji; pressed again, it takes the reaction back", async ({ page }) => {
  const sent = await ready(page);
  const heart = chips(page).first();
  await expect(heart).toHaveAttribute("aria-pressed", "false");
  await heart.click();
  await expect.poll(() => sent.length).toBe(1);
  expect(castOf(sent[0]).text).toBe(":pepe_heart_hug:");
  await expect(heart.locator(".n")).toHaveText("3");
  await expect(heart).toHaveAttribute("aria-pressed", "true");
  await expect(heart).toHaveAccessibleName("Pepe hugs a big heart: 3 reactions, by you, @bob, @carol. Take yours back");
  // Pressed again: the reply that is the reaction is deleted, and the chip
  // is as it was.
  await heart.click();
  await expect.poll(() => sent.length).toBe(2);
  expect([sent[1].type, sent[1].fid, removed(sent[1])]).toEqual([2, me.fid, sent[0].hash]);
  await expect(heart.locator(".n")).toHaveText("2");
  await expect(heart).toHaveAttribute("aria-pressed", "false");
  await expect(heart).toHaveAccessibleName("Pepe hugs a big heart: 2 reactions, by @bob, @carol. React with it too");
  // And reacts again from there: a new reply, a second later at least,
  // not the one deleted, which the node wouldn't take again.
  await heart.click();
  await expect.poll(() => sent.length).toBe(3);
  expect(castOf(sent[2]).text).toBe(":pepe_heart_hug:");
  expect(sent[2].hash).not.toBe(sent[0].hash);
  expect(sent[2].at).toBeGreaterThan(sent[0].at);
  await expect(heart).toHaveAttribute("aria-pressed", "true");
});

test("the reader's reaction from before is taken back from its chip, in the Farcaster app where Allthing doesn't post", async ({ page }) => {
  // The reader's NO sign is a reply in the thread already, and Allthing
  // doesn't write for them: taking it back opens that reply in the app.
  const own = reply(7, me, ":pepe_no:", 35);
  const withMine = { ...thread, children: [...thread.children, own] };
  await signIn(page, me);
  await serve(page);
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: { thread: withMine, notes: {}, meta } }));
  await page.route("**/api/v1/signer", (route) => route.fulfill({ status: 404, json: { error: "Liking and recasting from Allthing isn't set up on this server" } }));
  // The node still has the reply: not deleted in the app yet.
  await page.route(`${NODE}/castById?*`, (route) => route.fulfill({ headers: cors, json: { data: { type: "MESSAGE_TYPE_CAST_ADD" } } }));
  await page.goto(`/p/${post.hash}`);
  const no = chips(page).filter({ has: page.locator('img[src$="pepe_no.webp"]') });
  await expect(no).toHaveAttribute("aria-pressed", "true");
  await expect(no).toHaveAccessibleName("Pepe holds up a NO sign: 2 reactions, by @bob, @reader. Take yours back");
  await no.click();
  const dialog = page.getByRole("dialog", { name: "Delete in Farcaster" });
  await expect(dialog).toContainText("Delete your reaction from its menu there.");
  await expect(dialog.getByRole("link", { name: /Open on farcaster\.xyz/ })).toHaveAttribute("href", `https://farcaster.xyz/reader/${own.cast.hash.slice(0, 10)}`);
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
  await expect.poll(() => footSpread(page)).toBeLessThan(2);
  await react.tap();
  const panel = page.getByRole("dialog", { name: "React with a Pepe" });
  await expect(panel).toBeVisible();
  const box = (await panel.boundingBox())!;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(390);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  // Open, it keeps 8 px from the screen's edge. (Placed by its box as it
  // began to open, scaled about its middle, it kept 5.)
  await panel.evaluate((el) => Promise.all(el.getAnimations().map((animation) => animation.finished)));
  const open = (await panel.boundingBox())!;
  expect(open.x + open.width).toBeLessThanOrEqual(390 - 8 + 0.5);
  expect(open.x + open.width).toBeGreaterThan(390 - 8 - 1);
  await panel.locator(".emoji-tile").first().tap();
  await expect.poll(() => sent.length).toBe(1);
  expect(castOf(sent[0]).text).toBe(":pepe_no:");
  await expect(panel).toBeHidden();
  // A tap on its chip takes it back, and the chip looks like any other
  // again: a phone keeps :hover on what was last tapped, which mustn't look
  // like a reaction of the reader's.
  const no = chips(page).filter({ has: page.locator('img[src$="pepe_no.webp"]') });
  const heart = chips(page).filter({ has: page.locator('img[src$="pepe_heart_hug.webp"]') });
  await expect(no).toHaveAttribute("aria-pressed", "true");
  await no.tap();
  await expect.poll(() => sent.length).toBe(2);
  expect(removed(sent[1])).toBe(sent[0].hash);
  await expect(no).toHaveAttribute("aria-pressed", "false");
  await expect(no).not.toHaveClass(/pending/);
  // Held hovered, as the phone holds it. (Chrome's emulated phone sometimes
  // moves the tap's :hover to the page's header a moment later, and then
  // there was nothing to compare.)
  await holdHover(page, no);
  expect(await no.evaluate((el) => el.matches(":hover"))).toBe(true);
  const look = (chip: Locator) => chip.evaluate((el) => [getComputedStyle(el).borderColor, getComputedStyle(el).color]);
  expect(await look(no)).toEqual(await look(heart));
  await context.close();
});

test("at 390, React's picker is inside the screen from the moment it opens", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await signIn(page, me);
  await serve(page);
  await page.goto(`/p/${post.hash}`);
  await expect(page.locator(".post")).toBeVisible();
  await approvedKey(page, me.fid, 3_600_000);
  await page.reload();
  const react = page.locator(".post").getByRole("button", { name: "React to @alice's cast with an emoji" });
  await expect(react).toBeVisible();
  await expect.poll(() => footSpread(page)).toBeLessThan(2);
  // Read in the very next task after the press. It's placed in the press's
  // own; placed a frame later, it hung over the edge until then, where a
  // test that measured it in time found it, and the phone's page widened
  // to its right edge.
  const opened = await react.evaluate(async (button) => {
    button.click();
    await new Promise((next) => setTimeout(next, 0));
    const box = document.querySelector(".react-panel")!.getBoundingClientRect();
    return { left: box.left, right: box.right, page: innerWidth };
  });
  expect(opened.left).toBeGreaterThanOrEqual(0);
  expect(opened.right).toBeLessThanOrEqual(390);
  expect(opened.page).toBe(390);
  await context.close();
});
