// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

for (const width of [1280, 390]) {
  for (const asset of ["wasm", "js"]) {
    test(`a failed ${asset} load offers a working reload at width ${width}`, async ({ page }, info) => {
      await page.setViewportSize({ width, height: 844 });
      let blocked = true;
      const errors: string[] = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await page.route(`**/assets/allthing-*.${asset}`, (route) => blocked ? route.abort("failed") : route.continue());
      await page.route("**/api/v1/**", (route) => route.fulfill({ json: {
        pool: [], meta: { backend: "hypersnap", fetched_at: Date.now() },
      } }));
      await page.goto("/?sort=top&t=week");
      const originalURL = page.url();
      // Capture both a failed baseline and the rendered recovery state.
      try {
        await expect(page.getByRole("alert")).toContainText("Allthing couldn’t load", { timeout: 2500 });
      } finally {
        await page.screenshot({ path: info.outputPath(`boot-${asset}-${width}.png`) });
      }
      const reload = page.getByRole("button", { name: "Reload", exact: true });
      await expect(reload).toBeVisible();
      blocked = false;
      await reload.click();
      await expect(page.getByRole("combobox", { name: "Search casts, people and channels", exact: true })).toBeVisible();
      await expect(page).toHaveURL(originalURL);
      await expect(page.getByRole("alert")).toHaveCount(0);
      await expect(page.locator("#boot")).toHaveCount(0);
      expect(errors).toEqual([]);
    });
  }
}
