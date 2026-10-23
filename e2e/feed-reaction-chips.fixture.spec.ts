// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";
import { approvedKey, cast as castOf, removed, signIn, type Envelope } from "./helpers/farcaster";
import { standIn } from "./helpers/node";
import { NOW } from "./fixture-time";

/*
 * Emoji reactions on a listing's cards. The cron keeps the replies that are
 * one Pepe from the hub's events, and a listing carries its casts' in its
 * notes, by fid with no names; a card shows them as chips under its foot,
 * as a post shows its own. A chip reacts, or takes the reader's reaction
 * back, as on a post, and never opens the post. The node is a stand-in:
 * every message handed to it is decoded, its hash and signature checked,
 * and read back from its store (`helpers/node.ts`).
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = (NOW - 19);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const NODE = "https://haatz.quilibrium.com/v1";
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" };

const reacted = { hash: hex(1), author: alice, text: "Pick a Pepe", ts, likes: 3, replies: 6 };
const quiet = { hash: hex(2), author: alice, text: "No one reacted to this", ts: ts - 60, likes: 1, replies: 0 };
const kept = (n: number, emoji: string, fid: number, at: number) => ({ hash: hex(n), emoji, fid, ts: ts + at });
// Hearts from three accounts, one of them twice; a NO sign from the reader
// and from one other; and an emoji since gone from the pack.
const reactions = {
  [reacted.hash]: [
    kept(10, "pepe_heart_hug", 7, 10),
    kept(11, "pepe_heart_hug", 8, 20),
    kept(12, "pepe_no", me.fid, 30),
    kept(13, "pepe_no", 9, 40),
    kept(14, "pepe_heart_hug", 9, 50),
    kept(15, "pepe_heart_hug", 9, 60),
    kept(16, "pepe_gone_from_the_pack", 9, 70),
  ],
};

async function serve(page: Page): Promise<Envelope[]> {
  // The node, with the reader's NO sign from before: it checks each
  // message, and answers its read-backs.
  const node = await standIn(page, { fid: me.fid });
  node.had.cast(me.fid, hex(12));
  const { sent } = node;
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => route.fulfill({ json: { pool: [reacted, quiet], notes: { reactions }, meta } }));
  await page.route("**/api/v1/cast?*", (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash");
    return route.fulfill({ json: { thread: { cast: hash === quiet.hash ? quiet : reacted, children: [] }, notes: {}, meta } });
  });
  await page.route("**/api/v1/signer", (route) => route.fulfill({ json: { app_fid: 9152 } }));
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  return sent;
}

/** On the front page, signed in, with a key approved an hour ago. */
async function ready(page: Page) {
  await signIn(page, me);
  const sent = await serve(page);
  await page.goto("/");
  await expect(card(page, reacted.hash)).toBeVisible();
  await approvedKey(page, me.fid, 3_600_000);
  await page.reload();
  await expect(chips(page, reacted.hash)).toHaveCount(2);
  return sent;
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  return { context, page: await context.newPage() };
}

const card = (page: Page, hash: string) => page.locator(`.cast-card[data-hash="${hash}"]`);
const chips = (page: Page, hash: string) => card(page, hash).locator(":scope > .reaction-chips .reaction-chip");
const chip = (page: Page, hash: string, emoji: string) => chips(page, hash).filter({ has: page.locator(`img[src$="/${emoji}.webp"]`) });
const path = (page: Page) => new URL(page.url()).pathname;

