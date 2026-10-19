// SPDX-License-Identifier: AGPL-3.0-or-later
import { readFileSync } from "node:fs";
import path from "node:path";
import { expect, test, type Locator, type Page, type Route } from "@playwright/test";

/** Waits until the images on screen have loaded, so nothing shifts under the pointer. */
async function settleImages(page: Page) {
  await page.evaluate(async () => {
    const pending = [...document.images].filter((img) => !img.complete);
    await Promise.race([
      Promise.all(
        pending.map(
          (img) =>
            new Promise((done) => {
              img.addEventListener("load", done, { once: true });
              img.addEventListener("error", done, { once: true });
            }),
        ),
      ),
      new Promise((done) => setTimeout(done, 4000)),
    ]);
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  });
}

/** A popover panel lies wholly inside the viewport, below the sticky header. */
async function expectClearOfHeader(page: Page, panel: Locator) {
  const header = await page.locator(".site-header").boundingBox();
  const box = await panel.boundingBox();
  const viewport = page.viewportSize();
  expect(box && header && viewport).toBeTruthy();
  expect(box!.y).toBeGreaterThanOrEqual(header!.y + header!.height - 1);
  expect(box!.y + box!.height).toBeLessThanOrEqual(viewport!.height + 1);
  expect(box!.x).toBeGreaterThanOrEqual(0);
  expect(box!.x + box!.width).toBeLessThanOrEqual(viewport!.width + 1);
  // Nothing else is painted over it: the topmost element across its
  // middle and near its bottom belongs to the panel.
  const covered = await panel.evaluate((el) => {
    const r = el.getBoundingClientRect();
    return [0.5, 0.9].some((f) => {
      const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height * f);
      return !hit || !el.contains(hit);
    });
  });
  expect(covered).toBe(false);
}

/*
 * Allthing end to end, against live Farcaster data (Hypersnap), either through
 * the Worker or, with E2E_PAGES=1, as the static GitHub Pages copy that reads
 * Hypersnap from the browser. Screenshots land in docs/screenshots (override
 * with E2E_SCREENSHOT_DIR).
 */

const SHOTS = process.env.E2E_SCREENSHOT_DIR ?? "docs/screenshots";
const LIVE = Boolean(process.env.E2E_BASE_URL);
const PAGES = Boolean(process.env.E2E_PAGES);
// The static copy is served under a sub-path, like https://ael-dev3.github.io/Allthing/.
const BASE_PATH = process.env.E2E_BASE_PATH ?? (PAGES ? "/Allthing" : "");
const at = (path: string) => BASE_PATH + path;

// Known casts. Casts are immutable, so these stay valid unless deleted.
const VIDEO_CAST = "0x3b6c0db881a351984ab7844cd56265d492431d79"; // @0xael.eth, Hyperion prologue (HLS video)
const YOUTUBE_CAST = "0x4717fecaeb0d48aedfb30bf1a2b2436e70d4b80c"; // @0xael.eth, a youtu.be link
const GALLERY_CAST = "0x4cc5b1c6f925c928694fef95643cd3bb24803b14"; // two images in /itookaphoto
const NESTED_THREAD = "0xf454b8d3e5ee2c9252a0b7574ad4942e9ba502d2"; // @balajis.eth, replies four levels deep

const firstCard = (page: Page) => page.locator(".cast-card:not(.skeleton)").first();
/** Cards whose text has no links, so a click or drag on the text lands on plain text. */
const textCards = (page: Page) => page.locator(".cast-card:not(.skeleton):has(.cast-text):not(:has(.cast-text a))");

/** Data requests made during `action`: Worker API calls, or Hypersnap reads in the static copy. */
async function apiRequests(page: Page, action: () => Promise<void>): Promise<string[]> {
  const seen: string[] = [];
  const listener = (request: { url(): string; method(): string }) => {
    const url = new URL(request.url());
    if (url.pathname.startsWith("/api/") || url.hostname === "haatz.quilibrium.com") seen.push(`${request.method()} ${request.url()}`);
  };
  page.on("request", listener);
  await action();
  page.off("request", listener);
  return seen;
}

/** Screenshot the viewport once the images in view have loaded (or after 10 s). */
async function shoot(page: Page, name: string) {
  await page
    .waitForFunction(() => Array.from(document.images).every((img) => {
      const box = img.getBoundingClientRect();
      return img.complete || box.bottom < 0 || box.top > innerHeight || box.width === 0;
    }), undefined, { timeout: 10_000 })
    .catch(() => {});
  await page.screenshot({ path: `${SHOTS}/${name}.png`, animations: "disabled" });
}

/** Serve the VP9/Opus fixture in place of every Farcaster HLS stream. */
async function useFixtureStream(page: Page) {
  const dir = path.join(__dirname, "fixtures/hls");
  await page.route("https://stream.farcaster.xyz/v1/video/**", async (route: Route) => {
    const name = new URL(route.request().url()).pathname.split("/").pop() ?? "";
    const file = name.endsWith(".m3u8") && !name.startsWith("stream_") ? "master.m3u8" : name;
    const type = file.endsWith(".m3u8") ? "application/vnd.apple.mpegurl" : "video/mp4";
    await route.fulfill({ body: readFileSync(path.join(dir, file)), contentType: type, headers: { "access-control-allow-origin": "*" } });
  });
}

