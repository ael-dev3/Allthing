// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";
import path from "node:path";
import { NOW } from "./fixture-time";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const hash = `0x${"a".repeat(40)}`;
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const fixtures = path.join(__dirname, "fixtures/hls");

const withVideo = (hash: string, text: string, url: string) => ({
  hash, author: { fid: 1, username: "fixture" }, text, ts: (NOW - 19), embeds: [{ kind: "video", url }],
});

test.beforeEach(async ({ page }) => {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  // A post with a video, and a reply with another.
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
    thread: {
      cast: withVideo(hash, "Sticky sound fixture", "https://media.example/first.m3u8"),
      children: [{ cast: withVideo(`0x${"b".repeat(40)}`, "A reply with a video", "https://media.example/second.m3u8") }],
    },
    meta,
  } }));
  // Both stream the VP9 and Opus fixture (Playwright's Chromium has no H.264).
  await page.route("https://media.example/**", (route) => {
    const name = new URL(route.request().url()).pathname.split("/").pop() ?? "";
    const file = name.endsWith(".m3u8") && !name.startsWith("stream_") ? "master.m3u8" : name;
    const type = file.endsWith(".m3u8") ? "application/vnd.apple.mpegurl" : "video/mp4";
    return route.fulfill({
      body: readFileSync(path.join(fixtures, file)), contentType: type, headers: { "access-control-allow-origin": "*" },
    });
  });
  // The reply's video sits far below, so only one is in view at a time.
  await page.addInitScript(() => {
    document.addEventListener("DOMContentLoaded", () => {
      const style = document.createElement("style");
      style.textContent = ".comment .video { margin-top: 1600px; }";
      document.head.append(style);
    });
  });
});

for (const width of [1280, 390]) {
  test(`a stale pause event does not hide controls for a playing video at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    await page.goto(`/p/${hash}`);
    const player = page.locator(".video").first();
    const video = player.locator("video");
    await expect.poll(() => video.evaluate((v) => !v.paused && v.currentTime > 0.2), {timeout:15000}).toBe(true);
    await expect(player).toHaveClass(/autoplaying/);
    // Model a queued event arriving after playback resumed. The element's
    // current state stays authoritative even when an older event arrives.
    await video.evaluate((v) => v.dispatchEvent(new Event("pause")));
    expect(await video.evaluate((v) => v.paused)).toBe(false);
    await expect(player).toHaveClass(/autoplaying/, {timeout:2000});
    await expect(player.getByRole("button", {name:"Sound off · tap for sound",exact:true})).toBeVisible();
  });
  test(`a stale playing event does not show controls for a paused video at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    await page.goto(`/p/${hash}`);
    const player = page.locator(".video").first();
    const video = player.locator("video");
    await expect.poll(() => video.evaluate((v) => !v.paused && v.currentTime > 0.2), {timeout:15000}).toBe(true);
    await video.evaluate((v) => v.pause());
    await expect(player).not.toHaveClass(/autoplaying/);
    await video.evaluate((v) => v.dispatchEvent(new Event("playing")));
    expect(await video.evaluate((v) => v.paused)).toBe(true);
    await expect(player).not.toHaveClass(/autoplaying/, {timeout:2000});
  });
}
