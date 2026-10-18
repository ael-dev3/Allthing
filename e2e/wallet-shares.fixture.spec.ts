// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const user = {
  fid: 42,
  username: "alice",
  display_name: "Alice",
  bio: "Reads conversations.",
  follower_count: 120,
  following_count: 30,
  registered_at: 1759222921,
  eth_addresses: ["0x76d0e7a13248945ee9f808b4a472262b28778942"],
};
const evm = (chain: string, name: string, count: number, percent: number) =>
  ({ chain, name, kind: "evm", count, capped: false, percent });

async function serve(page: Page, chains: object[]) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: { user, pool: [], notes: {}, meta } }));
  await page.route("**/api/v1/vote?*", (route) => route.fulfill({ json: { fid: 42, onchain: "stay", meta } }));
  await page.route("**/api/v1/wallets?*", (route) => route.fulfill({ json: {
    fid: 42,
    addresses: [{ address: user.eth_addresses[0], kind: "evm", primary: true }],
    chains,
    total: 33090,
    sources: [],
    fetched_at: Date.now(),
  } }));
}

for (const width of [1280, 390]) {
  test(`wallet shares follow the current rules in a payload cached under older ones at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    // As the edge cached it before chains under 1% were left out: Optimism's
    // 95 of 33,090 transactions (0.3%) rounded up to 1%.
    await serve(page, [
      evm("base", "Base", 32520, 98),
      evm("ethereum", "Ethereum", 388, 1),
      evm("optimism", "Optimism", 95, 1),
      evm("monad", "Monad", 66, 0),
      evm("arbitrum", "Arbitrum", 19, 0),
    ]);
    await page.goto("/u/alice");
    const wallets = page.locator("section.wallets");
    await expect(wallets.locator(".share-list li")).toHaveCount(2);
    await expect(wallets.locator(".share-list")).toHaveText(/Base\s*99%\s*Ethereum\s*1%/);
    await expect(wallets.locator(".share-bar")).toHaveAttribute("aria-label", "Base 99%, Ethereum 1%");
    await expect(wallets).not.toContainText("Optimism");
  });
}
