// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Locator, type Page } from "@playwright/test";

/*
 * Item 142: a name in a cast's Liked by, Recasted by or Replies popover, on
 * its post page and on its card in a feed, opens that person's profile card,
 * complete from its first frame (the list preloads its people's facts). At
 * 1280 it opens beside the list on hover and on keyboard focus, and Escape
 * closes the card, then the list. At 390 a tap opens it, without following
 * the link, and the name tapped stays in view beside it.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const person = (fid: number) => ({ fid, username: `person${fid}`, display_name: `Person ${fid}` });
const post = { hash: hex(1), author: person(42), text: "A post with people", ts, likes: 12, recasts: 4, replies: 3 };
const reply = (n: number) => ({ hash: hex(10 + n), author: person(50 + n), text: `Reply ${n}`, ts: ts + 60 * n, parent_hash: post.hash });
const people = Array.from({ length: 10 }, (_, i) => person(100 + i));

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [post], notes: {}, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
    thread: { cast: post, children: [1, 2, 3].map((n) => ({ cast: reply(n) })) }, notes: {}, meta,
  } }));
  await page.route("**/api/v1/reactions?*", (route) => {
    const url = new URL(route.request().url());
    const kind = url.searchParams.get("type") ?? url.searchParams.get("kind") ?? "likes";
    return route.fulfill({ json: { hash: url.searchParams.get("hash"), kind, users: people, more: false, meta } });
  });
  await page.route("**/api/v1/vote?*", (route) => {
    const fid = Number(new URL(route.request().url()).searchParams.get("fid"));
    return route.fulfill({ json: { fid, onchain: "stay", app: "opted_in", meta } });
  });
  await page.route("**/api/v1/wallets?*", (route) => {
    const fid = Number(new URL(route.request().url()).searchParams.get("fid"));
    return route.fulfill({ json: {
      fid, addresses: [{ address: hex(fid), kind: "evm", primary: true }],
      chains: [
        { chain: "base", name: "Base", kind: "evm", count: 30, capped: false, percent: 75 },
        { chain: "ethereum", name: "Ethereum", kind: "evm", count: 10, capped: false, percent: 25 },
      ],
      total: 40, sources: [], fetched_at: Date.now(),
    } });
  });
}

const places = ["post", "feed"] as const;
/** Where the cast's counts are: its post page, or its card in the front page's feed. */
const pathOf = (where: (typeof places)[number]) => (where === "post" ? `/p/${post.hash}` : "/");

/** The cast's counts, once the app has drawn them where `where` says. */
async function open(page: Page, where: (typeof places)[number]) {
  await page.goto(pathOf(where));
  if (where === "post") {
    await expect(page.locator(".replies .comment")).toHaveCount(3);
    return page.locator(".post .engagement");
  }
  const card = page.locator(`#app .cast-card[data-hash="${post.hash}"]`);
  await expect(card).toBeVisible();
  return card.locator(".engagement");
}

const lists: [string, string, string][] = [
  ["12 likes: show who", "Liked by", ".who-link"],
  ["4 recasts: show who", "Recasted by", ".who-link"],
  ["3 replies: preview", "Replies", ".reply-author-link"],
];

/** Watches for the profile card's first frame, and says whether its facts
 * were in it. */
const firstFrame = (page: Page) => page.evaluate(() => new Promise<{ complete: boolean } | null>((resolve) => {
  const started = performance.now();
  const look = () => {
    const card = document.querySelector(".popover-panel.profile-card");
    if (card && getComputedStyle(card).opacity !== "0") {
      resolve({ complete: !card.querySelector(".vote-loading, .share-bar.loading, .share-list-loading") });
    } else if (performance.now() - started > 3000) resolve(null);
    else requestAnimationFrame(look);
  };
  look();
}));

/** Every animation that ends has ended: the card has finished coming in. */
const settled = (page: Page) =>
  page.evaluate(() =>
    Promise.all(
      document
        .getAnimations()
        .filter((a) => a.effect?.getComputedTiming().iterations !== Infinity)
        // One cut short (a list's pop-in, as a card opens beside it) has settled too.
        .map((a) => a.finished.catch(() => undefined)),
    ),
  );

