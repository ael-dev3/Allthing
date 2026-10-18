// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
type ClipboardWindow = Window & { finishCopy: () => void };
const address = `0x${"b".repeat(40)}`;

test.beforeEach(async ({ page }) => {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: {
    user: { fid: 1, username: "fixture", eth_addresses: [address] },
    pool: [], meta: { backend: "hypersnap", fetched_at: Date.now() },
  } }));
});

test("copy feedback waits for the clipboard write", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: {
      writeText: () => new Promise<void>((resolve) => { (window as ClipboardWindow).finishCopy = resolve; }),
    } });
  });
  await page.goto("/u/fixture");
  await page.getByRole("button", { name: "Show 1 verified wallet", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Verified wallets" });
  const copy = dialog.getByRole("button", { name: `Copy ${address}`, exact: true });
  await copy.click();
  await expect(copy).toHaveText("Copying…");
  await expect(copy).toBeDisabled();
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
  await page.goto("/u/fixture");
  await page.getByRole("button", { name: "Show 1 verified wallet", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Verified wallets" });
  const copy = dialog.getByRole("button", { name: `Copy ${address}`, exact: true });
  await copy.click();
  await expect(dialog).toContainText("Could not copy. Select and copy the address manually.");
  await expect(copy).not.toHaveText("Copied");
  await expect(dialog.locator(".wallet-copy-error code")).toHaveText(address);
  const box = await dialog.boundingBox();
  expect(box!.x).toBeGreaterThanOrEqual(0);
  expect(box!.x + box!.width).toBeLessThanOrEqual(390);
  await copy.click();
  await expect(copy).toHaveText("Copied");
});

test("an unavailable clipboard API exposes the full address for manual copy", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: undefined });
  });
  await page.goto("/u/fixture");
  await page.getByRole("button", { name: "Show 1 verified wallet", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Verified wallets" });
  await dialog.getByRole("button", { name: `Copy ${address}`, exact: true }).click();
  await expect(dialog.locator(".wallet-copy-error code")).toHaveText(address);
});
