// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Locator, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import path from "node:path";

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
      cast: { hash, author: { fid: 1, username: "fixture" }, text: "Video fixture", ts: 1792235681,
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
    await expect(badge.locator("svg")).toBeHidden();
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

  test("is a speaker in the corner, a finger-sized target, and a tap turns the sound on", async ({ page }, info) => {
    await videoPost(page);
    await page.goto(`/p/${hash}`);
    const player = page.locator(".post .video");
    const badge = player.getByRole("button", { name: "Sound off · tap for sound", exact: true });
    await expect(player).toHaveClass(/autoplaying/, { timeout: 15_000 });
    // Showing without a hover, as a 32 px speaker, its words for screen readers only.
    expect(await opacity(badge)).toBe("1");
    const box = (await badge.boundingBox())!;
    expect(Math.round(box.width)).toBe(32);
    expect(Math.round(box.height)).toBe(32);
    await expect(badge.locator("svg")).toBeVisible();
    expect(await badge.locator(".video-sound-label").evaluate((l) => l.getBoundingClientRect().width)).toBeLessThanOrEqual(1);
    // A tap 5 px outside the speaker still reaches it, and plays the video's
    // sound without its controls.
    await page.screenshot({ path: info.outputPath("video-badge-390.png") });
    await page.touchscreen.tap(box.x - 5, box.y + box.height / 2);
    const on = player.getByRole("button", { name: "Sound on · tap to mute", exact: true });
    await expect(on).toBeVisible();
    await expect.poll(() => player.locator("video").evaluate((v: HTMLVideoElement) => !v.muted && !v.paused && !v.controls)).toBe(true);
    await page.screenshot({ path: info.outputPath("video-badge-390-sound.png") });
  });
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
