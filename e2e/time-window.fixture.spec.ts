// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page, type Route } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const meta = { backend: "hypersnap", fetched_at: Date.now() };
// Four casts posted 30 minutes, 2 hours, 5 hours and 3 days ago. The page is
// given all four for every window (as a copy from the edge may be), and
// keeps the ones inside the window it shows.
const now = Math.floor(Date.now() / 1000);
const ages = [["30 minutes", 30 * 60], ["2 hours", 2 * 3600], ["5 hours", 5 * 3600], ["3 days", 3 * 86400]] as const;
const pool = ages.map(([label, age], i) => ({
  hash: `0x${String(i + 1).repeat(40)}`,
  author: { fid: 1, username: "fixture" },
  text: `Cast ${label} old`,
  ts: now - age,
  likes: 4 - i,
}));
const indexed = ["30d", "all"];
const since = 1710201600;

async function serve(page: Page) {
  const calls: string[] = [];
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route: Route) => {
    const url = new URL(route.request().url());
    const window = url.searchParams.get("window") ?? "";
    // First pages only: a short list also asks for the next pages, which are empty.
    if (!url.searchParams.has("offset")) calls.push(url.pathname + url.search);
    const coverage = indexed.includes(window) ? { index: { casts: 12345, since, ranked_at: Date.now() } } : {};
    return route.fulfill({ json: { pool: url.searchParams.has("offset") ? [] : pool, meta, ...coverage } });
  });
  return calls;
}

for (const width of [1280, 390]) {
  test.describe(`at ${width} px`, () => {
    test.use({ viewport: { width, height: 800 } });

    test("the range menu offers the seven ranges, narrowest first", async ({ page }) => {
      await serve(page);
      await page.goto("/");
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
    });

    test("each range keeps only the casts posted inside it, and says how many there are", async ({ page }) => {
      const calls = await serve(page);
      await page.goto("/");
      const cards = page.locator(".cast-card[data-hash]");
      const range = page.getByRole("combobox", { name: "Time range" });
      await expect(cards).toHaveCount(3);

      const cases: [string, string[], string | null][] = [
        ["hour", ["30 minutes"], "Only 1 cast in the past hour."],
        ["6h", ["30 minutes", "2 hours", "5 hours"], "Only 3 casts in the past 6 hours."],
        ["12h", ["30 minutes", "2 hours", "5 hours"], "Only 3 casts in the past 12 hours."],
        ["day", ["30 minutes", "2 hours", "5 hours"], "Only 3 casts in the past 24 hours."],
        ["week", ["30 minutes", "2 hours", "5 hours", "3 days"], "Only 4 casts in the past 7 days."],
        ["month", ["30 minutes", "2 hours", "5 hours", "3 days"], null],
        ["all", ["30 minutes", "2 hours", "5 hours", "3 days"], null],
      ];
      for (const [value, shown, note] of cases) {
        await range.selectOption(value);
        // The default range (24 hours) is left out of the URL.
        if (value !== "day") await expect(page).toHaveURL(new RegExp(`[?&]t=${value}`));
        await expect(cards).toHaveCount(shown.length);
        for (const [i, label] of shown.entries()) {
          await expect(cards.nth(i)).toContainText(`Cast ${label} old`);
        }
        if (note) await expect(page.locator(".range-note")).toHaveText(note);
      }
      // Each window's first page is asked for once, with its own window.
      expect(calls).toContain("/api/v1/home?window=1h");
      expect(calls).toContain("/api/v1/home?window=6h");
      expect(calls).toContain("/api/v1/home?window=12h");
      expect(calls).toContain("/api/v1/home?window=7d");
      expect(calls).toContain("/api/v1/home?window=30d");
    });
  });
}
