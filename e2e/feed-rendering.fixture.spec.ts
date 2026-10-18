// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const pool = Array.from({ length: 100 }, (_, i) => ({
  hash: `0x${i.toString(16).padStart(40, "0")}`, author: { fid: 1, username: "fixture" },
  text: `Rendering fixture ${i}: a card with enough text to exercise wrapping at narrow viewport sizes.`,
  ts: 1792235681 + i, likes: 10,
  embeds: i % 4 === 0 ? [{ kind: "image", url: `https://media.example/image-${i}.svg` }] : [],
}));
const meta = { backend: "hypersnap", fetched_at: Date.now() };

test.beforeEach(async ({ page }) => {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home", (route) => route.fulfill({ json: { pool, meta } }));
  await page.route("https://media.example/image-*.svg", async (route) => {
    await new Promise((resolve) => setTimeout(resolve, 200));
    await route.fulfill({ contentType: "image/svg+xml", body: '<svg xmlns="http://www.w3.org/2000/svg" width="320" height="160"><rect width="320" height="160" fill="#ddd9f0"/></svg>' });
  });
  await page.route("**/api/v1/cast?*", (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash");
    return route.fulfill({ json: { thread: { cast: pool.find((cast) => cast.hash === hash) ?? pool[0] }, meta } });
  });
  await page.route("**/api/v1/reactions?*", (route) => route.fulfill({ json: {
    hash: pool[0].hash, kind: "likes", users: Array.from({ length: 5 }, (_, fid) => ({ fid: fid + 2, username: `reader${fid}` })),
    more: true, meta,
  } }));
});

test("keyboard focus reveals a card below the viewport", async ({ page }) => {
  await page.goto("/?sort=new");
  const hash = await page.locator(".cast-card[data-hash]").last().getAttribute("data-hash");
  const card = page.locator(`.cast-card[data-hash="${hash}"]`);
  const link = card.locator("a.when");
  await link.focus();
  await expect(link).toBeFocused();
  await expect(card).toBeInViewport();
  await expect(card.locator(".cast-title")).toBeVisible();
});

for (const width of [1280, 390]) {
  test(`an open popover remains reachable over adjacent cards at width ${width}`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    await page.goto("/?sort=new");
    await page.locator(".cast-card[data-hash]").first().getByRole("button", { name: "10 likes: show who", exact: true }).hover();
    const panel = page.getByRole("dialog", { name: "Liked by", exact: true });
    await expect(panel.getByRole("button", { name: "See everyone" })).toBeVisible();
    expect(await panel.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return document.elementFromPoint(rect.left + rect.width / 2, rect.bottom - 12)?.closest(".popover-panel") === element;
    })).toBe(true);
    const box = (await panel.boundingBox())!;
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(width);
    expect(box.y).toBeGreaterThanOrEqual(72);
    expect(box.y + box.height).toBeLessThanOrEqual(900);
    await page.screenshot({ path: testInfo.outputPath(`popover-${width}.png`) });
  });
}

test("Back holds the same card after offscreen content resumes layout", async ({ page }) => {
  await page.goto("/?sort=new");
  const cards = page.locator(".cast-card[data-hash]");
  await expect(cards.first()).toBeVisible();
  while (await cards.count() < 100) {
    const before = await cards.count();
    await page.locator(".feed-sentinel").scrollIntoViewIfNeeded();
    await expect.poll(() => cards.count()).toBeGreaterThan(before);
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  }
  const card = cards.nth(60);
  await card.scrollIntoViewIfNeeded();
  await page.waitForTimeout(400);
  const hash = (await card.getAttribute("data-hash"))!;
  const y = (await card.boundingBox())!.y;
  await card.locator("a.when").click();
  await expect(page).toHaveURL(new RegExp(`/p/${hash}$`));
  await expect(page.locator(".post")).toBeVisible();
  await page.goBack();
  const returned = page.locator(`.cast-card[data-hash="${hash}"]`);
  const distance = async () => Math.abs(((await returned.boundingBox())?.y ?? Infinity) - y);
  await expect.poll(distance).toBeLessThan(40);
  // Check again after the scroll-restoration hold has finished.
  await page.waitForTimeout(7500);
  expect(await distance()).toBeLessThan(40);
  await expect(returned.locator(".cast-title")).toBeVisible();
});
