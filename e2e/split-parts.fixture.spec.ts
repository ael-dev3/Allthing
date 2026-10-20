// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

/*
 * Parts of the app fetched the first time they're used (allthing-web's
 * `split`): the DAU dashboard's code comes with /dau, not before.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const DAY = 86_400;
const now = Math.floor(Date.now() / 1000);
const today = Math.floor(now / DAY);
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const activity = {
  reached: now - 60, rolling_to: Math.floor((now - 60) / 3_600) * 3_600, rolling_24h: 6_174, previous_24h: 5_982,
  since: (today - 29) * DAY,
  days: Array.from({ length: 30 }, (_, i) => ({ day: today - 29 + i, active: 6_000 + i, complete: i < 29, actions: { cast: 100 } })),
  meta,
};

/** The fixtures, and the WebAssembly the page fetches, by file name. */
async function serve(page: Page) {
  const modules: string[] = [];
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    if (path.endsWith(".wasm")) modules.push(path.split("/").pop()!.replace(/^allthing-[0-9a-f]+\.wasm$/, "app"));
  });
  await page.route("**/api/v1/**", (route) => route.fulfill({ json: { pool: [], meta } }));
  await page.route("**/api/v1/activity", (route) => route.fulfill({ json: activity }));
  return modules;
}

for (const width of [1280, 390]) {
  test(`the DAU dashboard's code is fetched on /dau, once, and not before, at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const modules = await serve(page);
    await page.goto("/");
    await expect(page.getByRole("combobox", { name: "Search casts, people and channels", exact: true })).toBeVisible();
    await page.waitForLoadState("networkidle");
    expect(modules).toEqual(["app"]);

    await page.goto("/dau");
    await expect(page.locator(".dau-meter")).toContainText("6,174");
    expect(modules).toEqual(["app", "app", "split_dau.wasm"]);
    // Away and back in the app: the code is already in.
    await page.getByRole("link", { name: "Allthing, home" }).click();
    await expect(page).toHaveURL(/\/$/);
    await page.goBack();
    await expect(page.locator(".dau-meter")).toContainText("6,174");
    expect(modules).toEqual(["app", "app", "split_dau.wasm"]);
    expect(errors).toEqual([]);
  });
}

test("a part that can't be fetched loads the page again once, then offers to try again", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await serve(page);
  // As when Allthing was deployed since the page was opened: the files of
  // the version it runs are gone.
  let blocked = true;
  await page.route("**/split_dau.wasm", (route) => blocked ? route.fulfill({ status: 404, contentType: "text/html", body: "<!doctype html>" }) : route.continue());
  let loads = 0;
  page.on("load", () => loads++);
  await page.goto("/dau");
  const alert = page.getByRole("alert");
  await expect(alert).toContainText("This part of Allthing couldn't load");
  expect(loads).toBe(2);
  await expect(page.locator(".dau-meter")).toHaveCount(0);

  blocked = false;
  await alert.getByRole("button", { name: "Try again" }).click();
  await expect(page.locator(".dau-meter")).toContainText("6,174");
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(loads).toBe(3);
  expect(errors).toEqual([]);
});