test.describe("front page", () => {
  test("loads trending casts fast, with the wordmark and honest engagement", async ({ page }) => {
    const started = Date.now();
    await page.goto(at("/"));
    await expect(firstCard(page)).toBeVisible();
    const elapsed = Date.now() - started;
    // The static copy builds the page in the browser (several upstream round trips, no edge cache).
    expect(elapsed, `first casts after ${elapsed} ms`).toBeLessThan(PAGES ? 6000 : LIVE ? 3000 : 5000);

    // Branding: the wordmark only, no logo mark.
    await expect(page.locator("header .wordmark")).toHaveText("Allthing");
    await expect(page.locator(".site-header img, .wordmark svg, .logo-mark")).toHaveCount(0);

    // Likes, recasts and replies as three labeled counts; nothing to vote with.
    const card = firstCard(page);
    await expect(card.locator(".stat-likes .n")).toHaveText(/^\d[\d,.km]*$/);
    await expect(card.locator(".stat-recasts")).toContainText(/recasts?/);
    await expect(card.locator(".stat-replies")).toContainText(/repl(y|ies)/);
    await expect(page.locator('[aria-label*="downvote" i], .downvote, .arrow.down')).toHaveCount(0);
    await expect(page.getByRole("button", { name: /vote/i })).toHaveCount(0);

    // Who, when, where, and via which app; no scores beside names.
    await expect(card.locator(".user-chip .avatar").first()).toBeVisible();
    await expect(card.locator("header.cast-meta time")).toHaveAttribute("datetime", /^\d{4}-\d\d-\d\dT/);
    await expect(page.locator(".cast-card .origin").first()).toContainText("via ");
    await expect(page.locator(".score")).toHaveCount(0);

    // 0.3: no backend badges, intro, ranking or keyboard boxes; a Home button,
    // and a one-line footer with the version.
    await expect(page.locator(".backend")).toHaveCount(0);
    await expect(page.locator(".main-nav a")).toHaveText(["Home"]);
    // No sidebar: the feed is one centered column.
    await expect(page.locator(".side")).toHaveCount(0);
    const feed = await page.locator(".columns .content").boundingBox();
    const viewport = page.viewportSize()!;
    expect(Math.abs(feed!.x + feed!.width / 2 - viewport.width / 2)).toBeLessThan(viewport.width > 900 ? 24 : 40);
    await expect(page.getByText("Front page")).toHaveCount(0);
    // No "Updated N min ago" anywhere.
    await expect(page.getByText(/Updated (just now|\d+ (min|h) ago)/)).toHaveCount(0);
    // The page's own chrome, not casts (whose authors may well write "Keyboard").
    const chrome = page.locator(".site-header, .listing-head, .site-footer, .side, .intro");
    await expect(chrome.getByText(/How ranking works|A quiet, read-only window|Keyboard/)).toHaveCount(0);
    await expect(page.locator(".site-footer")).toContainText(/AGPL-3\.0\s*·\s*Source\s*·\s*v\d+\.\d+\.\d+ \(\w+\)/);
    // Mini apps and frames are filtered out.
    await expect(page.locator(".cast-card a[href*='/miniapps/'], .cast-card a[href*='neynar.app']")).toHaveCount(0);

    // Current data: the newest cast in the pool is under two days old.
    const newest = await page.locator(".cast-card").evaluateAll((cards) =>
      Math.max(...cards.map((c) => Number(c.getAttribute("data-ts")))),
    );
    expect(Date.now() / 1000 - newest).toBeLessThan(2 * 86_400);
    await settleImages(page);
    await shoot(page, "home");
  });

  test("new and top re-rank the same pool without another API call", async ({ page }) => {
    await page.goto(at("/"));
    await expect(firstCard(page)).toBeVisible();
    const calls = await apiRequests(page, async () => {
      await page.locator(".sort-tabs a", { hasText: "new" }).click();
      await expect(page).toHaveURL(/sort=new/);
      await expect(firstCard(page)).toBeVisible();
    });
    // Prefetches of posts may run meanwhile; the feed itself is not refetched.
    expect(calls.filter((c) => /\/api\/v1\/home|feed\/trending/.test(c)), "sorting is local").toHaveLength(0);
    const times = await page.locator(".cast-card").evaluateAll((c) => c.map((e) => Number(e.getAttribute("data-ts"))));
    expect(times).toEqual([...times].sort((a, b) => b - a));

    await page.locator(".sort-tabs a", { hasText: "top" }).click();
    await expect(page).toHaveURL(/sort=top/);
    const points = await page.locator(".cast-card").evaluateAll((c) => c.map((e) => Number(e.getAttribute("data-points"))));
    expect(points).toEqual([...points].sort((a, b) => b - a));
  });

  test("a feed page that fails to load is tried again", async ({ page }) => {
    // The second page fails once, as a Worker error would.
    let failed = 0;
    await page.route(/\/api\/v1\/home\?offset=100$/, async (route: Route) => {
      if (failed++ === 0) return route.fulfill({ status: 500, json: { error: "upstream failed" } });
      await route.fallback();
    });
    await page.goto(at("/"));
    await expect(firstCard(page)).toBeVisible();
    await expect
      .poll(
        async () => {
          await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
          return page.locator(".cast-card").count();
        },
        { timeout: 30_000, intervals: [600] },
      )
      .toBeGreaterThan(100);
    expect(failed).toBeGreaterThanOrEqual(2);
  });

  test("each page view costs one API call", async ({ page }) => {
    test.skip(PAGES, "the static copy has no API: the browser reads Hypersnap itself");
    const calls = await apiRequests(page, async () => {
      await page.goto(at("/c/farcaster"));
      await expect(firstCard(page)).toBeVisible();
    });
    expect(calls.filter((c) => c.includes("/api/v1/channel"))).toHaveLength(1);
    expect(calls.every((c) => c.startsWith("GET "))).toBe(true);
  });
});

