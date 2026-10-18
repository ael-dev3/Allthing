// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

for (const width of [1280, 390]) {
  test(`a valid JSON body with the wrong payload shape is requested again at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    let attempts = 0;
    const meta = { backend: "hypersnap", fetched_at: Date.now() };
    await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture" } }));
    await page.route("**/api/v1/home**", (route) => route.fulfill({ json: { pool: [], meta } }));
    await page.route("**/api/v1/search?*", (route) => route.fulfill({ json: ++attempts === 1 ? {} : {
      query: "recovery", users: [], channels: [], casts: [], meta,
    } }));
    await page.goto("/search?q=recovery");
    await expect(page.getByRole("alert")).toContainText("Unexpected data from the API");
    await page.getByRole("link", { name: "Allthing, home", exact: true }).click();
    const box = page.getByRole("combobox", { name: "Search casts, people and channels", exact: true });
    await box.fill("recovery");
    await box.press("Enter");
    await expect(page.getByText("Nothing found. Try another word or a username.", { exact: true }))
      .toBeVisible({ timeout: 2500 });
    expect(attempts).toBe(2);
  });
}
