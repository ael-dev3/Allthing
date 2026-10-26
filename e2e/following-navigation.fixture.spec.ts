// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";
import { signIn } from "./helpers/farcaster";

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no signed-in Following tab");

test("Following sort links and time changes never prefetch an unrelated feed", async ({ page }) => {
  await signIn(page, { fid: 1234, username: "reader", display_name: "Reader" });
  const requested: string[] = [];
  const meta = { backend: "hypersnap", fetched_at: Date.now() };
  const pool = [{
    hash: `0x${"a".repeat(40)}`, author: { fid: 1, username: "author", display_name: "Author" },
    text: "A followed cast", ts: Math.floor(Date.now() / 1000) - 60, likes: 1, recasts: 0, replies: 0,
  }];
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  for (const endpoint of ["following", "home", "latest"]) {
    await page.route(`**/api/v1/${endpoint}**`, (route) => {
      requested.push(endpoint);
      return route.fulfill({ json: { pool, notes: {}, meta }, headers: { "cache-control": "private, no-store" } });
    });
  }
  await page.route("**/api/v1/notifications**", (route) => route.fulfill({ json: { groups: [], meta } }));
  await page.route("https://haatz.quilibrium.com/**", (route) => route.fulfill({
    headers: { "access-control-allow-origin": "*" }, json: { messages: [] },
  }));
  await page.goto("/?feed=following");
  const following = page.getByRole("navigation", { name: "Feed", exact: true }).getByRole("link", { name: "Following", exact: true });
  await expect(following).toHaveAttribute("aria-current", "page");
  await expect(page.locator("#app .cast-card[data-hash]").first()).toContainText("A followed cast");
  expect(requested).toEqual(["following"]);

  const sortLinks = page.getByRole("navigation", { name: "Sort", exact: true });
  const range = page.getByRole("combobox", { name: "Time range", exact: true });
  await expect(sortLinks.getByRole("link", { name: "new", exact: true })).toHaveAttribute("aria-current", "page");
  // Hot is an explicit override in Following; omitting it would select New.
  await expect(sortLinks.getByRole("link", { name: "hot", exact: true })).toHaveAttribute("href", /[?&]sort=hot/);
  // Actual link presses exercise nav::go/fetch_ahead, unlike direct goto.
  for (const sort of ["new", "top", "hot"]) {
    await sortLinks.getByRole("link", { name: sort, exact: true }).click();
    await expect(page).toHaveURL(/feed=following/);
    await expect.poll(() => new URL(page.url()).searchParams.get("sort") || "new").toBe(sort);
    await expect(sortLinks.getByRole("link", { name: sort, exact: true })).toHaveAttribute("aria-current", "page");
    await expect(range).toBeVisible();
    for (const window of ["6h", "hour"]) {
      await range.selectOption(window);
      await expect(page).toHaveURL(new RegExp(`t=${window}`));
      await expect(following).toHaveAttribute("aria-current", "page");
      expect(new URL(page.url()).searchParams.get("sort") || "new").toBe(sort);
    }
  }
  // The private answer stays cached while every sort/range is changed.
  expect(requested).toEqual(["following"]);
});
