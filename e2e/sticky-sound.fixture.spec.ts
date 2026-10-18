// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { activateVisible } from "./helpers/pointer";
import { readFileSync } from "node:fs";
import path from "node:path";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const hash = `0x${"a".repeat(40)}`;
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const fixtures = path.join(__dirname, "fixtures/hls");

const withVideo = (hash: string, text: string, url: string) => ({
  hash, author: { fid: 1, username: "fixture" }, text, ts: 1792235681, embeds: [{ kind: "video", url }],
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

const sound = (page: Page) => page.evaluate(() => localStorage.getItem("allthing.sound"));
const playingMuted = (v: HTMLVideoElement) => v.muted && !v.paused;
const playingWithSound = (v: HTMLVideoElement) => !v.muted && !v.paused;

test("once the reader turns sound on, later videos autoplay with sound until they mute one", async ({ page }) => {
  await page.goto(`/p/${hash}`);
  const [first, second] = [page.locator(".video").first(), page.locator(".video").nth(1)];
  await expect.poll(() => first.locator("video").evaluate(playingMuted), { timeout: 15_000 }).toBe(true);
  expect(await sound(page)).toBeNull();

  // The badge turns this video's sound on, in place.
  await activateVisible(first.getByRole("button", { name: "Sound off · tap for sound", exact: true }));
  await expect.poll(() => first.locator("video").evaluate(playingWithSound)).toBe(true);
  await expect(first.getByRole("button", { name: "Sound on · tap to mute", exact: true })).toBeVisible();
  await expect(first.getByRole("button", { name: /^Show controls:/ })).toBeVisible();
  expect(await sound(page)).toBe("on");

  // The next video autoplays with sound.
  await second.scrollIntoViewIfNeeded();
  await expect.poll(() => first.locator("video").evaluate((v: HTMLVideoElement) => v.paused)).toBe(true);
  await expect.poll(() => second.locator("video").evaluate(playingWithSound), { timeout: 15_000 }).toBe(true);

  // Muting it turns sound off for the videos after it.
  await activateVisible(second.getByRole("button", { name: "Sound on · tap to mute", exact: true }));
  await expect.poll(() => second.locator("video").evaluate(playingMuted)).toBe(true);
  expect(await sound(page)).toBeNull();
  await first.scrollIntoViewIfNeeded();
  await expect.poll(() => first.locator("video").evaluate(playingMuted), { timeout: 15_000 }).toBe(true);
});

test("playing a video turns sound on, and muting it in its controls turns sound off", async ({ page }) => {
  await page.goto(`/p/${hash}`);
  const player = page.locator(".video").first();
  const video = player.locator("video");
  await expect.poll(() => video.evaluate(playingMuted), { timeout: 15_000 }).toBe(true);
  await player.getByRole("button", { name: /^Play with sound:/ }).click();
  await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.controls && !v.muted && !v.paused)).toBe(true);
  expect(await sound(page)).toBe("on");

  // The reader mutes and unmutes it with its own controls (as their mute
  // button does): the choice holds for later videos.
  await video.evaluate((v: HTMLVideoElement) => { v.muted = true; });
  await expect.poll(() => sound(page)).toBeNull();
  await video.evaluate((v: HTMLVideoElement) => { v.muted = false; });
  await expect.poll(() => sound(page)).toBe("on");
});

for (const gesture of ["key press", "tap"] as const) {
  test(`with sound on, a video the browser won't play with sound yet plays muted until the reader's first ${gesture}`, async ({ page, hasTouch }) => {
    // Sound was turned on in an earlier visit.
    await page.addInitScript(() => localStorage.setItem("allthing.sound", "on"));
    // Report playback without evaluating anything in the page: Playwright
    // evaluates with a user gesture, which would let it play with sound.
    await page.addInitScript(() => {
      document.addEventListener("playing", (e) => console.log(`playing muted=${(e.target as HTMLVideoElement).muted}`), true);
    });
    const playing = page.waitForEvent("console", { predicate: (m) => m.text().startsWith("playing "), timeout: 20_000 });
    await page.goto(`/p/${hash}`);
    // Nothing from the reader yet: refused sound, so it plays muted.
    expect((await playing).text()).toBe("playing muted=true");
    const video = page.locator(".post .video video");
    expect(await video.evaluate(playingMuted)).toBe(true);
    expect(await sound(page)).toBe("on");
    await expect(page.locator(".post .video").getByRole("button", { name: "Sound off · tap for sound", exact: true })).toBeVisible();

    // The reader's first gesture anywhere on the page turns its sound on.
    if (gesture === "key press") await page.keyboard.press("a");
    else if (hasTouch) await page.locator(".post-text").tap();
    else await page.locator(".post-text").click();
    await expect.poll(() => video.evaluate(playingWithSound)).toBe(true);
    expect(await sound(page)).toBe("on");
  });
}
