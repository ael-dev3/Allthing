// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { appCame } from "./helpers/app";
import { NOW } from "./fixture-time";

/*
 * Who liked and recasted feed cards loads a run of six cards ahead of the
 * reader: the next run's batch comes once its first card is less than a
 * screen below the viewport, so its popovers open with their people when
 * the reader gets there. A page view loads at most eight runs, 48 cards.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const hash = (i: number) => `0x${i.toString(16).padStart(40, "0")}`;
/** `n` casts with a like and a recast each, newest first, of `paragraphs` paragraphs. */
const castsOf = (n: number, paragraphs: number) =>
  Array.from({ length: n }, (_, i) => ({
    hash: hash(i),
    author: { fid: 100 + i, username: `author${i}` },
    text: Array.from({ length: paragraphs }, (_, p) => `Card ${i}, paragraph ${p + 1}.`).join("\n\n"),
    ts: (NOW - 19) - i * 60,
    likes: 1, recasts: 1, replies: 0,
  }));
const liker = (h: string) => `Liker of ${parseInt(h, 16)}`;

/** Serve "new" with `pool`, and answer each batch for what it asks; the batches asked, as their likes' hashes. */
async function serve(page: Page, pool: object[]) {
  const batches: string[][] = [];
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/latest", (route) => route.fulfill({ json: { pool, meta } }));
  await page.route("**/api/v1/engagement?*", (route) => {
    const asked = new URL(route.request().url()).searchParams;
    const likes = asked.get("likes")?.split(",") ?? [];
    const recasts = asked.get("recasts")?.split(",") ?? [];
    batches.push(likes);
    const users = (h: string) => [{ fid: 9, username: `liker${parseInt(h, 16)}`, display_name: liker(h) }];
    return route.fulfill({ json: {
      previews: [
        ...likes.map((h) => ({ hash: h, kind: "likes", users: users(h), more: false, meta })),
        ...recasts.map((h) => ({ hash: h, kind: "recasts", users: users(h), more: false, meta })),
      ],
      meta,
    } });
  });
  return batches;
}

/** URLs of the requests whose URL contains `part`, as they are sent. */
function requests(page: Page, part: string): string[] {
  const seen: string[] = [];
  page.on("request", (request) => {
    if (request.url().includes(part)) seen.push(request.url());
  });
  return seen;
}

const card = (page: Page, i: number) => page.locator(`.cast-card[data-hash="${hash(i)}"]`);
/** Where card `i`'s top is, against the viewport's height. */
const top = (page: Page, i: number) =>
  card(page, i).evaluate((el) => ({ top: el.getBoundingClientRect().top, height: innerHeight }));
/** Scroll so that card `i`'s top is `below` px under the viewport's bottom edge. */
async function bring(page: Page, i: number, below: number) {
  await card(page, i).evaluate((el, below) => {
    window.scrollBy(0, el.getBoundingClientRect().top - innerHeight - below);
  }, below);
}

for (const phone of [false, true]) {
  test(`${phone ? "at 390" : "at 1280"}, the next run's batch comes while its first card is still below the viewport`, async ({ browser }, testInfo) => {
    const context = await browser.newContext({
      baseURL: testInfo.project.use.baseURL,
      ...(phone ? { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true } : { viewport: { width: 1280, height: 900 } }),
    });
    const page = await context.newPage();
    // Tall enough cards that the second run starts more than a screen down.
    const batches = await serve(page, castsOf(12, 6));
    const reactions = requests(page, "/api/v1/reactions");
    await page.goto("/?sort=new");
    await appCame(page);
    await expect(page.locator(".cast-card[data-hash]")).toHaveCount(12);
    await expect.poll(() => batches.length).toBe(1);
    expect(batches[0]).toEqual([0, 1, 2, 3, 4, 5].map(hash));
    const first = await top(page, 6);
    expect(first.top).toBeGreaterThan(2 * first.height);
    await page.waitForTimeout(2500);
    expect(batches).toHaveLength(1);

    // Card 6 half a screen below the viewport: its run is near, so it comes.
    await bring(page, 6, first.height / 2);
    await expect.poll(() => batches.length, { timeout: 6000 }).toBe(2);
    expect(batches[1]).toEqual([6, 7, 8, 9, 10, 11].map(hash));
    const near = await top(page, 6);
    expect(near.top).toBeGreaterThan(near.height);

    // Got to, its popover opens with its people, and asks for nothing.
    await card(page, 6).scrollIntoViewIfNeeded();
    const likes = card(page, 6).locator(".stat-likes .stat-count");
    if (phone) await likes.tap();
    else await likes.hover();
    await expect(card(page, 6).getByRole("dialog", { name: "Liked by", exact: true })).toContainText(liker(hash(6)));
    expect(reactions).toEqual([]);
    await page.screenshot({ path: testInfo.outputPath(`ahead-${phone ? 390 : 1280}.png`) });
    await context.close();
  });
}

test("a page view preloads at most eight runs, 48 cards, the first page's last card among them", async ({ page }) => {
  test.setTimeout(120_000);
  // Short cards, so a scroll to the end brings each run near in turn.
  const batches = await serve(page, castsOf(60, 1));
  await page.goto("/?sort=new");
  await appCame(page);
  await expect(page.locator(".cast-card[data-hash]")).toHaveCount(25);
  // Read down at a reader's pace, as more cards are added. (A run scrolled
  // past before its turn isn't loaded: its popovers would load on hover.)
  for (let i = 0; i < 40 && batches.length < 8; i++) {
    await page.mouse.wheel(0, 500);
    await page.waitForTimeout(2100);
  }
  await expect.poll(() => batches.length, { timeout: 30_000 }).toBe(8);
  // Batches follow the list, each run once: the first page's 25th card (24)
  // is in the fifth, with the next page's first five.
  expect(batches[4]).toEqual([24, 25, 26, 27, 28, 29].map(hash));
  expect(new Set(batches.flat()).size).toBe(48);
  // No more, however far the reader goes.
  await page.mouse.wheel(0, 4000);
  await page.waitForTimeout(3000);
  expect(batches).toHaveLength(8);
});
