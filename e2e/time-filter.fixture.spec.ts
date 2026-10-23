// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page, type Route } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const meta = { backend: "hypersnap", fetched_at: Date.now() };
// One pool per window, told apart by their text.
const pools: Record<string, string> = {
  "": "Today",
  "1h": "Past hour",
  "7d": "Past week",
  "30d": "Past month",
  "365d": "Past year",
  all: "All time",
};
const indexed = ["30d", "365d", "all"];
const pool = (window: string) =>
  [0, 1, 2].map((i) => ({
    hash: `0x${String(Object.keys(pools).indexOf(window) + 1).repeat(39)}${i}`,
    author: { fid: 1, username: "fixture" },
    text: `${pools[window]} cast ${i}`,
    ts: Math.floor(Date.now() / 1000) - 60 * (i + 1),
    likes: 3 - i,
  }));
// 12 Mar 2024, the oldest cast in the fixture index.
const since = 1710201600;

async function serve(page: Page, { index = true } = {}) {
  const calls: string[] = [];
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route: Route) => {
    const url = new URL(route.request().url());
    const window = url.searchParams.get("window") ?? "";
    const offset = url.searchParams.get("offset");
    // First pages only: a short list also asks for the next pages.
    if (!offset) calls.push(url.pathname + url.search);
    if (indexed.includes(window) && !index) {
      // No ranking yet: the Worker answers with the past week, and no `index`.
      return route.fulfill({ json: { pool: offset ? [] : pool("7d"), meta } });
    }
    const coverage = indexed.includes(window) ? { index: { casts: 12345, since, ranked_at: Date.now() } } : {};
    return route.fulfill({ json: { pool: offset ? [] : pool(window), meta, ...coverage } });
  });
  return calls;
}

test("the time filter picks the window and is kept in the URL", async ({ page }) => {
  const calls = await serve(page);
  await page.goto("/");
  const cards = page.locator(".cast-card[data-hash]");
  await expect(cards.first()).toContainText("Today cast");
  const range = page.getByRole("combobox", { name: "Time range" });
  await expect(range).toHaveValue("day");
  await expect(range.locator("option")).toHaveText([
    "Past hour",
    "6 hours",
    "12 hours",
    "24 hours",
    "7 days",
    "30 days",
    "All time",
  ]);

  await page.getByRole("navigation", { name: "Sort", exact: true }).getByRole("link", { name: "top", exact: true }).click();
  await expect(page).toHaveURL(/\?sort=top$/);

  await range.selectOption("hour");
  await expect(page).toHaveURL(/[?&]t=hour/);
  await expect(page).toHaveURL(/[?&]sort=top/);
  await expect(cards.first()).toContainText("Past hour cast");
  expect(calls).toContain("/api/v1/home?window=1h");
  // The fixture's list has three casts, and the page says so once it has run out.
  await expect(page.locator(".range-note")).toHaveText("Only 3 casts in the past hour.");

  // Beyond a week, each range is ranked from Allthing's index, and says what it covers.
  for (const [t, window, label] of [
    ["month", "30d", "Past month"],
    ["all", "all", "All time"],
  ]) {
    await range.selectOption(t);
    await expect(page).toHaveURL(new RegExp(`[?&]t=${t}`));
    await expect(cards.first()).toContainText(`${label} cast`);
    expect(calls).toContain(`/api/v1/home?window=${window}`);
    await expect(page.locator(".range-note")).toHaveText(
      "Ranked from 12,345 casts Allthing has indexed back to 12 Mar 2024.",
    );
  }

  await range.selectOption("week");
  await expect(page).toHaveURL(/[?&]t=week/);
  await expect(cards.first()).toContainText("Past week cast");
  await expect(page.locator(".range-note")).toHaveText("Only 3 casts in the past 7 days.");

  // The sort tabs keep the range, including New.
  await page.getByRole("navigation", { name: "Sort", exact: true }).getByRole("link", { name: "new", exact: true }).click();
  await expect(page).toHaveURL(/[?&]t=week/);
  await expect(page).toHaveURL(/[?&]sort=new/);
  await expect(range).toBeVisible();
  await expect(range).toHaveValue("week");
});

test("until the index can rank a range, the past week is shown and the page says so", async ({ page }) => {
  await serve(page, { index: false });
  await page.goto("/?sort=top&t=year");
  await expect(page.locator(".cast-card[data-hash]").first()).toContainText("Past week cast");
  await expect(page.locator(".range-note")).toHaveText(
    "Allthing's index can't rank this range yet, so this shows the past week's trending casts.",
  );
});

for (const [t, window, label] of [
  ["week", "7d", "Past week"],
  ["all", "all", "All time"],
]) {
  test(`a link to ${t} costs one request, started inline`, async ({ page }) => {
    const calls = await serve(page);
    await page.goto(`/?sort=top&t=${t}`);
    await expect(page.locator(".cast-card[data-hash]").first()).toContainText(`${label} cast`);
    await expect(page.getByRole("combobox", { name: "Time range" })).toHaveValue(t);
    // The inline script asked for the same path the app needs, and the app took it.
    expect(await page.evaluate(() => "__allthingEarly" in window)).toBe(false);
    expect(calls).toEqual([`/api/v1/home?window=${window}`]);
  });
}
