// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Locator, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import path from "node:path";
import { appCame } from "./helpers/app";

/*
 * GIFs show at about the size Discord gives them, not a photo's: a Tenor
 * GIF, a KLIPY GIF and an image whose path is a .gif each take their own
 * shape, at most 300 px wide and 300 px tall, from the left edge of the
 * card's text, with rounded corners, in a feed card and on the post's page;
 * in a feed no taller than its other media (270 px on a computer). On a
 * phone, at most 70% of the card's width. A Tenor GIF that comes as an MP4
 * is sized the same. A photo in the same feed is as large as a feed's media
 * is. A click (a tap on a phone) on a GIF opens the media viewer at the
 * GIF's own size, in a feed as on its post's page. The GIFs are animated
 * GIFs made here, and the MP4 is the fixture stream's video, so nothing
 * leaves the test.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const author = { fid: 1, username: "fixture", display_name: "Fixture" };
const CAP = 300;
/** How tall a feed's media is at most on a computer (style/allthing.css, --feed-media). */
const FEED_CAP = 270;

type Media = { url: string; width: number; height: number };
const tenor: Media = { url: "https://media.tenor.com/fixtureAAAAC/cat-dance.gif", width: 498, height: 280 };
const klipy: Media = { url: "https://static.klipy.com/ii/fixture/bf/e4/Plhldnjk.gif", width: 320, height: 400 };
const dotGif: Media = { url: "https://i.postimg.cc/fixture/sunset.GIF", width: 640, height: 480 };
const tenorMp4: Media = { url: "https://media.tenor.com/fixtureAAAPo/cat-dance.mp4", width: 320, height: 180 };
const photo: Media = { url: "https://media.example/photo.jpg", width: 1200, height: 800 };

const cast = (n: number, text: string, kind: "image" | "video", media: Media) => ({
  hash: hex(n), author, text, ts: ts - n * 60, likes: 1, recasts: 0, replies: 0, embeds: [{ kind, url: media.url }],
});
const gifs = [
  { cast: cast(1, "A Tenor GIF", "image", tenor), media: tenor },
  { cast: cast(2, "A KLIPY GIF", "image", klipy), media: klipy },
  { cast: cast(3, "An image whose path is a .gif", "image", dotGif), media: dotGif },
];
const photoCast = cast(4, "A photo", "image", photo);
const videoGif = cast(5, "A Tenor GIF that comes as an MP4", "video", tenorMp4);
const casts = [...gifs.map((g) => g.cast), photoCast, videoGif];

/** GIF's LZW, for `pixels` of a palette of four (2-bit codes to start). */
function lzw(pixels: Uint8Array): number[] {
  const clear = 4;
  const end = 5;
  const out: number[] = [];
  let acc = 0;
  let bits = 0;
  // As a decoder reads them: the codes' width, and its next entry.
  let size = 3;
  let avail = end + 1;
  let sent = 0;
  const write = (code: number) => {
    acc |= code << bits;
    bits += size;
    for (; bits >= 8; bits -= 8, acc >>>= 8) out.push(acc & 0xff);
  };
  const put = (code: number) => {
    write(code);
    // Each code after the first adds an entry; codes widen as the entries reach a power of two.
    if (++sent > 1 && avail < 4096 && ++avail === 1 << size && avail < 4096) size += 1;
  };
  write(clear);
  const table = new Map<number, number>();
  let next = end + 1;
  let prefix = pixels[0];
  for (const pixel of pixels.subarray(1)) {
    const known = table.get((prefix << 8) | pixel);
    if (known !== undefined) {
      prefix = known;
      continue;
    }
    put(prefix);
    if (next < 4096) table.set((prefix << 8) | pixel, next++);
    prefix = pixel;
  }
  put(prefix);
  write(end);
  if (bits > 0) out.push(acc & 0xff);
  return out;
}

