// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { NOW } from "./fixture-time";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const hash = `0x${"d".repeat(40)}`;
const src = "https://decode.media.example/large.svg";
const meta = { backend: "hypersnap", fetched_at: Date.now() };
type DecodeWindow = Window & { releaseImageDecode: () => void; imageDecodeCalls: number };

async function serve(page: Page, mode: "held" | "rejected") {
  await page.route("**/api/v1/**", route => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", route => route.fulfill({ json: { pool: [], notes: {}, meta } }));
  await page.route("**/api/v1/cast?*", route => route.fulfill({ json: {
    thread: { cast: { hash, author: { fid: 7, username: "lens" }, text: "A large original",
      ts: NOW - 19, embeds: [{ kind: "image", url: src }] } }, notes: {}, meta,
  } }));
  await page.route(src, route => route.fulfill({ contentType: "image/svg+xml", body:
    '<svg xmlns="http://www.w3.org/2000/svg" width="4284" height="4284"><rect width="4284" height="4284" fill="#5544aa"/></svg>' }));
  await page.addInitScript(({ src, mode }) => {
    const decode = HTMLImageElement.prototype.decode;
    const state = window as DecodeWindow;
    state.imageDecodeCalls = 0;
    let release!: () => void;
    const held = new Promise<void>(resolve => { release = resolve; });
    state.releaseImageDecode = release;
    HTMLImageElement.prototype.decode = function () {
      if (this.src !== src) return decode.call(this);
      state.imageDecodeCalls++;
      return mode === "held" ? held : Promise.reject(new Error("Fixture decode unavailable"));
    };
  }, { src, mode });
}

for (const width of [390, 1280]) {
  test(`at ${width}, an inline image reserves its box and reveals after decode`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await serve(page, "held");
    await page.goto(`/p/${hash}`);
    const image = page.locator(".post .tile-image img");
    await expect.poll(() => page.evaluate(() => (window as DecodeWindow).imageDecodeCalls)).toBeGreaterThan(0);
    await expect(image).toHaveCSS("visibility", "hidden");
    const tile = page.locator(".post .tile-image");
    const before = await tile.boundingBox();
    expect(before?.height).toBeGreaterThan(0);
    expect(await image.getAttribute("src")).toBe(src);
    // Waiting for pixels never blocks a separate UI interaction.
    await page.getByRole("button", { name: "Settings", exact: true }).click();
    await expect(page.getByRole("switch", { name: "Show mini app casts", exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Settings", exact: true }).click();
    await page.evaluate(() => (window as DecodeWindow).releaseImageDecode());
    await expect(image).toHaveCSS("visibility", "visible");
    await expect(image).not.toHaveAttribute("style");
    const after = await tile.boundingBox();
    expect(after?.height).toBeCloseTo(before!.height, 0);
    expect(after?.width).toBeCloseTo(before!.width, 0);
    await tile.click();
    const viewer = page.getByRole("dialog", { name: /Media viewer/ });
    await expect(viewer).toBeVisible();
    expect(await viewer.locator("img").getAttribute("src")).toBe(src);
  });

  test(`at ${width}, a rejected decode still reveals the original image`, async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", error => errors.push(error.message));
    await page.setViewportSize({ width, height: 900 });
    await serve(page, "rejected");
    await page.goto(`/p/${hash}`);
    await expect.poll(() => page.evaluate(() => (window as DecodeWindow).imageDecodeCalls)).toBeGreaterThan(0);
    await expect(page.locator(".post .tile-image img")).toHaveCSS("visibility", "visible");
    expect(errors).toEqual([]);
  });

  test(`at ${width}, decode completion after navigation leaves the new page alone`, async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", error => errors.push(error.message));
    await page.setViewportSize({ width, height: 900 });
    await serve(page, "held");
    await page.goto(`/p/${hash}`);
    await expect.poll(() => page.evaluate(() => (window as DecodeWindow).imageDecodeCalls)).toBeGreaterThan(0);
    await page.getByRole("banner").getByRole("link", { name: "Home", exact: true }).click();
    await expect(page).toHaveURL(/\/$/);
    await expect(page.locator(".post")).toHaveCount(0);
    await page.evaluate(() => (window as DecodeWindow).releaseImageDecode());
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    await expect(page.locator(".post")).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}