test.describe("media", () => {
  test("images open an accessible lightbox (and no longer enlarge on hover)", async ({ page }) => {
    await page.goto(at(`/p/${GALLERY_CAST}`));
    const tiles = page.locator(".post .tile-image");
    await expect(tiles).toHaveCount(2);
    await tiles.first().hover();
    await page.waitForTimeout(400);
    await expect(page.locator(".hover-preview")).toHaveCount(0);

    // Lightbox: dialog, focus inside, alt text, keyboard and swipe navigation.
    await tiles.first().click();
    const dialog = page.getByRole("dialog", { name: "Media viewer" });
    await expect(dialog).toBeVisible();
    await expect(page.getByRole("button", { name: /Close media viewer/ })).toBeFocused();
    const image = dialog.locator("img.lb-media");
    await expect(image).toHaveAttribute("alt", /^Image 1 of 2 posted by @/);
    await expect(dialog.locator(".lb-alt")).toContainText("Image 1 of 2");
    await expect(dialog.locator(".lb-counter")).toHaveText("1 of 2");
    await expect.poll(() => image.evaluate((img: HTMLImageElement) => img.complete && img.naturalWidth > 0)).toBe(true);
    await shoot(page, "post-lightbox");

    await page.keyboard.press("ArrowRight");
    await expect(dialog.locator(".lb-counter")).toHaveText("2 of 2");
    await page.keyboard.press("ArrowLeft");
    await expect(dialog.locator(".lb-counter")).toHaveText("1 of 2");

    // Swipe left with a pointer drag.
    const box = (await dialog.locator(".lb-stage").boundingBox())!;
    const y = box.y + box.height / 2;
    await page.mouse.move(box.x + box.width * 0.7, y);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * 0.2, y, { steps: 6 });
    await page.mouse.up();
    await expect(dialog.locator(".lb-counter")).toHaveText("2 of 2");

    // Tab stays inside, Escape closes and returns focus to the image.
    for (let i = 0; i < 5; i++) await page.keyboard.press("Tab");
    expect(await page.evaluate(() => Boolean(document.activeElement?.closest(".lightbox")))).toBe(true);
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    await expect(tiles.first()).toBeFocused();
  });

  test("a Farcaster video by @0xael.eth plays inline, not as a link", async ({ page }) => {
    await page.goto(at(`/p/${VIDEO_CAST}`));
    const video = page.locator(".post .video video");
    await expect(video).toBeVisible();
    // The poster comes from the stream host itself, never through our Worker.
    await expect(video).toHaveAttribute("poster", /cloudflarestream\.com\/.+\/thumbnails\/thumbnail\.jpg/);
    await expect.poll(() => page.evaluate(() => {
      const src = document.querySelector<HTMLVideoElement>(".post .video video")?.poster;
      return new Promise<boolean>((resolve) => {
        const img = new Image();
        img.onload = () => resolve(img.naturalWidth > 0);
        img.onerror = () => resolve(false);
        img.src = src ?? "";
      });
    })).toBe(true);
    await expect(page.locator(".post").getByRole("button", { name: /^Play video:/ })).toBeVisible();
    // No raw playlist links or grey placeholders.
    await expect(page.locator(".post a[href*='.m3u8']")).toHaveCount(0);
    await expect(page.locator(".post")).not.toContainText(".m3u8");
    await shoot(page, "video-post");
  });

  test("the Rust HLS player streams segments: autoplays muted in view, click plays with controls", async ({ page }) => {
    await useFixtureStream(page);
    await page.goto(at(`/p/${VIDEO_CAST}`));
    const player = page.locator(".post .video").first();
    const video = player.locator("video");
    await expect(player).toBeVisible();
    await expect.poll(() => player.getAttribute("class")).not.toMatch(/loading/);

    // Muted and inline in the markup, as iOS Safari requires for autoplay.
    await expect(video).toHaveAttribute("muted", "");
    await expect(video).toHaveAttribute("playsinline", "");
    // In view: plays muted without a click.
    await player.scrollIntoViewIfNeeded();
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.muted && !v.paused)).toBe(true);
    // While it plays, no play icon is drawn over it; a "sound off" badge is.
    await expect(player).toHaveClass(/autoplaying/);
    await expect(player.locator(".video-play svg")).toBeHidden();
    await expect(player.locator(".video-sound")).toBeVisible();
    await expect(player.getByRole("button", { name: /^Play with sound:/ })).toBeVisible();
    await shoot(page, "video-autoplaying");
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.currentTime), { timeout: 15_000 }).toBeGreaterThan(0.3);
    expect(await video.evaluate((v: HTMLVideoElement) => v.src)).toMatch(/^blob:/);
    // Off screen: paused.
    await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
    await page.evaluate(() => document.body.style.minHeight = "4000px");
    await page.evaluate(() => window.scrollTo(0, 3500));
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.paused)).toBe(true);
    // Paused: the play icon is back.
    await expect(player).not.toHaveClass(/autoplaying/);
    await expect(player.locator(".video-play svg")).toBeVisible();
    await page.evaluate(() => window.scrollTo(0, 0));

    await player.getByRole("button", { name: /^Play (video|with sound):/ }).click();
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.controls && !v.muted)).toBe(true);
    const playedTo = await video.evaluate((v: HTMLVideoElement) => v.currentTime);
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.currentTime), { timeout: 15_000 }).toBeGreaterThan(playedTo + 0.5);
    expect(await video.getAttribute("data-error")).toBeNull();

    // Seeking past the buffer fetches the right segment.
    await video.evaluate((v: HTMLVideoElement) => { v.currentTime = 4.5; });
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.readyState >= 3 && v.currentTime >= 4.5), { timeout: 15_000 }).toBe(true);
  });

  test("a click on a video while its playlist loads plays it once loaded", async ({ page }) => {
    await useFixtureStream(page);
    // Hold the playlist until the reader has clicked.
    let release!: () => void;
    const clicked = new Promise<void>((done) => (release = done));
    await page.route("https://stream.farcaster.xyz/v1/video/**", async (route: Route) => {
      if (route.request().url().endsWith(".m3u8")) await clicked;
      await route.fallback();
    });
    try {
      await page.goto(at(`/p/${VIDEO_CAST}`));
      const player = page.locator(".post .video").first();
      await expect(player).toHaveClass(/\bloading\b/);
      await player.locator(".video-play").click();
      release();
      const video = player.locator("video");
      await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.currentTime), { timeout: 15_000 }).toBeGreaterThan(0.1);
      // With sound and controls, as a click asks, not a muted autoplay.
      await expect(video).toHaveJSProperty("muted", false);
      await expect(video).toHaveJSProperty("controls", true);
    } finally {
      release();
    }
  });

  test("a video the reader started keeps playing when another one could autoplay", async ({ page }) => {
    // Tall enough that the post's video and a reply's video are both in view.
    await page.setViewportSize({ width: 1280, height: 2000 });
    await useFixtureStream(page);
    // The post gets a reply with a second video, whose playlist arrives only
    // once the reader's video is playing.
    const second = "https://stream.farcaster.xyz/v1/video/second-video.m3u8";
    await page.route(/\/api\/v1\/cast\?/, async (route: Route) => {
      const payload = await (await route.fetch()).json();
      const cast = payload.thread.cast;
      payload.thread.children.unshift({
        cast: { ...cast, hash: `0x${"2".repeat(40)}`, text: "A reply with a video", embeds: [{ kind: "video", url: second }], replies: 0 },
        children: [],
      });
      await route.fulfill({ json: payload });
    });
    let release!: () => void;
    const started = new Promise<void>((done) => (release = done));
    await page.route(second, async (route: Route) => {
      await started;
      await route.fallback();
    });
    try {
      await page.goto(at(`/p/${VIDEO_CAST}`));
      const mine = page.locator(".post .video").first().locator("video");
      const reply = page.locator(".video").nth(1).locator("video");
      await expect(reply).toBeAttached();
      await page.locator(".post .video").first().locator(".video-play").click();
      await expect.poll(() => mine.evaluate((v: HTMLVideoElement) => v.currentTime), { timeout: 15_000 }).toBeGreaterThan(0.1);
      release();
      // The reply's playlist arrives and it could autoplay; give it time.
      await page.waitForTimeout(2500);
      expect(await mine.evaluate((v: HTMLVideoElement) => !v.paused && !v.muted), "the reader's video still playing with sound").toBe(true);
      expect(await reply.evaluate((v: HTMLVideoElement) => v.paused), "the reply's video did not take over").toBe(true);
    } finally {
      release();
    }
  });

  test("a video scrolling into view autoplays without flashing the play icon", async ({ page }) => {
    await useFixtureStream(page);
    // The playlist takes a moment, as over a real network...
    await page.route("https://stream.farcaster.xyz/v1/video/**", async (route: Route) => {
      await new Promise((done) => setTimeout(done, 400));
      await route.fallback();
    });
    // ...and the video starts far below the fold, so it loads as it arrives.
    await page.addInitScript(() => {
      document.addEventListener("DOMContentLoaded", () => {
        const style = document.createElement("style");
        style.textContent = ".post .video { margin-top: 3000px; }";
        document.head.append(style);
      });
    });
    await page.goto(at(`/p/${VIDEO_CAST}`));
    const player = page.locator(".post .video").first();
    const video = player.locator("video");
    await expect(player).toBeAttached();
    // Count frames, from now until it plays, in which the play icon shows over
    // the video while any of it is on screen.
    await player.evaluate((el) => {
      const w = window as any;
      w.__iconFrames = 0;
      const video = el.querySelector("video") as HTMLVideoElement;
      const tick = () => {
        const r = el.getBoundingClientRect();
        const onScreen = r.bottom > 0 && r.top < innerHeight;
        const button = el.querySelector(".video-play");
        const icon = el.querySelector(".video-play svg");
        const shown =
          !!button && !!icon && getComputedStyle(button).opacity !== "0" && getComputedStyle(icon).display !== "none";
        if (onScreen && video.paused && shown && r.top < innerHeight * 0.5) w.__iconFrames += 1;
        if (video.paused) requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });
    await player.scrollIntoViewIfNeeded();
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => !v.paused), { timeout: 15_000 }).toBe(true);
    const frames = await page.evaluate(() => (window as any).__iconFrames as number);
    // One frame of slack for the in-view signal to arrive.
    expect(frames, "frames with the play icon over the video before it played").toBeLessThanOrEqual(1);
  });

  test("a video that can't autoplay shows its play icon again", async ({ page }) => {
    // The real stream (H.264): Playwright's Chromium can't decode it, so
    // autoplay never starts and the icon must not stay hidden.
    await page.goto(at(`/p/${VIDEO_CAST}`));
    const player = page.locator(".post .video").first();
    await expect(player).toBeVisible();
    await page.waitForTimeout(3500);
    expect(await player.locator("video").evaluate((v: HTMLVideoElement) => v.paused)).toBe(true);
    expect(await player.locator(".video-play").evaluate((b) => getComputedStyle(b).opacity)).not.toBe("0");
  });

  test("videos respect reduced motion: no autoplay", async ({ browser }) => {
    const context = await browser.newContext({ reducedMotion: "reduce", viewport: { width: 1280, height: 900 } });
    const page = await context.newPage();
    await useFixtureStream(page);
    await page.goto(at(`/p/${VIDEO_CAST}`));
    const video = page.locator(".post .video video").first();
    await expect(video).toBeVisible();
    await page.waitForTimeout(1500);
    expect(await video.evaluate((v: HTMLVideoElement) => v.paused)).toBe(true);
    // No autoplay coming: the play icon shows.
    const button = page.locator(".post .video .video-play").first();
    expect(await button.evaluate((b) => getComputedStyle(b).opacity)).not.toBe("0");
    await context.close();
  });

  test("a video opens in the lightbox with the full player", async ({ page }) => {
    await useFixtureStream(page);
    await page.goto(at(`/p/${VIDEO_CAST}`));
    await page.locator(".post .video").first().hover();
    await page.locator(".post").getByRole("button", { name: "Open in the media viewer" }).first().click();
    const dialog = page.getByRole("dialog", { name: "Media viewer" });
    const video = dialog.locator("video");
    await expect(video).toBeVisible();
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.controls && v.currentTime > 0.2), { timeout: 15_000 }).toBe(true);
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
  });

  test("plain video files play natively, straight from their host", async ({ page }) => {
    // The same cast, with its stream swapped for a plain .mp4 file.
    const clip = "https://media.example/clip.mp4";
    const dir = path.join(__dirname, "fixtures/hls");
    const body = Buffer.concat(["init_0.mp4", "seg_0_0.m4s", "seg_0_1.m4s", "seg_0_2.m4s"].map((f) => readFileSync(path.join(dir, f))));
    await page.route(clip, (route) => route.fulfill({ body, contentType: "video/mp4" }));
    // The Worker's payload, or (static copy) Hypersnap's conversation answer.
    await page.route(new RegExp(`/api/v1/cast\\?hash=${VIDEO_CAST}$|haatz\\.quilibrium\\.com/v2/farcaster/cast/conversation\\?`), async (route) => {
      const response = await route.fetch();
      const text = (await response.text()).replace(/https:\/\/stream\.farcaster\.xyz\/[^"]+/g, clip);
      await route.fulfill({ response, body: text });
    });
    await page.goto(at(`/p/${VIDEO_CAST}`));
    const player = page.locator(".post .video").first();
    const video = player.locator("video");
    await expect(video).toHaveAttribute("src", `${clip}#t=0.1`);
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.muted && !v.paused)).toBe(true);
    await expect(player).toHaveClass(/autoplaying/);
    await player.getByRole("button", { name: /^Play with sound:/ }).click();
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.controls && !v.muted && v.currentTime > 0.5), { timeout: 15_000 }).toBe(true);
    await expect(player.locator(".video-note")).toHaveCount(0);
  });

  test("hovering media doesn't enlarge or highlight it", async ({ page }) => {
    const look = (el: Element) => {
      const style = getComputedStyle(el);
      return `${style.transform} ${style.opacity} ${style.filter}`;
    };
    const unchangedOnHover = async (media: Locator) => {
      await media.scrollIntoViewIfNeeded();
      await page.mouse.move(0, 0);
      const before = await media.evaluate(look);
      await media.hover({ force: true });
      await page.waitForTimeout(700);
      expect(await media.evaluate(look)).toBe(before);
      expect(before).toBe("none 1 none");
    };
    await page.goto(at(`/p/${YOUTUBE_CAST}`));
    await unchangedOnHover(page.locator(".post .facade img"));
    await page.goto(at(`/p/${VIDEO_CAST}`));
    await unchangedOnHover(page.locator(".post .video video").first());
    await page.goto(at("/"));
    await unchangedOnHover(page.locator(".cast-card .tile-image img").first());
  });

  test("YouTube links get a privacy-friendly, click-to-load player", async ({ page }) => {
    // Keep the test hermetic: nothing from YouTube itself needs to load.
    await page.route(/youtube-nocookie\.com|ytimg\.com/, (route) => route.fulfill({ status: 204, body: "" }));
    await page.goto(at(`/p/${YOUTUBE_CAST}`));
    const facade = page.locator(".post .facade[data-provider=youtube]");
    await expect(facade).toBeVisible();
    await expect(facade.locator("iframe")).toHaveCount(0);
    await expect(facade.locator("img")).toHaveAttribute("src", "https://i.ytimg.com/vi/cBT6vU55Gkg/hqdefault.jpg");
    await facade.getByRole("button", { name: /Play YouTube video/ }).click();
    await expect(facade.locator("iframe")).toHaveAttribute("src", /^https:\/\/www\.youtube-nocookie\.com\/embed\/cBT6vU55Gkg\?autoplay=1/);
  });
});

