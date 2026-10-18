// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const hash = `0x${"a".repeat(40)}`;

test.beforeEach(async ({ page }) => {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home", (route) => route.fulfill({ json: {
    pool: Array.from({ length: 15 }, (_, i) => ({
      hash: `0x${i.toString(16).padStart(40, "0")}`, author: { fid: 1, username: "fixture" },
      text: `Reaction dialog fixture ${i}`, ts: 1792235681 - i, likes: 10,
    })), meta: { backend: "hypersnap", fetched_at: Date.now() },
  } }));
  await page.route("**/api/v1/reactions?*", (route) => route.fulfill({ json: {
    hash, kind: "likes", users: [{ fid: 2, username: "reader" }, { fid: 3, username: "other" }],
    meta: { backend: "hypersnap", fetched_at: Date.now() },
  } }));
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: {
    user: { fid: 2, username: "reader" }, pool: [],
    meta: { backend: "hypersnap", fetched_at: Date.now() },
  } }));
});

test("choosing a person closes the reaction dialog on their profile", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "10 likes: show who", exact: true }).first().click();
  const dialog = page.locator(".who-modal");
  const document = await page.locator("html").elementHandle();
  await dialog.getByRole("link", { name: /@reader/ }).click();
  await expect(page).toHaveURL(/\/u\/reader$/);
  expect(await document!.evaluate((root) => root.isConnected).catch(() => false)).toBe(true);
  await expect(dialog).toHaveCount(0, { timeout: 1500 });
});

test("Back closes a reaction dialog when the underlying page changes", async ({ page }) => {
  await page.goto("/channels");
  await page.getByRole("banner").getByRole("link", { name: "Home", exact: true }).click();
  await page.getByRole("button", { name: "10 likes: show who", exact: true }).first().click();
  await expect(page.locator(".who-modal")).toBeVisible();
  await page.goBack();
  await expect(page).toHaveURL(/\/channels$/);
  await expect(page.locator(".who-modal")).toHaveCount(0, { timeout: 1500 });
});

test("Escape returns keyboard focus to the reaction count", async ({ page }) => {
  await page.goto("/");
  const trigger = page.getByRole("button", { name: "10 likes: show who", exact: true }).first();
  await trigger.click();
  await expect(page.locator(".who-modal").getByRole("button", { name: "Close", exact: true })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(page.locator(".who-modal")).toHaveCount(0);
  await expect(trigger).toBeFocused({ timeout: 1500 });
});

test("Tab and Shift+Tab stay inside the reaction dialog", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "10 likes: show who", exact: true }).first().click();
  const dialog = page.locator(".who-modal");
  const close = dialog.getByRole("button", { name: "Close", exact: true });
  const last = dialog.getByRole("link").last();
  await expect(last).toBeVisible();
  await close.focus();
  await page.keyboard.press("Shift+Tab");
  await expect(last).toBeFocused({ timeout: 1500 });
  await page.keyboard.press("Tab");
  await expect(close).toBeFocused({ timeout: 1500 });
});

test("the reaction dialog locks page scrolling until it closes", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "10 likes: show who", exact: true }).first().click();
  const dialog = page.locator(".who-modal");
  await expect(dialog).toBeVisible();
  expect(await page.evaluate(() => getComputedStyle(document.documentElement).overflowY)).toBe("hidden");
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).overflowY)).not.toBe("hidden");
});

test("a loading dialog traps focus on its close button and can be dismissed", async ({ page }) => {
  let release!: () => void;
  const pending = new Promise<void>((resolve) => { release = resolve; });
  await page.route("**/api/v1/reactions?*", async (route) => {
    await pending;
    await route.fulfill({ status: 503, json: { error: "Busy" } });
  });
  await page.goto("/");
  await page.getByRole("button", { name: "10 likes: show who", exact: true }).first().click();
  const dialog = page.locator(".who-modal");
  const close = dialog.getByRole("button", { name: "Close", exact: true });
  try {
    await expect(dialog).toContainText("Loading…");
    await page.keyboard.press("Tab");
    await expect(close).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await expect(close).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
  } finally {
    release();
  }
});

test("on a phone the backdrop closes the dialog without activating a card", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  // A fine pointer still exercises the full-dialog trigger on this viewport.
  await page.goto("/");
  const trigger = page.getByRole("button", { name: "10 likes: show who", exact: true }).first();
  await trigger.click();
  await expect(page.locator(".who-modal")).toBeVisible();
  await page.locator(".modal-backdrop").click({ position: { x: 4, y: 4 } });
  await expect(page.locator(".who-modal")).toHaveCount(0);
  await expect(page).toHaveURL(/\/$/);
  await expect(trigger).toBeFocused();
});
