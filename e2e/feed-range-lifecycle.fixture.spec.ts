// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

for (const width of [1280, 390]) {
  test(`a pending page from another time range cannot block or finish this range at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    const calls: string[] = [];
    let releaseDay!: () => void, releaseHour!: () => void;
    const day = new Promise<void>((resolve) => { releaseDay = resolve; });
    const hour = new Promise<void>((resolve) => { releaseHour = resolve; });
    const meta = { backend: "hypersnap", fetched_at: Date.now() };
    await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture" } }));
    await page.route("**/api/v1/home**", async (route) => {
      const url = new URL(route.request().url());
      const window = url.searchParams.get("window") ?? "day";
      const offset = Number(url.searchParams.get("offset") ?? 0);
      calls.push(`${window}:${offset}`);
      if (offset >= 200) return route.fulfill({ status: 404, json: { error: "End" } });
      if (offset === 100) await (window === "day" ? day : hour);
      return route.fulfill({ json: { pool: [{
        hash: `0x${(window === "day" ? "a" : "b").repeat(38)}${offset === 100 ? "11" : "00"}`,
        author: { fid: 1, username: "fixture" }, text: `${window} page ${offset}`, ts: 1792235681,
      }], meta } });
    });
    try {
      await page.goto("/");
      await expect.poll(() => calls.includes("day:100")).toBe(true);
      await page.getByRole("combobox", { name: "Time range" }).selectOption("hour");
      await expect(page.locator(".cast-card[data-hash]").first()).toContainText("1h page 0");
      await expect.poll(() => calls.includes("1h:100"), { timeout: 2500 }).toBe(true);
      await expect(page.locator(".cast-card.skeleton")).toHaveCount(2);
      releaseDay();
      await expect.poll(() => page.evaluate(() => performance.getEntriesByType("resource")
        .some((entry) => entry.name.endsWith("/api/v1/home?offset=100")))).toBe(true);
      await page.waitForTimeout(100);
      // Finishing the old request must not clear the new range's loading state.
      await expect(page.locator(".cast-card.skeleton")).toHaveCount(2);
      await expect(page.getByText("day page 100", { exact: true })).toHaveCount(0);
      releaseHour();
      await expect(page.getByText("1h page 100", { exact: true })).toBeVisible();
      await expect(page.locator(".cast-card.skeleton")).toHaveCount(0);
    } finally { releaseDay(); releaseHour(); }
  });
}