/** An animated GIF of `width` × `height`: three frames, each a colour with a band of the next. */
function gif({ width, height }: Media): Buffer {
  const u16 = (n: number) => [n & 0xff, n >> 8];
  const palette = [0x4f, 0x43, 0xd4, 0x0a, 0x8f, 0xa7, 0xb9, 0x82, 0x2a, 0xff, 0xff, 0xff];
  // GIF89a, its size, a global palette of four, and "loop forever".
  const bytes = [...Buffer.from("GIF89a"), ...u16(width), ...u16(height), 0xf1, 0, 0, ...palette];
  bytes.push(0x21, 0xff, 0x0b, ...Buffer.from("NETSCAPE2.0"), 3, 1, 0, 0, 0);
  for (let frame = 0; frame < 3; frame++) {
    const pixels = new Uint8Array(width * height).fill(frame);
    pixels.fill((frame + 1) % 3, Math.floor(height * 0.4) * width, Math.floor(height * 0.6) * width);
    const data = lzw(pixels);
    bytes.push(0x21, 0xf9, 4, 4, ...u16(40), 0, 0, 0x2c, 0, 0, 0, 0, ...u16(width), ...u16(height), 0, 2);
    for (let i = 0; i < data.length; i += 255) bytes.push(Math.min(255, data.length - i), ...data.slice(i, i + 255));
    bytes.push(0);
  }
  bytes.push(0x3b);
  return Buffer.from(bytes);
}

/** The fixture stream's video as one MP4 file (VP9, which Playwright's Chromium plays). */
const hls = path.join(__dirname, "fixtures/hls");
const mp4 = Buffer.concat(["init_0.mp4", "seg_0_0.m4s", "seg_0_1.m4s", "seg_0_2.m4s"].map((f) => readFileSync(path.join(hls, f))));
const svg = ({ width, height }: Media) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}"><rect width="${width}" height="${height}" fill="#0a8fa7"/></svg>`;

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : casts, notes: {}, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash")?.toLowerCase();
    const found = casts.find((c) => c.hash === hash);
    return found ? route.fulfill({ json: { thread: { cast: found, children: [] }, notes: {}, meta } }) : route.fulfill({ status: 404, json: { error: "Cast not found" } });
  });
  for (const { media } of gifs) {
    const body = gif(media);
    await page.route(media.url, (route) => route.fulfill({ body, contentType: "image/gif" }));
  }
  await page.route(photo.url, (route) => route.fulfill({ body: svg(photo), contentType: "image/svg+xml" }));
  await page.route(tenorMp4.url, (route) => route.fulfill({ body: mp4, contentType: "video/mp4" }));
}

const box = async (locator: Locator) => (await locator.boundingBox())!;
const card = (page: Page, hash: string) => page.locator(`.cast-card[data-hash="${hash}"]`);

/** The width of what an element's content can take: inside its padding. */
const contentWidth = (locator: Locator) =>
  locator.evaluate((el) => {
    const style = getComputedStyle(el);
    return el.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
  });

/**
 * A GIF (`media`) in `scope`, a card or a post whose text is `text`, shown
 * by `frame`: at its own shape (±2 px), at most `cap` px wide (300 at most)
 * and `tall` px tall (300 at most), and as big as those allow; from the
 * text's left edge, with rounded corners.
 */
async function expectGifSize(scope: Locator, text: string, frame: Locator, media: Media, cap: number, tall = CAP) {
  await frame.scrollIntoViewIfNeeded();
  await expect(frame).toHaveAttribute("data-sized", "");
  const shown = await box(frame);
  const height = Math.min(tall, CAP);
  expect(shown.width).toBeLessThanOrEqual(Math.min(cap, CAP) + 0.5);
  expect(shown.height).toBeLessThanOrEqual(height + 0.5);
  expect(Math.abs(shown.height - (shown.width * media.height) / media.width)).toBeLessThanOrEqual(2);
  // An image also grows to 1.25 times its own width at most, as a photo does.
  const grow = (await frame.evaluate((el) => el.matches(".tile-image"))) ? media.width * 1.25 : Infinity;
  expect(Math.abs(shown.width - Math.min(cap, CAP, (height * media.width) / media.height, grow))).toBeLessThanOrEqual(1);
  expect(Math.abs(shown.x - (await box(scope.locator(text))).x)).toBeLessThanOrEqual(1);
  expect(await frame.evaluate((el) => parseFloat(getComputedStyle(el).borderTopLeftRadius))).toBeGreaterThan(0);
  return shown;
}

