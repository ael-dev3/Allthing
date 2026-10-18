// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const hash = `0x${"a".repeat(40)}`;
const cast = { hash, author: { fid: 1, username: "fixture" }, text: "Lightbox navigation fixture", ts: 1792235681,
  embeds: [{ kind: "image", url: "https://media.example/image.png" }] };
const meta = { backend: "hypersnap", fetched_at: Date.now() };

test.beforeEach(async ({ page }) => {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/search?*", (route) => route.fulfill({ json: { query: "fixture", users: [], channels: [], casts: [cast], meta } }));
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: { thread: { cast }, meta } }));
  await page.route("https://media.example/image.png", (route) => route.fulfill({ contentType: "image/png",
    body: Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aP1cAAAAASUVORK5CYII=", "base64") }));
});

test("Back dismisses a lightbox and unlocks the destination page", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/search?q=fixture");
  await page.locator(`.cast-card[data-hash="${hash}"] header.cast-meta a.when`).click();
  await expect(page).toHaveURL(new RegExp(`/p/${hash}$`));
  await page.locator(".post .tile-image").click();
  const dialog = page.getByRole("dialog", { name: "Media viewer", exact: true });
  await expect(dialog).toBeVisible();
  await page.goBack();
  await expect(page).toHaveURL(/\/search\?q=fixture$/);
  await expect(dialog).toHaveCount(0, { timeout: 1500 });
  await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).overflowY)).not.toBe("hidden");
  expect(errors).toEqual([]);
});

test("Escape still returns focus to the image that opened the lightbox", async ({ page }) => {
  await page.goto(`/p/${hash}`);
  const image = page.locator(".post .tile-image");
  await image.click();
  const dialog = page.getByRole("dialog", { name: "Media viewer", exact: true });
  await expect(dialog.getByRole("button", { name: "Close media viewer (Escape)", exact: true })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(image).toBeFocused();
});