test.describe("profiles", () => {
  test("show registration, counts, the vote and wallet activity by chain, and no account label", async ({ page }) => {
    const labelReads: string[] = [];
    page.on("request", (r) => r.url().includes("/data/labels/") && labelReads.push(r.url()));
    await page.goto(at("/u/0xael.eth"));
    const profile = page.locator(".profile");
    await expect(profile.locator(".profile-name")).toHaveText(/\S/);
    await expect(profile.locator(".handle")).toContainText("@0xael.eth");
    await expect(profile.locator(".handle")).toContainText("fid 539854");
    await expect(page.getByText(/Updated (just now|\d+ (min|h) ago)/)).toHaveCount(0);
    await expect(profile.locator(".counts")).toContainText(/[\d,]+ followers/);
    await expect(profile.locator(".counts")).toContainText(/[\d,]+ following/);
    await expect(profile.locator(".bio")).toHaveText(/\S/);
    await expect(profile.locator(".fact", { hasText: "Joined" }).locator("dd")).toHaveText(/^\d{1,2} [A-Z][a-z]{2} \d{4}$/);
    // No spam label from the Farcaster app's dataset, and nothing read for one.
    await expect(page.getByText(/Farcaster app label|Labeled likely spam|Not labeled spam|Nerfed for/)).toHaveCount(0);
    expect(labelReads).toEqual([]);
    // The vote, read onchain on Base (0xael.eth voted to stay).
    await expect(profile.locator(".vote-fact .vote")).toHaveText("Voted yes");
    await expect(profile.getByText(/Neynar/)).toHaveCount(0);
    // Only "Voted yes", "Voted no" or "Didn't vote", never the old wordings.
    await expect(page.getByText(/Voted to (stay|leave)|Stay vote revoked|Did not vote|checking…/)).toHaveCount(0);
    await page.goto(at("/u/vitalik.eth"));
    await expect(page.locator(".profile .vote-fact .vote")).toHaveText("Didn't vote");
    // Registered before the ID registry's move to OP Mainnet: joined when
    // the account got its first username, on 18 Nov 2022.
    await expect(page.locator(".profile .fact", { hasText: "Joined" }).locator("dd")).toHaveText("18 Nov 2022");

    // The side area holds only the wallet infographic.
    const wallets = page.locator(".profile-wallets");
    await expect(wallets.locator(".share-bar")).toBeVisible();
    await expect(wallets.locator(".share-list li").first()).toContainText(/%/);
    await expect(wallets.locator(".share-list")).toContainText("Base");
    await expect(page.getByText(/How this is measured|Solana: Not measured|No activity on|Posts via|Verified addresses/)).toHaveCount(0);
    await expect(page.locator(".side .side-box")).toHaveCount(1);
    await expect(firstCard(page)).toBeVisible();
    await shoot(page, "profile");

    // "Wallets": every verified address, copyable in one click.
    await page.context().grantPermissions(["clipboard-read", "clipboard-write"]).catch(() => {});
    await wallets.getByRole("button", { name: /verified wallet/ }).click();
    const list = page.getByRole("dialog", { name: "Verified wallets" });
    await expect(list).toBeVisible();
    await expect(list.locator(".chain-tag")).toContainText(["EVM"]);
    expect(await list.locator("li").count()).toBeGreaterThanOrEqual(2);
    await list.locator("li").first().getByRole("button", { name: /^Copy/ }).click();
    await expect(list.locator("li").first().locator(".copy")).toHaveText("Copied");
    await page.evaluate(() => window.scrollTo(0, 0));
    await expectClearOfHeader(page, list);
    await shoot(page, "profile-wallets");
    await page.keyboard.press("Escape");
    await expect(list).toHaveCount(0);
  });


  test("dotted ENS-style names and the old static-site links resolve", async ({ page }) => {
    await page.goto(at("/u/?name=balajis.eth"));
    await expect(page.locator(".profile .handle")).toContainText("@balajis.eth · fid 37");
    await page.goto(at("/u/balajis.eth"));
    await expect(page.locator(".profile .handle")).toContainText("@balajis.eth");
    await expect(firstCard(page).locator("header.cast-meta .author")).toHaveText("@balajis.eth");
    // A name the account holds but no longer goes by: dwr.eth is fid 3, now @dwr.
    await page.goto(at("/u/dwr.eth"));
    await expect(page.locator(".profile .handle")).toContainText("@dwr · fid 3");
  });
});

