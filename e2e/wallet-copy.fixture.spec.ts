// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { activateVisible } from "./helpers/pointer";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
type ClipboardWindow = Window & { finishCopy: () => void; copyCalls: number };
const address = `0x${"b".repeat(40)}`;

test.beforeEach(async ({ page }) => {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: {
    user: { fid: 1, username: "fixture", eth_addresses: [address] },
    pool: [], meta: { backend: "hypersnap", fetched_at: Date.now() },
  } }));
});

async function openWallets(page: Page) {
  await page.goto("/u/fixture");
  // These fixture endpoints return 404. Let their loading rows settle
  // before opening a fixed panel beside a trigger they would otherwise move.
  await expect(page.locator(".profile-wallets")).toContainText("Wallet activity is unavailable right now.");
  await expect(page.locator(".vote-loading")).toHaveCount(0);
  await activateVisible(page.getByRole("button", { name: "Show 1 verified wallet", exact: true }));
}

test("copy feedback waits for the clipboard write", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: {
      writeText: () => {
        (window as ClipboardWindow).copyCalls = ((window as ClipboardWindow).copyCalls ?? 0) + 1;
        return new Promise<void>((resolve) => { (window as ClipboardWindow).finishCopy = resolve; });
      },
    } });
  });
  await openWallets(page);
  const dialog = page.getByRole("dialog", { name: "Verified wallets" });
  const copy = dialog.getByRole("button", { name: `Copy ${address}`, exact: true });
  await expect(copy).toBeInViewport({ ratio: 1 });
  await activateVisible(copy);
  await expect(copy).toHaveText("Copying…");
  await expect(copy).toBeDisabled();
  await expect(copy).toBeFocused();
  await copy.dispatchEvent("click");
  expect(await page.evaluate(() => (window as ClipboardWindow).copyCalls)).toBe(1);
  await page.evaluate(() => (window as ClipboardWindow).finishCopy());
  await expect(copy).toHaveText("Copied");
  await expect(copy).toBeEnabled();
});

test("a refused clipboard write can be retried without false success", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.addInitScript(() => {
    let attempts = 0;
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: {
      writeText: async () => { if (++attempts === 1) throw new DOMException("Denied", "NotAllowedError"); },
    } });
  });
  await openWallets(page);
  const dialog = page.getByRole("dialog", { name: "Verified wallets" });
  const copy = dialog.getByRole("button", { name: `Copy ${address}`, exact: true });
  await expect(copy).toBeInViewport({ ratio: 1 });
  await activateVisible(copy);
  await expect(dialog).toContainText("Could not copy. Select and copy the address manually.");
  await expect(copy).not.toHaveText("Copied");
  await expect(dialog.locator(".wallet-copy-error code")).toHaveText(address);
  const box = await dialog.boundingBox();
  expect(box!.x).toBeGreaterThanOrEqual(0);
  expect(box!.x + box!.width).toBeLessThanOrEqual(390);
  await activateVisible(copy);
  await expect(copy).toHaveText("Copied");
});

test("an unavailable clipboard API exposes the full address for manual copy", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: undefined });
  });
  await openWallets(page);
  const dialog = page.getByRole("dialog", { name: "Verified wallets" });
  const copy = dialog.getByRole("button", { name: `Copy ${address}`, exact: true });
  await expect(copy).toBeInViewport({ ratio: 1 });
  await activateVisible(copy);
  await expect(dialog.locator(".wallet-copy-error code")).toHaveText(address);
});