const box = async (l: Locator) => (await l.boundingBox())!;
const overlap = (a: { x: number; y: number; width: number; height: number }, b: typeof a) =>
  Math.max(0, Math.min(a.x + a.width, b.x + b.width) - Math.max(a.x, b.x)) * Math.max(0, Math.min(a.y + a.height, b.y + b.height) - Math.max(a.y, b.y));

for (const where of places) {
  test(`at 1280, a name in each list opens its complete card beside the list, on hover and on keyboard focus, on the ${where}`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width: 1280, height: 900 });
    await serve(page);
    const foot = await open(page, where);
    const card = page.locator(".popover-panel.profile-card");
    for (const [trigger, name, row] of lists) {
      await test.step(`${name}, hovered`, async () => {
        await foot.getByRole("button", { name: trigger, exact: true }).hover();
        const list = page.getByRole("dialog", { name, exact: true });
        const first = list.locator(row).first();
        await expect(first).toBeVisible();
        const frame = firstFrame(page);
        await first.hover();
        expect(await frame, "the card's first frame has its facts").toEqual({ complete: true });
        await expect(card.locator(".vote")).toHaveText("Voted yes");
        // Beside the list, and on the page.
        expect(overlap(await box(list), await box(card))).toBe(0);
        const at = await box(card);
        expect(at.x).toBeGreaterThanOrEqual(0);
        expect(at.x + at.width).toBeLessThanOrEqual(1280);
        await settled(page);
        await page.screenshot({ path: testInfo.outputPath(`card-from-${name.split(" ")[0].toLowerCase()}-${where}-1280.png`) });
        await page.mouse.move(2, 2);
        await expect(list).toBeHidden();
        await expect(card).toBeHidden();
      });
      await test.step(`${name}, by keyboard`, async () => {
        const count = foot.getByRole("button", { name: trigger, exact: true });
        await count.focus();
        const list = page.getByRole("dialog", { name, exact: true });
        await expect(list).toBeVisible();
        // Tab: the first name, whose card opens beside the list.
        await page.keyboard.press("Tab");
        const first = list.locator(row).first();
        await expect(first).toBeFocused();
        await expect(card.locator(".vote")).toHaveText("Voted yes");
        expect(overlap(await box(list), await box(card))).toBe(0);
        // Escape closes the card, then the list, and focus goes back the way it came.
        await page.keyboard.press("Escape");
        await expect(card).toBeHidden();
        await expect(list).toBeVisible();
        await expect(first).toBeFocused();
        await page.keyboard.press("Escape");
        await expect(list).toBeHidden();
        await expect(count).toBeFocused();
      });
    }
  });
}

for (const where of places) {
  test(`at 390, a tapped name in each list opens its complete card, keeps the page, and stays in view beside it, on the ${where}`, async ({ browser }, testInfo) => {
    const context = await browser.newContext({
      baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true,
    });
    const page = await context.newPage();
    await serve(page);
    const foot = await open(page, where);
    const card = page.locator(".popover-panel.profile-card");
    for (const [trigger, name, row] of lists) {
      await test.step(name, async () => {
        await foot.getByRole("button", { name: trigger, exact: true }).tap();
        const list = page.getByRole("dialog", { name, exact: true });
        const tapped = list.locator(row).nth(1);
        await expect(tapped).toBeVisible();
        const frame = firstFrame(page);
        await tapped.tap();
        expect(await frame, "the card's first frame has its facts").toEqual({ complete: true });
        await expect(card.locator(".vote")).toHaveText("Voted yes");
        // The tap opened the card, not the profile.
        expect(new URL(page.url()).pathname).toBe(pathOf(where));
        // On the screen, and clear of the name tapped.
        const at = await box(card);
        expect(at.x).toBeGreaterThanOrEqual(0);
        expect(at.x + at.width).toBeLessThanOrEqual(390);
        expect(overlap(await box(tapped), at)).toBe(0);
        await settled(page);
        await page.screenshot({ path: testInfo.outputPath(`card-from-${name.split(" ")[0].toLowerCase()}-${where}-390.png`) });
        await page.keyboard.press("Escape");
        await page.keyboard.press("Escape");
        await expect(list).toBeHidden();
      });
    }
    await context.close();
  });
}