test.describe("threads and navigation", () => {
  test("replies collapse and expand with animation, by click and with x", async ({ page }) => {
    await page.goto(at(`/p/${NESTED_THREAD}`));
    const parent = page.locator(".comments > .comment").filter({ has: page.locator(":scope > .comment-body .children .comment") }).first();
    await expect(parent).toBeVisible();
    const toggle = parent.locator(":scope > .comment-head .toggle");
    const child = parent.locator(":scope > .comment-body .children .comment").first();
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
    await expect(child).toBeVisible();

    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await expect(parent).toHaveClass(/collapsed/);
    await expect(parent.locator(":scope > .comment-head .hidden-count")).toContainText(/\d+ repl(y|ies) hidden/);
    await expect.poll(() => parent.locator(":scope > .comment-body").evaluate((e) => e.getBoundingClientRect().height)).toBeLessThan(2);
    await shoot(page, "thread-collapsed");

    // Expanding keeps the reader where they were.
    const before = await page.evaluate(() => window.scrollY);
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
    await expect(child).toBeVisible();
    expect(Math.abs((await page.evaluate(() => window.scrollY)) - before)).toBeLessThan(40);
  });

  test("posts name the signing app in their meta line, without a side box or backend badges", async ({ page }) => {
    await page.goto(at(`/p/${VIDEO_CAST}`));
    await expect(page.locator(".side-box", { hasText: "Posted via" })).toHaveCount(0);
    await expect(page.locator(".side-box", { hasText: "/" })).toHaveCount(0);
    const via = page.locator(".post .post-via");
    await expect(via).toContainText("via ");
    await expect(via).not.toContainText("Warpcast");
    await expect(via).toHaveAttribute("title", /Signed with key 0x[0-9a-f]{4}…[0-9a-f]{4}/);
    await expect(page.getByText(/Hypersnap|view on farcaster/i)).toHaveCount(0);
    await shoot(page, "post");
  });

  test("the whole card opens the post, but selecting text or clicking links does not", async ({ page }) => {
    await page.goto(at("/"));
    const card = textCards(page).first();
    await expect(card).toBeVisible();
    const hash = await card.getAttribute("data-hash");
    // Select text inside the card: no navigation.
    const text = card.locator(".cast-text");
    // Measured right before the drag, on the first line, so media loading
    // above can't have moved it.
    await text.scrollIntoViewIfNeeded();
    const box = (await text.boundingBox())!;
    const y = box.y + Math.min(box.height / 2, 10);
    await page.mouse.move(box.x + 2, y);
    await page.mouse.down();
    await page.mouse.move(box.x + Math.min(box.width - 2, 120), y, { steps: 5 });
    await page.mouse.up();
    await page.waitForTimeout(300);
    // The drag selected text (it did not drag a link), and did not navigate.
    const selected = await page.evaluate(() => window.getSelection()?.toString() ?? "");
    expect(selected.trim().length).toBeGreaterThan(2);
    expect((await text.innerText()).includes(selected.trim().slice(0, 3))).toBe(true);
    await expect(page).not.toHaveURL(/\/p\//);
    // A plain click on the same text (no selection) opens the post.
    await page.evaluate(() => window.getSelection()?.removeAllRanges());
    await text.click({ position: { x: 4, y: 8 } });
    await expect(page).toHaveURL(new RegExp(`/p/${hash}$`));
    await page.goBack();
    await expect(firstCard(page)).toBeVisible();
    // A link inside the card goes where it points.
    await card.locator("header.cast-meta .user-chip .author").click();
    await expect(page).toHaveURL(/\/u\//);
    await page.goBack();
    // Clicking empty space in the card opens the post.
    await expect(firstCard(page)).toBeVisible();
    const target = page.locator(`.cast-card[data-hash="${hash}"]`);
    const cardBox = (await target.boundingBox())!;
    await page.mouse.click(cardBox.x + cardBox.width - 12, cardBox.y + 10);
    await expect(page).toHaveURL(new RegExp(`/p/${hash}$`));
  });

  test("back lays out the feed's images at their size before they load", async ({ page }) => {
    await page.goto(at("/"));
    await expect(firstCard(page)).toBeVisible();
    // A card with a single image, scrolled past so its image has loaded.
    const image = page.locator(".cast-card > .media-grid.n1 .tile-image img").first();
    await image.scrollIntoViewIfNeeded();
    await expect.poll(() => image.evaluate((img: HTMLImageElement) => img.complete && img.naturalHeight > 0)).toBe(true);
    const hash = await image.evaluate((img) => img.closest(".cast-card")!.getAttribute("data-hash")!);
    const height = await image.evaluate((img) => Math.round(img.getBoundingClientRect().height));
    expect(height).toBeGreaterThan(50);
    // Open it inside the app, as a reader does.
    await page.locator(`.cast-card[data-hash="${hash}"] header.cast-meta a.when`).click();
    await expect(page).toHaveURL(new RegExp(`/p/${hash}$`));
    await expect(page.locator(".post")).toBeVisible();
    // Images now take 3 s to arrive.
    await page.route(
      (url) => !url.pathname.startsWith("/api/"),
      async (route) => {
        if (route.request().resourceType() === "image") await new Promise((done) => setTimeout(done, 3000));
        await route.fallback();
      },
    );
    await page.goBack();
    const again = page.locator(`.cast-card[data-hash="${hash}"] > .media-grid.n1 .tile-image img`);
    await expect(again).toBeAttached();
    // Its box is already the loaded image's size: nothing below it will move.
    expect(await again.evaluate((img: HTMLImageElement) => img.complete)).toBe(false);
    expect(Math.abs((await again.evaluate((img) => img.getBoundingClientRect().height)) - height)).toBeLessThanOrEqual(2);
  });

  test("opening a post from the feed renders at once, and back restores the scroll position", async ({ page }) => {
    await page.goto(at("/"));
    await expect(firstCard(page)).toBeVisible();
    await page.evaluate(() => window.scrollTo(0, 1600));
    await page.waitForTimeout(300);
    const card = textCards(page).nth(4);
    await card.scrollIntoViewIfNeeded();
    const hash = await card.getAttribute("data-hash");
    // Where the card is on screen. (Not window.scrollY: after Back, cards
    // above it that are off screen are laid out at an estimated height until
    // they come into view, so the same place can have another scrollY.)
    const left = page.locator(`.cast-card[data-hash="${hash}"]`);
    const top = () => left.evaluate((el) => Math.round(el.getBoundingClientRect().top));
    // Cards that a jump brings into view are drawn then, and grow from their
    // estimated height, and images above it load: measure once the card has
    // stopped moving.
    await page.waitForLoadState("networkidle").catch(() => {});
    let before = await top();
    await expect
      .poll(async () => {
        const [was, now] = [before, (before = await top())];
        return was === now;
      }, { intervals: [250] })
      .toBe(true);
    // Measure in the page: from the click to the post's text in the DOM.
    await page.evaluate(() => {
      (window as any).__openedIn = null;
      document.addEventListener("click", () => {
        const t0 = performance.now();
        const check = () => document.querySelector(".post .post-text, .post .mini-app-note")
          ? ((window as any).__openedIn = performance.now() - t0)
          : requestAnimationFrame(check);
        requestAnimationFrame(check);
      }, { capture: true, once: true });
    });
    await card.locator(".cast-text").click({ position: { x: 4, y: 8 } });
    await expect(page).toHaveURL(new RegExp(`/p/${hash}$`));
    await expect.poll(() => page.evaluate(() => (window as any).__openedIn)).not.toBeNull();
    const openedIn = await page.evaluate(() => (window as any).__openedIn as number);
    console.log(`post opened in ${openedIn.toFixed(0)} ms`);
    expect(openedIn, "post renders from feed data").toBeLessThan(100);

    await page.goBack();
    await expect(left).toBeVisible();
    await expect.poll(top).toBeGreaterThan(before - 30);
    expect(await top()).toBeLessThan(before + 30);
    // And it stays there.
    await page.waitForTimeout(1500);
    expect(Math.abs((await top()) - before)).toBeLessThan(30);
  });

  test("hovering likes, recasts and replies previews them; clicking opens the full list", async ({ page }) => {
    await page.goto(at(`/p/${NESTED_THREAD}`));
    const foot = page.locator(".post .engagement");
    await expect(foot).toBeVisible();
    await settleImages(page);
    await foot.locator(".stat-likes").hover();
    const likes = page.getByRole("dialog", { name: "Liked by" });
    await expect(likes).toBeVisible();
    await expect(likes.locator(".who-list li").first()).toBeVisible();
    expect(await likes.locator(".who-list li").count()).toBeLessThanOrEqual(10);
    await expectClearOfHeader(page, likes);
    await shoot(page, "likes-popover");
    // Moving into the popover keeps it open.
    await likes.locator(".who-list li").first().hover();
    await page.waitForTimeout(400);
    await expect(likes).toBeVisible();
    await page.mouse.move(5, 5);
    await expect(likes).toHaveCount(0);

    // Click: the full list in a dialog.
    await foot.locator(".stat-likes").click();
    const full = page.locator(".who-modal");
    await expect(full).toBeVisible();
    await expect.poll(() => full.locator(".who-list li").count()).toBeGreaterThan(10);
    await page.keyboard.press("Escape");
    await expect(full).toHaveCount(0);

    // Keyboard focus opens it too.
    await foot.locator(".stat-recasts").focus();
    await expect(page.getByRole("dialog", { name: "Recasted by" })).toBeVisible();
    await page.keyboard.press("Escape");

    // Replies preview from a card on the front page.
    await page.goto(at("/"));
    const withReplies = page.locator(".cast-card").filter({ has: page.locator("button.stat-replies") }).first();
    await expect(withReplies).toBeVisible();
    await settleImages(page);
    await withReplies.locator(".stat-replies").hover();
    const replies = page.getByRole("dialog", { name: "Replies" });
    await expect(replies).toBeVisible();
    await expect(replies.locator(".reply-item").first()).toBeVisible();
    await expect(replies.getByRole("link", { name: "View full thread →" })).toBeVisible();
    // Near the top of the page it opens downward, clear of the sticky header.
    await expectClearOfHeader(page, replies);
    await shoot(page, "replies-popover");
  });

  for (const { card, atLeast } of [
    { card: 20, atLeast: 50 },
    { card: 80, atLeast: 100 },
  ])
  test(`back restores the place at card ${card} of ${atLeast}+ and holds it, even when the post loads slowly`, async ({ page }) => {
    test.setTimeout(120_000);
    // A post the client never prefetched: its data takes 2.5 s.
    await page.route(/\/api\/v1\/cast\?/, async (route) => {
      await new Promise((done) => setTimeout(done, 2500));
      await route.continue();
    });
    await page.goto(at("/"));
    await expect(firstCard(page)).toBeVisible();
    // Several pages deep: more than one server page of trending casts.
    for (let i = 0; i < 40 && (await page.locator(".cast-card").count()) < atLeast + 10; i++) {
      await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
      await page.waitForTimeout(600);
    }
    const loaded = await page.locator(".cast-card").count();
    expect(loaded).toBeGreaterThan(atLeast);
    await page.locator(".cast-card").nth(card).scrollIntoViewIfNeeded();
    await page.mouse.wheel(0, -150);
    await page.waitForTimeout(600);
    // The card under the header line, and where it sits.
    const anchor = () =>
      page.evaluate(() => {
        const list = document.querySelector(".cast-list")!.getBoundingClientRect();
        // The lines the app itself probes: one may fall in the gap between cards.
        const card = [120, 150, 180]
          .map((y) => document.elementFromPoint(list.left + 24, y)?.closest(".cast-card"))
          .find(Boolean);
        return { hash: card?.getAttribute("data-hash") ?? "", top: Math.round(card?.getBoundingClientRect().top ?? 0), y: Math.round(scrollY) };
      });
    const before = await anchor();
    expect(before.hash).not.toBe("");
    const topOf = (hash: string) =>
      page.evaluate((h) => Math.round(document.querySelector(`.cast-card[data-hash="${h}"]`)?.getBoundingClientRect().top ?? -1e6), hash);
    // Open the anchor card itself, clicking where it is (no scrolling first).
    const openAnchor = async () => {
      const box = await page.evaluate((h) => {
        // The card's empty bottom-right corner: not a link, not under the header.
        const r = document.querySelector(`.cast-card[data-hash="${h}"]`)!.getBoundingClientRect();
        return { x: r.right - 10, y: Math.min(r.bottom, window.innerHeight) - 8 };
      }, before.hash);
      await page.mouse.click(box.x, box.y);
      await expect(page).toHaveURL(new RegExp(`/p/${before.hash}$`));
    };
    await openAnchor();
    await page.waitForTimeout(500);
    await page.goBack();
    await expect(page).toHaveURL(new RegExp(`${at("/")}$`));
    // Where things are, for a failure's message: the anchor card's top and
    // place in the list, the scroll offset and how much of the page exists.
    const where = () =>
      page.evaluate((h) => {
        const cards = [...document.querySelectorAll(".cast-card")];
        const card = document.querySelector(`.cast-card[data-hash="${h}"]`);
        return JSON.stringify({
          top: card ? Math.round(card.getBoundingClientRect().top) : null,
          index: card ? cards.indexOf(card) : -1,
          cards: cards.length,
          scrollY: Math.round(scrollY),
          height: document.documentElement.scrollHeight,
        });
      }, before.hash);
    for (const at of [500, 2000, 4000, 7000]) {
      await page.waitForTimeout(at === 500 ? 500 : at === 2000 ? 1500 : at === 4000 ? 2000 : 3000);
      // Polled briefly: an image loading above can shift it for a frame
      // before the hold puts it back.
      await expect
        .poll(async () => Math.abs((await topOf(before.hash)) - before.top), { message: `card back in place at ${at} ms`, timeout: 400, intervals: [50] })
        .toBeLessThanOrEqual(4)
        .catch(async (error: Error) => {
          throw new Error(`${error.message}\nexpected top ${before.top} (card ${card}, scrollY ${before.y}); now ${await where()}`);
        });
    }
    expect(await page.locator(".cast-card").count()).toBeGreaterThanOrEqual(loaded);

    // The reader's own scrolling during the hold is not undone. (Images are
    // let finish first, so only the wheel moves the page.)
    await openAnchor();
    await page.goBack();
    await page.waitForTimeout(800);
    await settleImages(page);
    const held = await topOf(before.hash);
    await page.mouse.wheel(0, 600);
    await page.waitForTimeout(800);
    expect(held - (await topOf(before.hash)), "the wheel moved the page").toBeGreaterThan(300);
    // ...and it stays where the reader put it (no snapping back).
    await page.waitForTimeout(1500);
    expect(held - (await topOf(before.hash)), "still where the reader scrolled").toBeGreaterThan(300);
  });

  test("spam is left out of the feed and search", async ({ page }) => {
    // Links to vercel.app apps, gig-bot, and DEGEN reward or airdrop bait.
    const spammy = (root: Locator) =>
      root.evaluateAll((cards) =>
        cards
          .filter((card) => {
            const text = (card.textContent ?? "").toLowerCase().replace(/\$/g, "");
            const links = [...card.querySelectorAll("a[href]")].map((a) => (a as HTMLAnchorElement).href.toLowerCase());
            return (
              /gig-?bot|unlocked my degen|degen (reward|airdrop)/.test(text) ||
              links.some((h) => /\.vercel\.app|gig-?bot|[?&]degen=1/.test(h))
            );
          })
          .map((card) => card.getAttribute("data-hash")),
      );
    await page.goto(at("/"));
    await expect(firstCard(page)).toBeVisible();
    expect(await spammy(page.locator(".cast-card"))).toEqual([]);
    for (const q of ["gig-bot", "DEGEN rewards", "vercel.app"]) {
      await page.goto(at(`/search?q=${encodeURIComponent(q)}`));
      const casts = page.getByRole("region", { name: "Casts" });
      await expect(page.getByRole("region", { name: "People" }).or(casts).or(page.locator(".empty")).first()).toBeVisible();
      expect(await spammy(casts.locator(".cast-card")), `search ${q}`).toEqual([]);
    }
  });

  test("one search box finds posts, people and channels", async ({ page }) => {
    await page.goto(at("/"));
    const box = page.getByRole("combobox", { name: /Search casts, people and channels/ });
    await box.fill("farcaster");
    await box.press("Enter");
    await expect(page).toHaveURL(/\/search\?q=farcaster/);
    await expect(page.getByRole("region", { name: "People" }).locator(".person").first()).toBeVisible();
    await expect(page.getByRole("region", { name: "Channels" }).locator("li").first()).toBeVisible();
    await expect(page.getByRole("region", { name: "Casts" }).locator(".cast-card").first()).toBeVisible();
    await expect(box).toHaveValue("farcaster");
    // Long groups are capped so casts stay near the top; "Show all" expands.
    const people = page.getByRole("region", { name: "People" });
    expect(await people.locator(".person").count()).toBeLessThanOrEqual(6);
    const casts = await page.getByRole("region", { name: "Casts" }).boundingBox();
    expect(casts!.y).toBeLessThan(1400);
    await shoot(page, "search");
    const showAll = people.getByRole("button", { name: /^Show all \d+ people$/ });
    if (await showAll.count()) {
      const total = Number((await showAll.innerText()).match(/\d+/)![0]);
      await showAll.click();
      await expect(people.locator(".person")).toHaveCount(total);
      await expect(showAll).toHaveCount(0);
    }
  });

  test("channels and the directory render", async ({ page }) => {
    await page.goto(at("/channels"));
    await expect(page.locator(".channel-card").first()).toBeVisible();
    await page.locator(".channel-card a", { hasText: "/farcaster" }).click();
    await expect(page).toHaveURL(/\/c\/farcaster/);
    // The Farcaster app's display name, or the id where the app can't be read (the static copy).
    await expect(page.locator(".page-title")).toHaveText(/^(Farcaster|\/farcaster)$/);
    await expect(firstCard(page)).toBeVisible();
  });

  test("channel pages show the channel's details and true member counts", async ({ page }) => {
    for (const { id, name } of [
      { id: "dev", name: "/dev" },
      { id: "farcaster", name: "Farcaster" },
      { id: "base", name: "Base" },
    ]) {
      await page.goto(at(`/c/${id}`));
      await expect(firstCard(page)).toBeVisible();
      const hero = page.locator(".channel-hero");
      const counts = hero.locator(".channel-stats .count");
      if (PAGES) {
        // The static copy can't read the Farcaster app's channel API (no CORS
        // headers): Hypersnap's count of casters shows, never called members.
        await expect(page.getByRole("heading", { level: 1 })).toHaveText(`/${id}`);
        await expect(counts).toHaveText([/^[\d,]+ casters?$/]);
        await expect(hero).not.toContainText(/\bmembers?\b/);
        continue;
      }
      await expect(page.getByRole("heading", { level: 1 })).toHaveText(name);
      if (name !== `/${id}`) await expect(hero.locator(".channel-handle")).toHaveText(`/${id}`);
      const members = counts.filter({ hasText: /^[\d,]+ members?$/ });
      await expect(members.or(counts.filter({ hasText: /^[\d,]+ followers?$/ })).first()).toBeVisible();
      await expect(counts.filter({ hasText: /casters?$/ })).toHaveCount(0);
      if (id === "dev") {
        // Hypersnap's count of distinct casters (15,664) was shown as members; /dev has a few hundred.
        const n = Number((await members.innerText()).replace(/\D/g, ""));
        expect(n, "/dev's member count").toBeGreaterThan(0);
        expect(n, "/dev's member count").toBeLessThan(5000);
      }
      if (id === "farcaster") {
        const pinned = page.getByRole("region", { name: "Pinned" });
        if (await pinned.count()) {
          const card = pinned.locator(".cast-card");
          await expect(card).toHaveCount(1);
          await expect(card.locator("header.cast-meta .user-chip .author")).toHaveText(/^@/);
        }
      }
    }
  });

  test("read-only: no sign-in, no writes", async ({ page }) => {
    const writes: string[] = [];
    page.on("request", (r) => {
      const url = new URL(r.url());
      // Cloudflare Web Analytics reports page timings to /cdn-cgi/rum; that is
      // the platform's beacon, not a write by Allthing.
      if (url.pathname === "/cdn-cgi/rum") return;
      if (r.method() !== "GET" && r.method() !== "HEAD" && url.origin === new URL(page.url() || "http://x").origin) writes.push(r.url());
    });
    await page.goto(at("/"));
    await expect(firstCard(page)).toBeVisible();
    await expect(page.locator("header, .side, footer").getByText(/sign in|log in|connect wallet/i)).toHaveCount(0);
    expect(writes).toHaveLength(0);
    if (!PAGES) {
      const response = await page.request.post("/api/v1/home", { data: {} });
      expect(response.status()).toBe(405);
    }
  });

  test("pages load without Content Security Policy violations", async ({ page }) => {
    const violations: string[] = [];
    await page.exposeFunction("__reportCsp", (v: string) => violations.push(v));
    await page.addInitScript(() => {
      document.addEventListener("securitypolicyviolation", (e) =>
        (window as any).__reportCsp(`${e.violatedDirective} ${e.blockedURI}`),
      );
    });
    for (const path of ["/", `/p/${VIDEO_CAST}`, "/u/0xael.eth"]) {
      await page.goto(at(path));
      await expect(page.locator("main .cast-card, main .post").first()).toBeVisible();
      await page.waitForTimeout(1500);
    }
    expect(violations).toEqual([]);
  });

  test("new lists casts from the last few minutes, from Snapchain's event stream", async ({ page, request }) => {
    await page.goto(at("/?sort=new"));
    const first = firstCard(page);
    await expect(first).toBeVisible();
    const age = Date.now() / 1000 - Number(await first.getAttribute("data-ts"));
    expect(age, `newest cast is ${Math.round(age)} s old`).toBeLessThan(600);
    // /status reports the real lag of both lists.
    test.skip(PAGES, "the static copy has no Worker");
    // The edge cache is per data center: make sure the one this client
    // reaches holds the list, then read its freshness there.
    await request.get(at("/api/v1/latest"));
    const status = await (await request.get(at("/api/v1/status"))).json();
    expect(status.freshness.new.newest_cast_age_s).toBeLessThan(600);
    expect(status.freshness.new.built_age_s).toBeGreaterThanOrEqual(0);
  });

  // Mixed case and an "@" or "0X" prefix are normalized the way the app
  // normalizes them, so the app still finds the request it started.
  for (const [page_, path, api, shown] of [
    ["front page", "/", "/api/v1/home", ".cast-card:not(.skeleton)"],
    ["post", `/p/${GALLERY_CAST.replace("0x", "0X").toUpperCase()}`, `/api/v1/cast?hash=${GALLERY_CAST}`, ".post .post-text"],
    ["profile", "/u/@DWR", "/api/v1/user?name=dwr", ".profile-name"],
    ["channel", "/c/Farcaster", "/api/v1/channel?id=farcaster", ".cast-card:not(.skeleton)"],
  ] as const) {
    test(`the ${page_}'s data request starts inline and is reused`, async ({ page }) => {
      test.skip(PAGES, "the static copy has no API");
      const violations: string[] = [];
      page.on("console", (m) => /Content Security Policy/.test(m.text()) && violations.push(m.text()));
      const calls: string[] = [];
      page.on("request", (r) => r.url().includes("/api/v1/") && calls.push(new URL(r.url()).pathname + new URL(r.url()).search));
      await page.goto(at(path));
      await expect(page.locator(shown).first()).toBeVisible();
      // The inline script ran (CSP allowed it) and the app took its response:
      // the page's one data request is the one the script made.
      expect(await page.evaluate(() => "__allthingEarly" in window)).toBe(false);
      expect(calls.filter((c) => c.startsWith(api.split("?")[0]))).toEqual([api]);
      expect(violations).toEqual([]);
    });
  }

  test("/version names the release and commit", async ({ request }) => {
    test.skip(PAGES, "the static copy has no Worker");
    const response = await request.get("/version");
    expect(response.status()).toBe(200);
    const body = await response.json();
    expect(body.name).toBe("allthing");
    expect(body.version).toMatch(/^\d+\.\d+\.\d+$/);
    expect(body.commit).toMatch(/^[0-9a-f]{7}|dev$/);
  });

  test("unknown things say so", async ({ page }) => {
    await page.goto(at("/u/no-such-user-zz9-plural-z-alpha"));
    await expect(page.locator(".error-panel")).toContainText("Nothing here");
    await page.goto(at("/p/0x0000000000000000000000000000000000000000"));
    await expect(page.locator(".error-panel")).toContainText("Nothing here");
    await page.goto(at("/definitely/not/a/page"));
    await expect(page.locator(".error-panel")).toContainText("Page not found");
  });
});

test.describe("phone width", () => {
  test.use({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });

  test("pages fit the screen without sideways scrolling", async ({ page }) => {
    for (const path of ["/", "/u/0xael.eth", `/p/${VIDEO_CAST}`, "/search?q=farcaster", "/c/farcaster", "/channels"]) {
      await page.goto(at(path));
      await expect(page.locator("main .cast-card, main .person, main .channel-card, main .post").first()).toBeVisible();
      await page.waitForTimeout(500);
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
      expect(overflow, `${path} scrolls sideways`).toBeLessThanOrEqual(0);
    }
    await shoot(page, "phone-channels");
  });

  test("profiles show the wallet panel before the casts", async ({ page }) => {
    await page.goto(at("/u/0xael.eth"));
    const panel = page.locator(".side .side-box").first();
    const firstCast = page.locator(".cast-card").first();
    await expect(firstCast).toBeVisible();
    await expect(panel).toBeVisible();
    const [p, c, h] = await Promise.all([panel.boundingBox(), firstCast.boundingBox(), page.locator(".profile").boundingBox()]);
    expect(p!.y).toBeGreaterThan(h!.y + h!.height - 1);
    expect(p!.y + p!.height).toBeLessThan(c!.y);
    await panel.getByRole("button", { name: /verified wallet/ }).tap();
    const list = page.getByRole("dialog", { name: "Verified wallets" });
    await expect(list).toBeVisible();
    await page.waitForTimeout(200);
    await expectClearOfHeader(page, list);
    await shoot(page, "phone-profile-wallets");
  });

  test("tapping a replies count opens the preview", async ({ page }) => {
    await page.goto(at("/"));
    const button = page.locator(".cast-card button.stat-replies").first();
    await expect(button).toBeVisible();
    await button.tap();
    const replies = page.getByRole("dialog", { name: "Replies" });
    await expect(replies).toBeVisible();
    await expect(replies.locator(".reply-item").first()).toBeVisible();
    expect(new URL(page.url()).pathname).toBe(at("/"));
    await page.waitForTimeout(200);
    await expectClearOfHeader(page, replies);
    await shoot(page, "phone-replies");
    // A second tap closes it.
    await button.tap();
    await expect(replies).toHaveCount(0);
  });
});

test.describe("hosting", () => {
  test.skip(!LIVE || PAGES, "only against the Cloudflare deployment");

  test("www redirects to the apex domain", async ({ page }) => {
    const base = new URL(process.env.E2E_BASE_URL!);
    await page.goto(`${base.protocol}//www.${base.host}/u/dwr`);
    await expect(page).toHaveURL(`${base.origin}/u/dwr`);
    await expect(page.locator(".profile .handle")).toContainText("@dwr");
  });

  test("static assets are long-cached and the API is edge-cached", async ({ request }) => {
    const index = await request.get("/");
    const asset = /\/assets\/allthing-[0-9a-f]+\.wasm/.exec(await index.text())?.[0];
    expect(asset).toBeTruthy();
    const wasm = await request.get(asset!);
    expect(wasm.headers()["cache-control"]).toContain("immutable");
    await request.get("/api/v1/home");
    const second = await request.get("/api/v1/home");
    expect(second.headers()["x-allthing-cache"]).toMatch(/fresh|revalidating/);
  });
});
