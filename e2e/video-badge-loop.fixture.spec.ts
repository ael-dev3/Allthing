// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Locator, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import path from "node:path";
import { NOW } from "./fixture-time";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const hash = `0x${"a".repeat(40)}`;
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const fixtures = path.join(__dirname, "fixtures/hls");

// A post with a video streaming the VP9 and Opus fixture (Playwright's
// Chromium has no H.264). `extinf` rewrites each 2 s segment's length in the
// playlists; the media stays 6 s long. Returns the segments asked for.
async function videoPost(page: Page, extinf = "2.000000") {
  const segments: string[] = [];
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
    thread: {
      cast: { hash, author: { fid: 1, username: "fixture" }, text: "Video fixture", ts: (NOW - 19),
        embeds: [{ kind: "video", url: "https://media.example/clip.m3u8" }] },
      children: [],
    },
    meta,
  } }));
  await page.route("https://media.example/**", (route) => {
    const name = new URL(route.request().url()).pathname.split("/").pop() ?? "";
    const file = name.endsWith(".m3u8") && !name.startsWith("stream_") ? "master.m3u8" : name;
    if (file.endsWith(".m4s")) segments.push(file);
    let body: Buffer | string = readFileSync(path.join(fixtures, file));
    if (file.startsWith("stream_")) body = body.toString().replaceAll("#EXTINF:2.000000,", `#EXTINF:${extinf},`);
    const type = file.endsWith(".m3u8") ? "application/vnd.apple.mpegurl" : "video/mp4";
    return route.fulfill({ body, contentType: type, headers: { "access-control-allow-origin": "*" } });
  });
  return segments;
}

const opacity = (badge: Locator) => badge.evaluate((b) => getComputedStyle(b).opacity);

test.describe("the sound badge on a computer", () => {
  test("shows only while the pointer is over the video, or the keyboard is on it", async ({ page }, info) => {
    await videoPost(page);
    await page.mouse.move(0, 0);
    await page.goto(`/p/${hash}`);
    const player = page.locator(".post .video");
    const badge = player.locator(".video-sound");
    await expect(player).toHaveClass(/autoplaying/, { timeout: 15_000 });
    await expect.poll(() => opacity(badge)).toBe("0");
    await page.screenshot({ path: info.outputPath("video-badge-1280-rest.png") });

    // In words, over the video's corner.
    const box = (await player.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await expect.poll(() => opacity(badge)).toBe("1");
    await expect(badge).toHaveText("Sound off · tap for sound");
    await page.screenshot({ path: info.outputPath("video-badge-1280-hover.png") });
    await page.mouse.move(0, 0);
    await expect.poll(() => opacity(badge)).toBe("0");

    // Tabbing to the video, then to the badge, shows it.
    await page.locator(".post-text").click();
    const play = player.getByRole("button", { name: /^Play with sound:/ });
    for (let i = 0; i < 40 && !(await play.evaluate((b) => b === document.activeElement)); i++) {
      await page.keyboard.press("Tab");
    }
    await expect(play).toBeFocused();
    await expect.poll(() => opacity(badge)).toBe("1");
    await page.keyboard.press("Tab");
    await expect(badge).toBeFocused();
    await expect.poll(() => opacity(badge)).toBe("1");
    await page.keyboard.press("Enter");
    await expect(badge).toHaveText("Sound on · tap to mute");
    await expect.poll(() => player.locator("video").evaluate((v: HTMLVideoElement) => !v.muted && !v.paused)).toBe(true);
  });
});

test.describe("the sound badge on a phone", () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });

  test("is hidden; a tap on the video shows it for a moment, and a tap on it turns the sound on", async ({ page }, info) => {
    await videoPost(page);
    await page.goto(`/p/${hash}`);
    const player = page.locator(".post .video");
    const video = player.locator("video");
    const badge = player.locator(".video-sound");
    await expect(player).toHaveClass(/autoplaying/, { timeout: 15_000 });
    // Hidden, and a tap where it would be doesn't find it.
    await expect.poll(() => opacity(badge)).toBe("0");
    expect(await badge.evaluate((b) => getComputedStyle(b).pointerEvents)).toBe("none");
    await page.screenshot({ path: info.outputPath("video-badge-390-rest.png") });

    // A tap on the video shows the badge, in words, and doesn't open the
    // controls; it hides again after a moment.
    const box = (await player.boundingBox())!;
    await page.touchscreen.tap(box.x + box.width / 2, box.y + box.height / 2);
    await expect.poll(() => opacity(badge)).toBe("1");
    await expect(badge).toHaveText("Sound off · tap for sound");
    expect(await video.evaluate((v: HTMLVideoElement) => v.controls || !v.muted)).toBe(false);
    await page.screenshot({ path: info.outputPath("video-badge-390-tap.png") });
    await expect.poll(() => opacity(badge), { timeout: 6_000 }).toBe("0");

    // Shown again, a tap on it (or a finger's width beside it) turns the
    // sound on, without controls, and it says so for a moment.
    await page.touchscreen.tap(box.x + box.width / 2, box.y + box.height / 2);
    await expect.poll(() => opacity(badge)).toBe("1");
    const pill = (await badge.boundingBox())!;
    expect(pill.height).toBeGreaterThanOrEqual(28);
    await page.touchscreen.tap(pill.x + pill.width / 2, pill.y + pill.height + 6);
    await expect(badge).toHaveText("Sound on · tap to mute");
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => !v.muted && !v.paused && !v.controls)).toBe(true);
    await page.screenshot({ path: info.outputPath("video-badge-390-sound.png") });

    // A tap on the video while the badge shows plays it with controls.
    await page.touchscreen.tap(box.x + box.width / 2, box.y + box.height / 3);
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.controls && !v.paused)).toBe(true);
    await expect(badge).toHaveCount(0);
  });

  test("a feed video loops on a phone too", async ({ page }) => {
    await videoPost(page);
    await page.goto(`/p/${hash}`);
    const video = page.locator(".post .video video");
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.currentTime), { timeout: 15_000 }).toBeGreaterThan(5);
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.currentTime < 2 && !v.paused && !v.ended), { timeout: 10_000 }).toBe(true);
  });
});

