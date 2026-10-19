// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page, type TestInfo } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const user = {
  fid: 42, username: "alice", display_name: "Alice", bio: "Reads conversations.",
  follower_count: 120, following_count: 30, registered_at: 1759222921,
  eth_addresses: ["0x76d0e7a13248945ee9f808b4a472262b28778942"],
};

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: { user, pool: [], notes: {}, meta } }));
  await page.route("**/api/v1/vote?*", (route) => route.fulfill({ json: { fid: 42, onchain: "stay", meta } }));
  await page.route("**/api/v1/wallets?*", (route) => route.fulfill({ json: {
    fid: 42, addresses: [{ address: user.eth_addresses[0], kind: "evm", primary: true }],
    chains: [{ chain: "base", name: "Base", kind: "evm", count: 120, capped: false, percent: 100 }],
    total: 120, sources: [], fetched_at: Date.now(),
  } }));
}

test("the Wallets button opens its list on hover, and closes when the pointer leaves", async ({ page }, info: TestInfo) => {
  test.skip(Boolean(info.project.use.hasTouch), "hover needs a mouse");
  await serve(page);
  await page.goto("/u/alice");
  const button = page.getByRole("button", { name: "Show 1 verified wallet", exact: true });
  await button.hover();
  const list = page.getByRole("dialog", { name: "Verified wallets" });
  await expect(list).toBeVisible();
  await expect(list).toContainText("0x76d0");
  await page.mouse.move(0, 0);
  await expect(list).toHaveCount(0);
});

test("a tap on the Wallets button opens its list, and a second tap closes it", async ({ page }, info: TestInfo) => {
  test.skip(!info.project.use.hasTouch, "tap needs a touch screen");
  await serve(page);
  await page.goto("/u/alice");
  const button = page.getByRole("button", { name: "Show 1 verified wallet", exact: true });
  await button.tap();
  const list = page.getByRole("dialog", { name: "Verified wallets" });
  await expect(list).toBeVisible();
  await button.tap();
  await expect(list).toHaveCount(0);
});
