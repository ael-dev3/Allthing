// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const user = {
  fid: 42, username: "alice", display_name: "Alice", bio: "Reads conversations.",
  follower_count: 120, following_count: 30, registered_at: 1759222921,
  eth_addresses: ["0x76d0e7a13248945ee9f808b4a472262b28778942"],
};

// Base couldn't be read this time; its count is carried from the last read.
async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: { user, pool: [], notes: {}, meta } }));
  await page.route("**/api/v1/vote?*", (route) => route.fulfill({ json: { fid: 42, onchain: "stay", meta } }));
  await page.route("**/api/v1/wallets?*", (route) => route.fulfill({ json: {
    fid: 42, addresses: [{ address: user.eth_addresses[0], kind: "evm", primary: true }],
    chains: [
      { chain: "base", name: "Base", kind: "evm", count: 990, capped: false, percent: 99, carried: true },
      { chain: "optimism", name: "Optimism", kind: "evm", count: 10, capped: false, percent: 1 },
    ],
    total: 1000, sources: [], fetched_at: Date.now(),
  } }));
}

test("a chain's count carried from an earlier read keeps its share, and the chart says so", async ({ page }) => {
  await serve(page);
  await page.goto("/u/alice");
  const wallets = page.locator(".profile-wallets");
  await expect(wallets.locator(".share-list")).toContainText("Base");
  await expect(wallets.locator(".share-list")).toContainText("99%");
  await expect(wallets).toContainText("Some counts are from an earlier read");
  await expect(wallets).not.toContainText("Some chains weren't read just now");
});
