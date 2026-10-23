// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Locator, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import path from "node:path";
import { appCame } from "./helpers/app";
import { NOW } from "./fixture-time";

/*
 * A list of casts is denser, as in the Farcaster app: in a feed's cards and
 * the casts they quote, media is at most 270 px tall on a computer and half
 * the window on a phone, each image and video in its own shape (a tall image
 * is shown whole, narrower, not cropped), and a cast's images are one row of
 * squares on a computer. A click or a tap on an image opens it in the media
 * viewer at full size, where it used to open the post; a video keeps its own
 * controls, and its button opens it there. A post's page keeps its larger
 * media, for the cast it opens and for its replies. Here: a tall, a wide and
 * a square image, four images, a video and a quote with an image.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = (NOW - 19);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const author = { fid: 7, username: "lens", display_name: "Lens" };

/** Each fixture image's own size. */
const SIZES: Record<string, [number, number]> = {
  tall: [600, 1200],
  wide: [1200, 600],
  square: [800, 800],
  "four-1": [800, 600],
  "four-2": [600, 800],
  "four-3": [800, 800],
  "four-4": [1000, 500],
  quoted: [600, 1200],
  reply: [600, 1200],
};
const image = (name: string) => ({ kind: "image", url: `https://media.example/${name}.svg` });
const cast = (n: number, text: string, embeds: object[]) => ({ hash: hex(n), author, text, ts: ts - n * 60, likes: 100 - n, recasts: 0, replies: 0, embeds });

const tall = cast(1, "A tall photo", [image("tall")]);
const wide = cast(2, "A wide photo", [image("wide")]);
const square = cast(3, "A square photo", [image("square")]);
const four = cast(4, "Four photos", ["four-1", "four-2", "four-3", "four-4"].map(image));
const video = cast(5, "A video", [{ kind: "video", url: "https://media.example/wide.m3u8" }]);
const quoted = { ...cast(9, "The quoted photo", [image("quoted")]), author: { fid: 8, username: "quoted", display_name: "Quoted" } };
const quoting = cast(6, "Quotes a photo", [{ kind: "cast", fid: 8, hash: quoted.hash }]);
const pool = [tall, wide, square, four, video, quoting];
const reply = { ...cast(10, "A reply with a photo", [image("reply")]), parent_hash: tall.hash };

const svg = ([w, h]: [number, number]) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}"><rect width="${w}" height="${h}" fill="#4f43d4"/></svg>`;
const hls = path.join(__dirname, "fixtures/hls");

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : pool, notes: { quotes: { [quoted.hash]: quoted } }, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash")?.toLowerCase();
    const found = [...pool, quoted].find((c) => c.hash === hash);
    if (!found) return route.fulfill({ status: 404, json: { error: "Cast not found" } });
    const children = found === tall ? [{ cast: reply, children: [] }] : [];
    const quotes = found === quoting ? [{ hash: quoted.hash, cast: quoted }] : [];
    return route.fulfill({ json: { thread: { cast: found, children }, quotes, notes: {}, meta } });
  });
  await page.route("https://media.example/**", (route) => {
    const name = new URL(route.request().url()).pathname.slice(1);
    const headers = { "access-control-allow-origin": "*" };
    if (name.endsWith(".svg")) return route.fulfill({ body: svg(SIZES[name.replace(/\.svg$/, "")]), contentType: "image/svg+xml", headers });
    // The fixture stream, 320 × 180.
    const file = name.endsWith(".m3u8") && !name.startsWith("stream_") ? "master.m3u8" : name;
    const contentType = file.endsWith(".m3u8") ? "application/vnd.apple.mpegurl" : "video/mp4";
    return route.fulfill({ body: readFileSync(path.join(hls, file)), contentType, headers });
  });
}

const box = async (locator: Locator) => (await locator.boundingBox())!;
const card = (page: Page, c: { hash: string }) => page.locator(`.cast-card[data-hash="${c.hash}"]`);

/** Once `tile` has its image's size, its box. */
async function sized(tile: Locator) {
  await tile.scrollIntoViewIfNeeded();
  await expect(tile).toHaveAttribute("data-sized", "");
  return box(tile);
}

/** The width of what an element's content can take: inside its padding. */
const contentWidth = (locator: Locator) =>
  locator.evaluate((el) => {
    const style = getComputedStyle(el);
    return el.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
  });

/** `shown` is `[w, h]`'s shape (±2 px). */
function expectShape(shown: { width: number; height: number }, [w, h]: [number, number]) {
  expect(Math.abs(shown.height - (shown.width * h) / w)).toBeLessThanOrEqual(2);
}

