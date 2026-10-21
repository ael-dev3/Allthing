// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

/*
 * The static copy lives under /Allthing. What the app opens itself, rather
 * than through a link the reader clicks, stays under it: a cast's card, the
 * search box, a replies count and the time range. The router put its base
 * before those paths a second time (/Allthing/Allthing/p/...), which showed
 * "Page not found".
 */

test.skip(!process.env.E2E_PAGES, "the static copy, under /Allthing");

const hash = `0x${"a".repeat(40)}`;

async function serve(page: Page) {
  await page.route("https://haatz.quilibrium.com/**", (route) => {
    const url = new URL(route.request().url());
    if (url.pathname.endsWith("/feed/trending")) {
      return route.fulfill({
        json: {
          casts: url.searchParams.has("cursor") ? [] : [{
            hash, author: { fid: 1, username: "fixture" }, text: "A cast to open",
            timestamp: "2026-10-18T10:00:00Z", embeds: [], replies: { count: 2 }, reactions: { likes_count: 1, recasts_count: 0 },
          }],
        },
        headers: { "access-control-allow-origin": "*" },
      });
    }
    return route.fulfill({ status: 404, json: {}, headers: { "access-control-allow-origin": "*" } });
  });
  await page.goto("/Allthing/");
  await expect(page.getByText("A cast to open", { exact: true })).toBeVisible();
}

/** On `path` under /Allthing, and on a page the app knows. */
async function at(page: Page, path: RegExp) {
  await expect(page).toHaveURL(path);
  expect(new URL(page.url()).pathname).not.toContain("/Allthing/Allthing");
  await expect(page.getByRole("heading", { name: "Page not found" })).toHaveCount(0);
}

for (const width of [1280, 390]) {
  test(`at ${width}, a click on a cast's card opens its post under /Allthing`, async ({ browser }, testInfo) => {
    const phone = width === 390;
    const context = await browser.newContext({
      baseURL: testInfo.project.use.baseURL,
      viewport: { width, height: 844 },
      ...(phone ? { isMobile: true, hasTouch: true } : {}),
    });
    const page = await context.newPage();
    await serve(page);
    const text = page.getByText("A cast to open", { exact: true });
    if (phone) await text.tap();
    else await text.click();
    await at(page, new RegExp(`/Allthing/p/${hash}$`));
    await context.close();
  });
}

test("the search box searches under /Allthing", async ({ page }) => {
  await serve(page);
  const search = page.getByRole("combobox", { name: "Search casts, people and channels" });
  await search.fill("pepe");
  await search.press("Enter");
  await at(page, /\/Allthing\/search\?q=pepe$/);
});

test("a replies count, clicked, opens the post under /Allthing", async ({ page }) => {
  await serve(page);
  await page.getByRole("button", { name: /^2 replies/ }).click();
  await at(page, new RegExp(`/Allthing/p/${hash}$`));
});

test("the time range stays under /Allthing", async ({ page }) => {
  await serve(page);
  await page.getByRole("combobox", { name: "Time range" }).selectOption("week");
  await at(page, /\/Allthing\/\?t=week$/);
  await expect(page.getByText("A cast to open", { exact: true })).toBeVisible();
});