const sizes = [
  { name: "1280", phone: false, use: { viewport: { width: 1280, height: 900 } } },
  { name: "390", phone: true, use: { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true } },
];

for (const size of sizes) {
  test.describe(`at ${size.name}`, () => {
    test.use(size.use);

    test("GIFs in feed cards are Discord-sized, at their own shape from the text's left edge; a photo is as large as a feed's media", async ({ page }, info) => {
      const errors: string[] = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await serve(page);
      await page.goto("/");
      await appCame(page);
      const first = card(page, gifs[0].cast.hash);
      await expect(first).toBeVisible();
      const width = await contentWidth(first);
      // On a phone, at most 70% of the card's width; on a computer, no taller than a feed's media.
      const cap = size.phone ? 0.7 * width : CAP;
      const tall = size.phone ? CAP : FEED_CAP;

      for (const { cast: c, media } of gifs) {
        const tile = card(page, c.hash).locator(".media-grid.n1 .tile-image");
        await expect(tile).toHaveClass(/(^| )gif( |$)/);
        const shown = await expectGifSize(card(page, c.hash), ".cast-text", tile, media, cap, tall);
        // The GIF fills its tile, which clips it to its rounded corners.
        const img = await box(tile.locator("img"));
        expect(Math.abs(img.width - shown.width)).toBeLessThanOrEqual(1);
        expect(Math.abs(img.height - shown.height)).toBeLessThanOrEqual(1);
        expect(await tile.evaluate((el) => getComputedStyle(el).overflow)).toBe("hidden");
      }

      // A Tenor GIF that comes as an MP4 is a GIF too.
      const video = card(page, videoGif.hash).locator(".tile-video.gif .video");
      await expectGifSize(card(page, videoGif.hash), ".cast-text", video, tenorMp4, cap, tall);

      // A photo is a feed's media: at its own shape, as wide as the card and
      // a feed's media cap (half the window on a phone) allow.
      const still = card(page, photoCast.hash).locator(".tile-image");
      await still.scrollIntoViewIfNeeded();
      await expect(still).toHaveAttribute("data-sized", "");
      await expect(still).not.toHaveClass(/(^| )gif( |$)/);
      const grid = await box(card(page, photoCast.hash).locator(".media-grid"));
      const photoBox = await box(still);
      const photoCap = size.phone ? page.viewportSize()!.height / 2 : FEED_CAP;
      expect(Math.abs(grid.width - width)).toBeLessThanOrEqual(1);
      expect(Math.abs(photoBox.width - Math.min(grid.width, (photoCap * photo.width) / photo.height))).toBeLessThanOrEqual(1);
      expect(Math.abs(photoBox.height - (photoBox.width * photo.height) / photo.width)).toBeLessThanOrEqual(2);

      await first.scrollIntoViewIfNeeded();
      await first.screenshot({ path: info.outputPath(`gif-${size.name}.png`) });
      expect(errors).toEqual([]);
    });

    test("on its post's page a GIF is Discord-sized too, and a click or a tap opens it in the media viewer at its own size", async ({ page }, info) => {
      await serve(page);
      // In the feed, a click or a tap on a GIF opens it in the media viewer;
      // its time opens its post.
      await page.goto("/");
      await appCame(page);
      const feedTile = card(page, gifs[0].cast.hash).locator(".tile-image.gif");
      await expect(feedTile).toHaveAttribute("data-sized", "");
      if (size.phone) await feedTile.tap();
      else await feedTile.click();
      const feedViewer = page.getByRole("dialog", { name: "Media viewer", exact: true });
      await expect(feedViewer.locator("img.lb-media")).toHaveAttribute("src", gifs[0].media.url);
      await expect(page).toHaveURL(/\/$/);
      if (size.phone) await feedViewer.getByRole("button", { name: "Close media viewer (Escape)", exact: true }).tap();
      else await page.keyboard.press("Escape");
      await expect(feedViewer).toHaveCount(0);
      await card(page, gifs[0].cast.hash).locator("header.cast-meta a.when").click();
      await expect(page).toHaveURL(new RegExp(`/p/${gifs[0].cast.hash}$`));

      for (const [i, { cast: c, media }] of gifs.entries()) {
        if (i > 0) await page.goto(`/p/${c.hash}`);
        const post = page.locator(".post");
        const tile = post.locator(".media-grid.n1 .tile-image");
        await expect(tile).toHaveClass(/(^| )gif( |$)/);
        const width = await contentWidth(post);
        const shown = await expectGifSize(post, ".post-text", tile, media, size.phone ? 0.7 * width : CAP);
        if (i === 0) await page.screenshot({ path: info.outputPath(`gif-post-${size.name}.png`) });

        // The media viewer, at the GIF's own size (or the screen's).
        if (size.phone) await tile.tap();
        else await tile.click();
        const viewer = page.getByRole("dialog", { name: "Media viewer", exact: true });
        await expect(viewer).toBeVisible();
        const full = viewer.locator("img.lb-media");
        await expect(full).toHaveAttribute("src", media.url);
        await expect.poll(() => full.evaluate((img) => (img as HTMLImageElement).naturalWidth)).toBe(media.width);
        // Measured once it has come in (it grows from 97%).
        await viewer.locator(".lb-stage").evaluate((stage) => Promise.all(stage.getAnimations().map((a) => a.finished)));
        const viewport = page.viewportSize()!;
        const fit = Math.min(1, (viewport.width * 0.94) / media.width, (viewport.height - 120) / media.height);
        const large = await box(full);
        expect(Math.abs(large.width - media.width * fit)).toBeLessThanOrEqual(1);
        expect(Math.abs(large.height - media.height * fit)).toBeLessThanOrEqual(1);
        expect(large.width).toBeGreaterThan(shown.width);
        if (size.phone) await viewer.getByRole("button", { name: "Close media viewer (Escape)", exact: true }).tap();
        else await page.keyboard.press("Escape");
        await expect(viewer).toHaveCount(0);
      }
    });
  });
}