const sizes = [
  { name: "1280", phone: false, use: { viewport: { width: 1280, height: 900 } } },
  { name: "390", phone: true, use: { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true } },
];

for (const size of sizes) {
  test.describe(`at ${size.name}`, () => {
    test.use(size.use);
    /** A list's media cap: 270 px, or half the window on a phone; a quote's, 320 px at most. */
    const feedCap = (page: Page) => (size.phone ? page.viewportSize()!.height / 2 : 270);
    const press = (target: Locator) => (size.phone ? target.tap() : target.click());

    test("in a feed, each cast's media is at most the list's cap tall, in its own shape", async ({ page }, info) => {
      await serve(page);
      await page.goto("/");
      await appCame(page);
      const cap = feedCap(page);
      const width = await contentWidth(card(page, tall));

      // Single images: as large as the card's width and the cap allow, at their own shape.
      for (const [c, name] of [[tall, "tall"], [wide, "wide"], [square, "square"]] as const) {
        const tile = card(page, c).locator(".media-grid.n1 .tile-image");
        const shown = await sized(tile);
        const [w, h] = SIZES[name];
        expect(shown.height, name).toBeLessThanOrEqual(cap + 1);
        expectShape(shown, [w, h]);
        expect(Math.abs(shown.width - Math.min(width, (cap * w) / h, w * 1.25)), name).toBeLessThanOrEqual(1);
        // The image fills its tile: the tall one is whole, not cropped.
        const img = await box(tile.locator("img"));
        expect(Math.abs(img.width - shown.width)).toBeLessThanOrEqual(1);
        expect(Math.abs(img.height - shown.height)).toBeLessThanOrEqual(1);
      }

      // Four images: squares, on a computer in one row, on a phone two by two; no taller than the cap.
      const grid = card(page, four).locator(".media-grid.n4");
      await grid.scrollIntoViewIfNeeded();
      const tiles = grid.locator(".tile-image");
      await expect(tiles).toHaveCount(4);
      const boxes = await Promise.all([0, 1, 2, 3].map((i) => box(tiles.nth(i))));
      for (const b of boxes) expect(Math.abs(b.width - b.height)).toBeLessThanOrEqual(1);
      const rows = new Set(boxes.map((b) => Math.round(b.y)));
      expect(rows.size).toBe(size.phone ? 2 : 1);
      expect((await box(grid)).height).toBeLessThanOrEqual(cap + 1);

      // A video takes its stream's shape (320 × 180) under the same cap.
      const player = card(page, video).locator(".video");
      const played = await sized(player);
      expect(played.height).toBeLessThanOrEqual(cap + 1);
      expectShape(played, [320, 180]);
      expect(Math.abs(played.width - Math.min(width, (cap * 320) / 180))).toBeLessThanOrEqual(1);
      await expect(player.locator(".video-play")).toBeAttached();

      // A quoted cast's image: no taller than the cap, or 320 px.
      const inQuote = card(page, quoting).locator(".quote .media-grid.n1 .tile-image");
      const quotedBox = await sized(inQuote);
      expect(quotedBox.height).toBeLessThanOrEqual(Math.min(cap, 320) + 1);
      expect(Math.abs(quotedBox.height - Math.min(cap, 320))).toBeLessThanOrEqual(1);
      expectShape(quotedBox, SIZES.quoted);

      await page.evaluate(() => scrollTo(0, 0));
      await page.screenshot({ path: info.outputPath(`feed-media-${size.name}.png`) });
    });

    test("on its post's page, the opened cast's media and its replies' keep their larger size", async ({ page }, info) => {
      await serve(page);
      await page.goto("/");
      await appCame(page);
      const cap = feedCap(page);
      const inFeed = new Map<string, { width: number; height: number }>();
      inFeed.set("tall", await sized(card(page, tall).locator(".tile-image")));
      inFeed.set("video", await sized(card(page, video).locator(".video")));
      inFeed.set("quoted", await sized(card(page, quoting).locator(".quote .tile-image")));
      inFeed.set("four", await box(card(page, four).locator(".media-grid")));
      const viewport = page.viewportSize()!;

      // The tall photo: up to 80% of the window tall, as before, and its
      // reply's tall photo too, larger than a list's.
      await page.goto(`/p/${tall.hash}`);
      const shown = await sized(page.locator(".post .media-grid.n1 .tile-image"));
      expectShape(shown, SIZES.tall);
      expect(shown.height).toBeGreaterThan(inFeed.get("tall")!.height + 100);
      expect(shown.height).toBeLessThanOrEqual(viewport.height * 0.8 + 1);
      const replied = await sized(page.locator(".comment .media-grid.n1 .tile-image"));
      expectShape(replied, SIZES.reply);
      expect(replied.height).toBeGreaterThan(cap + 100);
      expect(replied.height).toBeLessThanOrEqual(viewport.height * 0.8 + 1);
      await page.evaluate(() => scrollTo(0, 0));
      await page.screenshot({ path: info.outputPath(`post-media-${size.name}.png`) });

      // Four photos: the post's grid, two by two across the post (on a
      // computer, taller than a list's row).
      await page.goto(`/p/${four.hash}`);
      const grid = page.locator(".post .media-grid.n4");
      await expect(grid.locator(".tile-image")).toHaveCount(4);
      const posted = await box(grid);
      expect(Math.abs(posted.width - (await contentWidth(page.locator(".post"))))).toBeLessThanOrEqual(1);
      expect(Math.abs(posted.height - posted.width)).toBeLessThanOrEqual(2);
      if (!size.phone) expect(posted.height).toBeGreaterThan(inFeed.get("four")!.height + 100);

      // The video, across the post (on a computer, wider than a list's).
      await page.goto(`/p/${video.hash}`);
      const big = await sized(page.locator(".post .video"));
      expectShape(big, [320, 180]);
      expect(Math.abs(big.width - (await contentWidth(page.locator(".post"))))).toBeLessThanOrEqual(1);
      if (!size.phone) expect(big.width).toBeGreaterThan(inFeed.get("video")!.width + 100);

      // The quote's photo, as large as the post's own.
      await page.goto(`/p/${quoting.hash}`);
      const quotedBig = await sized(page.locator(".post .quote .tile-image"));
      expectShape(quotedBig, SIZES.quoted);
      expect(quotedBig.height).toBeGreaterThan(inFeed.get("quoted")!.height + 100);
    });

    test("a click or a tap on media in the feed opens it in the media viewer at full size, not the post", async ({ page }) => {
      const errors: string[] = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await serve(page);
      await page.goto("/");
      await appCame(page);
      const viewer = page.getByRole("dialog", { name: "Media viewer", exact: true });
      const close = async () => {
        if (size.phone) await viewer.getByRole("button", { name: "Close media viewer (Escape)", exact: true }).tap();
        else await page.keyboard.press("Escape");
        await expect(viewer).toHaveCount(0);
      };

      // The tall photo, at its own size (or the screen's).
      const tile = card(page, tall).locator(".tile-image");
      const small = await sized(tile);
      await expect(tile).toHaveAccessibleName(/^Open Image posted by @lens: A tall photo in the media viewer$/);
      await press(tile);
      await expect(viewer).toBeVisible();
      const full = viewer.locator("img.lb-media");
      await expect(full).toHaveAttribute("src", "https://media.example/tall.svg");
      await viewer.locator(".lb-stage").evaluate((stage) => Promise.all(stage.getAnimations().map((a) => a.finished)));
      expect((await box(full)).height).toBeGreaterThan(small.height + 100);
      await expect(page).toHaveURL(/\/$/);
      await close();

      // The third of four photos: the viewer steps through all four.
      await card(page, four).scrollIntoViewIfNeeded();
      await press(card(page, four).locator(".tile-image").nth(2));
      await expect(viewer.locator("img.lb-media")).toHaveAttribute("src", "https://media.example/four-3.svg");
      await expect(viewer.locator(".lb-counter")).toHaveText("3 of 4");
      await expect(page).toHaveURL(/\/$/);
      await close();

      // The quoted cast's photo opens there too, not the quoted cast.
      const inQuote = card(page, quoting).locator(".quote .tile-image");
      await sized(inQuote);
      await press(inQuote);
      await expect(viewer.locator("img.lb-media")).toHaveAttribute("src", "https://media.example/quoted.svg");
      await expect(page).toHaveURL(/\/$/);
      await close();

      // A video keeps its own controls in the card, and its button opens it large.
      const player = card(page, video).locator(".video");
      const inCard = await sized(player);
      await expect(player.locator(".video-play")).toBeAttached();
      await press(player.getByRole("button", { name: "Open in the media viewer", exact: true }));
      await expect(viewer.locator(".lb-video .video")).toBeVisible();
      expect((await box(viewer.locator(".lb-video .video"))).width).toBeGreaterThan(inCard.width);
      await expect(page).toHaveURL(/\/$/);
      await close();
      expect(errors).toEqual([]);
    });
  });
}
