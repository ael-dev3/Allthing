// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";
import path from "node:path";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const stream = "https://media.example/master.m3u8";
const hash = `0x${"a".repeat(40)}`;
const meta = { backend: "hypersnap", fetched_at: Date.now() };

test.beforeEach(async ({ page }) => {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
    thread: { cast: { hash, author: { fid: 1, username: "fixture" },
      text: "Video lifecycle fixture", ts: 1792235681, embeds: [{ kind: "video", url: stream }] } }, meta,
  } }));
  await page.route("**/api/v1/search?*", (route) => route.fulfill({ json: { query: "fixture", users: [], channels: [], casts: [], meta } }));
});

for (const status of [200, 503]) {
  test(`a playlist returning ${status} after navigation does not update a removed video`, async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    let release!: () => void;
    const pending = new Promise<void>((resolve) => { release = resolve; });
    let arrived!: () => void;
    const requested = new Promise<void>((resolve) => { arrived = resolve; });
    await page.route(stream, async (route) => {
      arrived();
      await pending;
      await route.fulfill({ status, body: readFileSync(path.join(__dirname, "fixtures/hls/master.m3u8")),
        contentType: "application/vnd.apple.mpegurl" });
    });
    await page.goto(`/p/${hash}`);
    await requested;
    const search = page.getByRole("searchbox", { name: "Search casts, people and channels", exact: true });
    await search.fill("fixture");
    await search.press("Enter");
    await expect(page).toHaveURL(/\/search\?q=fixture$/);
    const response = page.waitForResponse(stream);
    release();
    await response;
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    expect(errors).toEqual([]);
    await page.goBack();
    await expect(page.locator(".video")).toBeVisible();
  });
}

test("an autoplay timeout after navigation does not access the removed video", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route(stream, (route) => route.fulfill({ body: readFileSync(path.join(__dirname, "fixtures/hls/master.m3u8")),
    contentType: "application/vnd.apple.mpegurl" }));
  await page.route("https://media.example/stream_*.m3u8", (route) => route.fulfill({ status: 503 }));
  await page.goto(`/p/${hash}`);
  await expect(page.locator(".video")).toHaveClass(/autoplay-pending/);
  const search = page.getByRole("searchbox", { name: "Search casts, people and channels", exact: true });
  await search.fill("fixture");
  await search.press("Enter");
  await expect(page).toHaveURL(/\/search\?q=fixture$/);
  // Wait past the 2.5s deadline with real browser timer IDs. Playwright's
  // fake IDs exceed i32::MAX, so web-sys cannot clear them correctly.
  await page.waitForTimeout(3000);
  expect(errors).toEqual([]);
});
