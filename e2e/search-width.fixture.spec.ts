// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { NOW } from "./fixture-time";

/*
 * Search results are in the feeds' column: at 1280 the page, its people,
 * its channels and its casts are as wide as the front page's feed (770 px),
 * centered as it is, where they ran the width of the page (1,100 px or so).
 * A phone's column is unchanged.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const people = [
  { fid: 2, username: "fixer", display_name: "Fixer One", follower_count: 10 },
  { fid: 3, username: "fixture.eth", display_name: "Fixture Two", follower_count: 5 },
];
const channels = [{ id: "fixtures", name: "Fixtures", parent_url: "https://farcaster.xyz/~/channel/fixtures" }];
const casts = [
  { hash: `0x${"c".repeat(40)}`, author: { fid: 2, username: "fixer" }, text: "A fixture cast about search", ts: (NOW - 19), likes: 3, recasts: 1, replies: 0 },
  { hash: `0x${"d".repeat(40)}`, author: { fid: 3, username: "fixture.eth" }, text: "Another cast about search", ts: (NOW - 79), likes: 1, recasts: 0, replies: 0 },
];

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : casts, notes: {}, meta } });
  });
  await page.route("**/api/v1/search?*", (route) => {
    const q = new URL(route.request().url()).searchParams.get("q")!;
    return route.fulfill({ json: { query: q, users: people, channels, casts, meta } });
  });
}

/** The left and right edges of what `selector` matches first, once the page's entrances are over. */
async function edges(page: Page, selector: string) {
  await page.locator(selector).first().waitFor();
  await page.waitForFunction(() =>
    document.getAnimations().every((a) => a.playState !== "running" || a.effect?.getTiming().iterations === Infinity),
  );
  return page.locator(selector).first().evaluate((el) => {
    const box = el.getBoundingClientRect();
    return [Math.round(box.left), Math.round(box.right)];
  });
}

test("at 1280, the results are in the feeds' column, as wide as the front page's cards", async ({ page }, testInfo) => {
  test.skip(Boolean(testInfo.project.use.hasTouch), "a computer's width");
  await page.setViewportSize({ width: 1280, height: 900 });
  await serve(page);
  await page.goto("/");
  const feed = await edges(page, "#app .cast-card:not(.skeleton)");
  expect(feed[1] - feed[0]).toBeLessThanOrEqual(770);

  await page.goto("/search?q=search");
  await expect(page.locator(".search-page .page-title")).toHaveText("Results for “search”");
  const card = await edges(page, ".search-page .cast-card:not(.skeleton)");
  expect(card, "a cast is as wide as a feed's, where a feed's is").toEqual(feed);
  for (const group of [".search-page .people", ".search-page .channel-results", ".search-page .page-title"]) {
    const [left, right] = await edges(page, group);
    expect(left, `${group}: from the column's left`).toBeGreaterThanOrEqual(feed[0] - 0.5);
    expect(right, `${group}: to its right, no further`).toBeLessThanOrEqual(feed[1] + 0.5);
  }
  await page.screenshot({ path: testInfo.outputPath("search-1280.png"), fullPage: true });

  // With no query, the page says what to do, in the same column.
  await page.goto("/search");
  const [left, right] = await edges(page, ".search-page");
  expect([left, right]).toEqual(feed);
});

test("at 390, the results fill the phone's column, as before", async ({ browser }, testInfo) => {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true,
  });
  const page = await context.newPage();
  await serve(page);
  await page.goto("/search?q=search");
  const [left, right] = await edges(page, ".search-page .cast-card:not(.skeleton)");
  expect(left).toBeGreaterThanOrEqual(10);
  expect(right).toBeLessThanOrEqual(380);
  expect(right - left).toBeGreaterThan(340);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await page.screenshot({ path: testInfo.outputPath("search-390.png"), fullPage: true });
  await context.close();
});