test("a listing's cards show their casts' emoji reactions as chips, each account once, most first", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  await expect(chips(page, reacted.hash)).toHaveCount(2);
  const heart = chips(page, reacted.hash).first();
  // A listing names no one: the accounts are counted.
  await expect(heart).toHaveAccessibleName("Pepe hugs a big heart: 3 reactions, by 3 accounts. React with it too");
  await expect(heart.locator(".n")).toHaveText("3");
  await expect(heart.locator("img")).toHaveAttribute("src", /\/assets\/emoji\/v1\/pepe_heart_hug\.webp$/);
  await expect(chips(page, reacted.hash).nth(1)).toHaveAccessibleName("Pepe holds up a NO sign: 2 reactions, by 2 accounts. React with it too");
  // Under the card's foot, as under a post's.
  const [foot, row] = await Promise.all([card(page, reacted.hash).locator(".cast-foot").boundingBox(), card(page, reacted.hash).locator(".reaction-chips").boundingBox()]);
  expect(row!.y).toBeGreaterThanOrEqual(foot!.y + foot!.height);
  // A cast no one reacted to has none.
  await expect(card(page, quiet.hash)).toBeVisible();
  await expect(card(page, quiet.hash).locator(".reaction-chips")).toHaveCount(0);
  // Signed out, a chip hands its Pepe to the Farcaster app, as a reply ready
  // to post, and the post doesn't open under it.
  await heart.click();
  const handOff = page.getByRole("dialog", { name: "React in Farcaster" });
  await expect(handOff).toContainText("A reply with :pepe_heart_hug:, ready to post.");
  const url = new URL((await handOff.getByRole("link", { name: /Open on farcaster\.xyz/ }).getAttribute("href"))!);
  expect(url.searchParams.get("text")).toBe(":pepe_heart_hug:");
  expect(url.searchParams.get("parentCastHash")).toBe(reacted.hash);
  expect(path(page)).toBe("/");
});

/** Once the cards' entrances have played (they move as they come in), or
 * been cut short. */
const settled = (page: Page) =>
  page.evaluate(() =>
    Promise.all(
      document
        .getAnimations()
        .filter((a) => a.effect?.getTiming().iterations !== Infinity)
        .map((a) => a.finished.catch(() => null)),
    ),
  );

test("until the chips' code has come, an empty row holds their place, and the cards below don't move", async ({ page }) => {
  await serve(page);
  let release!: () => void;
  const held = new Promise<void>((resolve) => (release = resolve));
  await page.route("**/split_chips.wasm", async (route) => {
    await held;
    await route.continue();
  });
  await page.goto("/");
  const row = card(page, reacted.hash).locator(".reaction-chips");
  await expect(row).toHaveClass(/\bcoming\b/);
  await expect(row).toHaveAttribute("aria-hidden", "true");
  await settled(page);
  const [empty, below] = await Promise.all([row.boundingBox(), card(page, quiet.hash).boundingBox()]);
  release();
  await expect(chips(page, reacted.hash)).toHaveCount(2);
  await expect(card(page, reacted.hash).locator(".reaction-chips.coming")).toHaveCount(0);
  await settled(page);
  const [full, after] = await Promise.all([row.boundingBox(), card(page, quiet.hash).boundingBox()]);
  expect(empty!.height).toBe(28);
  expect(full!.height).toBe(empty!.height);
  expect(after!.y).toBe(below!.y);
});

test("if the chips' code can't come, the cards go without them", async ({ page }) => {
  await serve(page);
  await page.route("**/split_chips.wasm", (route) => route.abort());
  const failed = page.waitForEvent("requestfailed", (request) => request.url().endsWith("/split_chips.wasm"));
  await page.goto("/");
  await failed;
  await expect(card(page, reacted.hash).locator(".reaction-chips")).toHaveCount(0);
  await expect(card(page, reacted.hash).locator(".cast-foot")).toBeVisible();
  // No "couldn't load" on every card, and the page isn't loaded again.
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(await page.evaluate(() => performance.getEntriesByType("navigation").length)).toBe(1);
});

