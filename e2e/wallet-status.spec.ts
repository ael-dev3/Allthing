// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

for (const state of ["unavailable", "zero", "no-wallets", "active"] as const) {
  test(`wallet measurement status: ${state}`, async ({ page }) => {
    const address = `0x${"a".repeat(40)}`;
    const noWallets = state === "no-wallets";
    await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
    await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: {
      user: { fid: 1, username: "fixture", eth_addresses: noWallets ? [] : [address] },
      pool: [], meta: { backend: "hypersnap", fetched_at: Date.now() },
    } }));
    await page.route("**/api/v1/wallets?*", (route) => route.fulfill({ json: {
      fid: 1, addresses: noWallets ? [] : [{ address, kind: "evm", primary: true }],
      chains: noWallets ? [] : [{ chain: "base", name: "Base", kind: "evm", count: state === "active" ? 2 : 0,
        percent: state === "active" ? 100 : 0, ...(state === "unavailable" ? { error: "RPC unavailable" } : {}) }],
      total: state === "active" ? 2 : 0, sources: [], fetched_at: Date.now(),
    } }));
    await page.goto("/u/fixture");
    const panel = page.getByRole("region", { name: "Wallet activity by chain" });
    if (state === "unavailable") {
      await expect(panel).toContainText("Wallet activity is unavailable right now.");
      await expect(panel).not.toContainText("No transactions");
    } else if (state === "zero") {
      await expect(panel).toContainText("No transactions sent on the measured chains.");
    } else if (noWallets) {
      await expect(panel).toContainText("No verified wallets to measure.");
    } else {
      await expect(panel.locator(".share-list")).toContainText("Base");
      await expect(panel.locator(".share-list")).toContainText("100%");
    }
  });
}
