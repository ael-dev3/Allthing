// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Locator, type Page, type Route } from "@playwright/test";
import { readFileSync } from "node:fs";
import path from "node:path";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const fixtures = path.join(__dirname, "fixtures/hls");
const hash = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const cast = (n: number, embed: { kind: string; url: string }) => ({
  hash: hash(n), author: { fid: 1, username: "fixture" }, text: `Media fixture ${n}`, ts: 1792235681 - n, likes: 1, embeds: [embed],
});
const casts = [
  cast(1, { kind: "image", url: "https://media.example/wide.svg" }),
  cast(2, { kind: "image", url: "https://media.example/tall.svg" }),
  cast(3, { kind: "video", url: "https://media.example/wide.m3u8" }),
  cast(4, { kind: "video", url: "https://media.example/tall.m3u8" }),
];
const svg = (w: number, h: number) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}"><rect width="${w}" height="${h}" fill="#4f43d4"/></svg>`;

async function serve(page: Page, delayImages = 0) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home", (route) => route.fulfill({ json: { pool: casts, meta } }));
  await page.route("https://media.example/**", async (route: Route) => {
    const name = new URL(route.request().url()).pathname.slice(1);
    const headers = { "access-control-allow-origin": "*" };
    if (name.endsWith(".svg")) {
      await new Promise((done) => setTimeout(done, delayImages));
      return route.fulfill({ body: name === "wide.svg" ? svg(1200, 600) : svg(600, 1200), contentType: "image/svg+xml", headers });
    }
    // The fixture stream; "tall" claims a portrait resolution.
    const file = name.endsWith(".m3u8") && !name.startsWith("stream_") ? "master.m3u8" : name;
    let body = readFileSync(path.join(fixtures, file));
    if (name === "tall.m3u8") body = Buffer.from(body.toString().replace("RESOLUTION=320x180", "RESOLUTION=180x320"));
    const contentType = file.endsWith(".m3u8") ? "application/vnd.apple.mpegurl" : "video/mp4";
    return route.fulfill({ body, contentType, headers });
  });
}

const box = async (locator: Locator) => (await locator.boundingBox())!;
const card = (page: Page, n: number) => page.locator(`.cast-card[data-hash="${hash(n)}"]`);

test("single images and videos take their own shape, the card's width and at most 70vh, or 80vh when portrait", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  const viewport = page.viewportSize()!;

  // A wide image fills the card's width, with no bars beside it.
  const wide = card(page, 1).locator(".tile-image");
  await wide.scrollIntoViewIfNeeded();
  await expect(wide).toHaveAttribute("data-sized", "");
  const grid = await box(card(page, 1).locator(".media-grid"));
  const wideBox = await box(wide);
  expect(Math.abs(wideBox.width - grid.width)).toBeLessThanOrEqual(1);
  expect(Math.abs(wideBox.height - wideBox.width / 2)).toBeLessThanOrEqual(2);
  const wideImg = await box(wide.locator("img"));
  expect(Math.abs(wideImg.width - wideBox.width)).toBeLessThanOrEqual(1);

  // A tall image stops at 80vh, and its tile is exactly as wide as the image.
  const tall = card(page, 2).locator(".tile-image");
  await tall.scrollIntoViewIfNeeded();
  await expect(tall).toHaveAttribute("data-portrait", "");
  const tallBox = await box(tall);
  const tallImg = await box(tall.locator("img"));
  expect(tallBox.height).toBeLessThanOrEqual(viewport.height * 0.8 + 1);
  expect(Math.abs(tallImg.width - tallBox.width)).toBeLessThanOrEqual(1);
  expect(Math.abs(tallImg.height - tallBox.height)).toBeLessThanOrEqual(1);
  expect(Math.abs(tallBox.width - tallBox.height / 2)).toBeLessThanOrEqual(2);

  // Videos take their stream's shape: a wide one fills the card's width...
  const wideVideo = card(page, 3).locator(".video");
  await wideVideo.scrollIntoViewIfNeeded();
  await expect(wideVideo).toHaveAttribute("data-sized", "");
  const wideVideoBox = await box(wideVideo);
  expect(Math.abs(wideVideoBox.width - grid.width)).toBeLessThanOrEqual(1);
  expect(Math.abs(wideVideoBox.height - (wideVideoBox.width * 180) / 320)).toBeLessThanOrEqual(2);
  // ...and a portrait one stops at 80vh.
  const tallVideo = card(page, 4).locator(".video");
  await tallVideo.scrollIntoViewIfNeeded();
  await expect(tallVideo).toHaveAttribute("data-portrait", "");
  const tallVideoBox = await box(tallVideo);
  expect(tallVideoBox.height).toBeLessThanOrEqual(viewport.height * 0.8 + 1);
  expect(Math.abs(tallVideoBox.width - (tallVideoBox.height * 180) / 320)).toBeLessThanOrEqual(2);
});

test("back to the feed, images take their final box at once", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  for (const n of [1, 2]) {
    const tile = card(page, n).locator(".tile-image");
    await tile.scrollIntoViewIfNeeded();
    await expect(tile).toHaveAttribute("data-sized", "");
  }
  const before = await box(card(page, 2).locator(".tile-image"));
  await card(page, 2).locator("header.cast-meta a.when").click();
  await expect(page).toHaveURL(new RegExp(`/p/${hash(2)}$`));
  // Note, for each image tile the feed adds again, whether it already has
  // its size: only a size known from the first visit can set it that early
  // (an image's own load comes in a later task). A tile is noted once, even
  // when the list and the page around it are inserted in separate steps.
  await page.evaluate(() => {
    const seen = ((window as any).__sizedAtInsert = new Map<Element, boolean>());
    new MutationObserver((records) => {
      for (const node of records.flatMap((r) => [...r.addedNodes])) {
        if (!(node instanceof Element)) continue;
        for (const tile of [node, ...node.querySelectorAll(".tile-image")]) {
          if (tile.matches(".tile-image") && !seen.has(tile)) seen.set(tile, tile.hasAttribute("data-sized"));
        }
      }
    }).observe(document.body, { childList: true, subtree: true });
  });
  await page.goBack();
  await expect(card(page, 2)).toBeVisible();
  expect(await page.evaluate(() => [...(window as any).__sizedAtInsert.values()])).toEqual([true, true]);
  const after = await box(card(page, 2).locator(".tile-image"));
  expect(Math.abs(after.width - before.width)).toBeLessThanOrEqual(1);
  expect(Math.abs(after.height - before.height)).toBeLessThanOrEqual(1);
});