test("back to the feed, GIFs take their final box at once", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  await appCame(page);
  const before = new Map<string, { width: number; height: number }>();
  for (const { cast: c } of gifs) {
    const tile = card(page, c.hash).locator(".tile-image.gif");
    await tile.scrollIntoViewIfNeeded();
    await expect(tile).toHaveAttribute("data-sized", "");
    before.set(c.hash, await box(tile));
  }
  await card(page, gifs[2].cast.hash).locator("header.cast-meta a.when").click();
  await expect(page).toHaveURL(new RegExp(`/p/${gifs[2].cast.hash}$`));
  // Note, for each GIF tile the feed adds again, whether it has its size as
  // it comes: only the size known from the first visit can give it that.
  await page.evaluate(() => {
    const seen = ((window as any).__sizedAtInsert = new Map<Element, boolean>());
    new MutationObserver((records) => {
      for (const node of records.flatMap((r) => [...r.addedNodes])) {
        if (!(node instanceof Element)) continue;
        for (const tile of [node, ...node.querySelectorAll(".tile-image.gif")]) {
          if (tile.matches(".cast-card .tile-image.gif") && !seen.has(tile)) seen.set(tile, tile.hasAttribute("data-sized"));
        }
      }
    }).observe(document.body, { childList: true, subtree: true });
  });
  await page.goBack();
  await expect(card(page, gifs[2].cast.hash)).toBeVisible();
  expect(await page.evaluate(() => [...(window as any).__sizedAtInsert.values()])).toEqual([true, true, true]);
  for (const { cast: c } of gifs) {
    const after = await box(card(page, c.hash).locator(".tile-image.gif"));
    expect(Math.abs(after.width - before.get(c.hash)!.width)).toBeLessThanOrEqual(1);
    expect(Math.abs(after.height - before.get(c.hash)!.height)).toBeLessThanOrEqual(1);
  }
});
