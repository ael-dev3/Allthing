// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const now = Math.floor(Date.now() / 1000);
const cast = (n: number, text: string, ts: number, likes = 0) => ({
  hash: `0x${n.toString(16).padStart(40, "0")}`, author: { fid: 1, username: "fixture" }, text, ts, likes,
});
const latest = [cast(1, "Newest cast", now - 20), cast(2, "A minute ago", now - 60), cast(3, "Two minutes ago", now - 120)];
const trending = [cast(9, "Trending cast", now - 7200, 50)];

async function serve(page: Page) {
  const calls: string[] = [];
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/latest", (route) => {
    calls.push("latest");
    return route.fulfill({ json: { pool: latest, cursors: [{ shard: 1, event_id: 12 }], meta } });
  });
  await page.route("**/api/v1/home**", (route) => {
    // First pages only: a short list also asks for the next pages.
    const offset = new URL(route.request().url()).searchParams.get("offset");
    if (!offset) calls.push("home");
    return route.fulfill({ json: { pool: offset ? [] : trending, meta } });
  });
  return calls;
}

test("new lists the newest casts with one request, started inline, and no trending read", async ({ page }) => {
  const calls = await serve(page);
  await page.goto("/?sort=new");
  const cards = page.locator(".cast-card[data-hash]");
  await expect(cards).toHaveCount(3);
  await expect(cards.first()).toContainText("Newest cast");
  await expect(cards.nth(2)).toContainText("Two minutes ago");
  expect(await page.evaluate(() => "__allthingEarly" in window)).toBe(false);
  expect(calls).toEqual(["latest"]);

  // Hot ranks the trending pool...
  const sort = page.getByRole("navigation", { name: "Sort", exact: true });
  await sort.getByRole("link", { name: "hot", exact: true }).click();
  await expect(cards.first()).toContainText("Trending cast");
  expect(calls).toEqual(["latest", "home"]);
  // ...and new again within half a minute reuses the list it has.
  await sort.getByRole("link", { name: "new", exact: true }).click();
  await expect(cards.first()).toContainText("Newest cast");
  expect(calls).toEqual(["latest", "home"]);
});