test("signed in, a card's chip reacts, and the reader's own takes their reaction back, on the card", async ({ page }) => {
  const sent = await ready(page);
  const heart = chip(page, reacted.hash, "pepe_heart_hug");
  const no = chip(page, reacted.hash, "pepe_no");
  // The reader's NO sign is theirs: "you", though the listing has no names.
  await expect(no).toHaveAttribute("aria-pressed", "true");
  await expect(no).toHaveAccessibleName("Pepe holds up a NO sign: 2 reactions, by you and 1 other. Take yours back");
  await expect(heart).toHaveAttribute("aria-pressed", "false");
  await heart.click();
  await expect.poll(() => sent.length).toBe(1);
  expect([sent[0].type, sent[0].fid]).toEqual([1, me.fid]);
  expect(castOf(sent[0])).toMatchObject({ text: ":pepe_heart_hug:", embeds: [], parent: { cast: { fid: alice.fid, hash: reacted.hash } } });
  await expect(heart.locator(".n")).toHaveText("4");
  await expect(heart).toHaveAttribute("aria-pressed", "true");
  await expect(heart).toHaveAccessibleName("Pepe hugs a big heart: 4 reactions, by you and 3 others. Take yours back");
  // The NO sign from before: the reply that is it is deleted.
  await no.click();
  await expect.poll(() => sent.length).toBe(2);
  expect([sent[1].type, sent[1].fid, removed(sent[1])]).toEqual([2, me.fid, hex(12)]);
  await expect(no).toHaveAttribute("aria-pressed", "false");
  await expect(no).toHaveAccessibleName("Pepe holds up a NO sign: 1 reaction, by 1 account. React with it too");
  await expect(no.locator(".n")).toHaveText("1");
  // And the heart from this visit goes back as well.
  await heart.click();
  await expect.poll(() => sent.length).toBe(3);
  expect(removed(sent[2])).toBe(sent[0].hash);
  await expect(heart).toHaveAccessibleName("Pepe hugs a big heart: 3 reactions, by 3 accounts. React with it too");
  // None of it opened the post.
  expect(path(page)).toBe("/");
});

test("a reaction from a post shows on its card back in the listing", async ({ page }) => {
  const sent = await ready(page);
  await expect(card(page, quiet.hash).locator(".reaction-chips")).toHaveCount(0);
  await card(page, quiet.hash).locator(".cast-text").click();
  await expect(page).toHaveURL(new RegExp(`/p/${quiet.hash}$`));
  await page.locator(".post").getByRole("button", { name: "React to @alice's cast with an emoji" }).click();
  const picker = page.getByRole("dialog", { name: "React with a Pepe" }).getByRole("region", { name: "Pepe emoji" });
  await picker.getByRole("searchbox", { name: "Search emoji" }).fill("pls");
  await picker.getByRole("button", { name: "Pepe dances on the spot", exact: true }).click();
  await expect.poll(() => sent.length).toBe(1);
  await page.goBack();
  await expect(page).toHaveURL(/\/$/);
  const dance = chip(page, quiet.hash, "pepe_pls");
  await expect(dance).toHaveAttribute("aria-pressed", "true");
  await expect(dance).toHaveAccessibleName("Pepe dances on the spot: 1 reaction, by you. Take yours back");
});

test("at 390, a card's chips fit it and a tap reacts there", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  const sent = await ready(page);
  const heart = chip(page, reacted.hash, "pepe_heart_hug");
  // Inside the card, and big enough to tap.
  const [box, inside] = await Promise.all([heart.boundingBox(), card(page, reacted.hash).boundingBox()]);
  expect(box!.height).toBeGreaterThanOrEqual(24);
  expect(box!.x).toBeGreaterThanOrEqual(inside!.x);
  expect(box!.x + box!.width).toBeLessThanOrEqual(inside!.x + inside!.width);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await heart.tap();
  await expect.poll(() => sent.length).toBe(1);
  expect(castOf(sent[0]).text).toBe(":pepe_heart_hug:");
  await expect(heart).toHaveAttribute("aria-pressed", "true");
  await expect(heart.locator(".n")).toHaveText("4");
  expect(path(page)).toBe("/");
  await context.close();
});
