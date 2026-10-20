// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

// 2023-11-07T19:42:51Z: what accounts registered before the ID registry's
// move to OP Mainnet report as their registration.
const MIGRATION = 1699386171;

async function profile(page: Page, user: object) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: {
    user: { fid: 3, username: "fixture", ...user },
    pool: [], meta: { backend: "hypersnap", fetched_at: Date.now() },
  } }));
  await page.goto("/u/fixture");
  return page.locator(".profile .fact", { hasText: "Joined" });
}

test("an account from before the migration joined when it got its first username", async ({ page }) => {
  const joined = await profile(page, { registered_at: MIGRATION, first_username_at: 1623888000 });
  await expect(joined.locator("dd")).toHaveText("17 Jun 2021");
  await expect(joined.locator("dd")).toHaveAttribute("aria-description", /^Got its first username .* ago\. The account was registered before the ID registry moved to OP Mainnet on 7 Nov 2023/);
});

test("without its first username's date it says before Nov 2023", async ({ page }) => {
  const joined = await profile(page, { registered_at: MIGRATION });
  await expect(joined.locator("dd")).toHaveText("before Nov 2023");
});

test("a newer account joined when it registered onchain", async ({ page }) => {
  const joined = await profile(page, { registered_at: 1715817600, first_username_at: 1715900000 });
  await expect(joined.locator("dd")).toHaveText("16 May 2024");
  await expect(joined.locator("dd")).toHaveAttribute("aria-description", /^Registered onchain .* ago\.$/);
});