test("a video the reader plays loops until they pause it, and stays paused", async ({ page }) => {
  await videoPost(page);
  await page.goto(`/p/${hash}`);
  const player = page.locator(".post .video");
  const video = player.locator("video");
  await expect(player).toHaveClass(/autoplaying/, { timeout: 15_000 });
  await player.getByRole("button", { name: /^Play with sound:/ }).click();
  await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.controls && v.loop && !v.paused)).toBe(true);
  // Past the end, it plays on.
  await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.currentTime), { timeout: 15_000 }).toBeGreaterThan(5);
  await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.currentTime < 2 && !v.paused), { timeout: 10_000 }).toBe(true);
  // Paused (its controls' button calls pause()), it stays paused, even
  // after scrolling away and back.
  await video.evaluate((v: HTMLVideoElement) => v.pause());
  await page.evaluate(() => { document.body.style.minHeight = "4000px"; window.scrollTo(0, 3000); });
  await page.waitForTimeout(500);
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.waitForTimeout(1500);
  expect(await video.evaluate((v: HTMLVideoElement) => v.paused)).toBe(true);
});

for (const [shape, extinf] of [["matches", "2.000000"], ["runs longer than", "2.300000"], ["runs shorter than", "1.700000"]]) {
  test(`a feed video loops when its playlist ${shape} its media, fetching each segment once`, async ({ page }) => {
    const segments = await videoPost(page, extinf);
    await page.goto(`/p/${hash}`);
    const video = page.locator(".post .video video");
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.currentTime), { timeout: 15_000 }).toBeGreaterThan(5);
    // Past the end, it starts again rather than stopping on the last frame.
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.currentTime < 2 && !v.paused && !v.ended), { timeout: 10_000 }).toBe(true);
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.currentTime), { timeout: 10_000 }).toBeGreaterThan(2.5);
    // The media's own length, not the playlist's.
    expect(await video.evaluate((v: HTMLVideoElement) => v.duration)).toBeLessThan(6.1);
    // No segment twice (a playlist off from its media had the player fetch
    // the last two over and over).
    expect(new Set(segments).size, segments.join(" ")).toBe(segments.length);
    expect(segments.filter((s) => s.startsWith("seg_0_")).sort()).toEqual(["seg_0_0.m4s", "seg_0_1.m4s", "seg_0_2.m4s"]);
  });
}
