// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page, type Route } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const meta = { backend: "hypersnap", fetched_at: Date.now() };
// One pool per trending window, told apart by their text.
const pools: Record<string, string> = { "": "Today", "1h": "Past hour", "7d": "Past week" };
const pool = (window: string) =>
  [0, 1, 2].map((i) => ({
    hash: `0x${String(Object.keys(pools).indexOf(window) + 1).repeat(39)}${i}`,
    author: { fid: 1, username: "fixture" },
    text: `${pools[window]} cast ${i}`,
    ts: 1792235681 - i,
    likes: 3 - i,
  }));

async function serve(page: Page) {
  const calls: string[] = [];
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route: Route) => {
    const url = new URL(route.request().url());
    const window = url.searchParams.get("window") ?? "";
    const offset = url.searchParams.get("offset");
    // First pages only: a short list also asks for the next pages.
    if (!offset) calls.push(url.pathname + url.search);
    return route.fulfill({ json: { pool: offset ? [] : pool(window), meta } });
  });
  return calls;
}

test("the time filter picks the trending window and is kept in the URL", async ({ page }) => {
  const calls = await serve(page);
  await page.goto("/");
  const cards = page.locator(".cast-card[data-hash]");
  await expect(cards.first()).toContainText("Today cast");
  const range = page.getByRole("combobox", { name: "Time range" });
  await expect(range).toHaveValue("day");

  await page.getByRole("navigation", { name: "Sort", exact: true }).getByRole("link", { name: "top", exact: true }).click();
  await expect(page).toHaveURL(/\?sort=top$/);

  await range.selectOption("hour");
  await expect(page).toHaveURL(/[?&]t=hour/);
  await expect(page).toHaveURL(/[?&]sort=top/);
  await expect(cards.first()).toContainText("Past hour cast");
  expect(calls).toContain("/api/v1/home?window=1h");

  // Longer than Hypersnap's widest window: the past week's casts, and a note.
  await range.selectOption("month");
  await expect(page).toHaveURL(/[?&]t=month/);
  await expect(cards.first()).toContainText("Past week cast");
  expect(calls).toContain("/api/v1/home?window=7d");
  await expect(page.locator(".range-note")).toContainText("reaches back 7 days");

  // The same window again needs no new request, and no note.
  const before = calls.length;
  await range.selectOption("week");
  await expect(page).toHaveURL(/[?&]t=week/);
  await expect(page.locator(".range-note")).toHaveCount(0);
  await expect(cards.first()).toContainText("Past week cast");
  expect(calls.length).toBe(before);

  // The sort tabs keep the range; "new" is newest first, with no filter.
  await page.getByRole("navigation", { name: "Sort", exact: true }).getByRole("link", { name: "new", exact: true }).click();
  await expect(page).toHaveURL(/[?&]t=week/);
  await expect(page).toHaveURL(/[?&]sort=new/);
  await expect(range).toHaveCount(0);
});

test("a link with a time range costs one request, started inline", async ({ page }) => {
  const calls = await serve(page);
  await page.goto("/?sort=top&t=week");
  await expect(page.locator(".cast-card[data-hash]").first()).toContainText("Past week cast");
  await expect(page.getByRole("combobox", { name: "Time range" })).toHaveValue("week");
  // The inline script asked for the same path the app needs, and the app took it.
  expect(await page.evaluate(() => "__allthingEarly" in window)).toBe(false);
  expect(calls).toEqual(["/api/v1/home?window=7d"]);
});
