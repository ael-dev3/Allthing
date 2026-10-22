// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

// The module comes from the Worker at /wasm/ and, failing that, from
// /assets/; its JavaScript from the build's folder in /assets/.
const paths = { wasm: "**/{assets,wasm}/allthing-*.wasm", js: "**/assets/allthing-*/allthing.js" };

for (const width of [1280, 390]) {
  for (const asset of ["wasm", "js"] as const) {
    test(`a failed ${asset} load offers a working reload at width ${width}`, async ({ page }, info) => {
      await page.setViewportSize({ width, height: 844 });
      let blocked = true;
      const errors: string[] = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await page.route(paths[asset], (route) => blocked ? route.abort("failed") : route.continue());
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

for (const width of [1280, 390]) {
  test(`the app loads the static copy of its module when the Worker can't serve it at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const modules: string[] = [];
    page.on("request", (request) => {
      const path = new URL(request.url()).pathname;
      // The app's own module only (allthing-<hash>.wasm), not the parts it
      // fetches when idle (split_*.wasm, chunk_*.wasm), such as the poll's.
      if (/\/allthing-[0-9a-f]+\.wasm$/.test(path)) modules.push(path.split("/")[1]);
    });
    await page.route("**/wasm/**", (route) => route.fulfill({ status: 429, contentType: "text/plain", body: "Too many requests" }));
    await page.route("**/api/v1/**", (route) => route.fulfill({ json: {
      pool: [], meta: { backend: "hypersnap", fetched_at: Date.now() },
    } }));
    await page.goto("/?sort=top&t=week");
    await expect(page.getByRole("combobox", { name: "Search casts, people and channels", exact: true })).toBeVisible();
    await expect(page.getByRole("alert")).toHaveCount(0);
    expect(modules).toEqual(["wasm", "assets"]);
    expect(errors).toEqual([]);
  });

  test(`the module comes from the Worker at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    const modules: { path: string, status: number, type: string | undefined }[] = [];
    page.on("response", (response) => {
      const path = new URL(response.url()).pathname;
      if (path.endsWith(".wasm")) modules.push({ path, status: response.status(), type: response.headers()["content-type"] });
    });
    await page.route("**/api/v1/**", (route) => route.fulfill({ json: {
      pool: [], meta: { backend: "hypersnap", fetched_at: Date.now() },
    } }));
    await page.goto("/");
    await expect(page.getByRole("combobox", { name: "Search casts, people and channels", exact: true })).toBeVisible();
    expect(modules).toHaveLength(1);
    expect(modules[0].path).toMatch(/^\/wasm\/allthing-[0-9a-f]+\.wasm$/);
    expect(modules[0].status).toBe(200);
    expect(modules[0].type).toBe("application/wasm");
  });
}
