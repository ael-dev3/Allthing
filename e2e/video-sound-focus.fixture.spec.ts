// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";
import { activateVisible } from "./helpers/pointer";
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
  // Retain a short trusted-input/media timeline if the keyboard transition
  // fails, so native activation can be distinguished from a focus/state change.
  await page.addInitScript(() => {
    const events: Array<Record<string, unknown>> = [];
    (window as any).soundInput = events;
    const record = (event: Event) => {
      const target = event.target instanceof Element ? event.target : null;
      if (target && !target.closest(".post .video")) return;
      queueMicrotask(() => {
        const player = document.querySelector(".post .video");
        const video = player?.querySelector("video");
        events.push({
          at: performance.now(), type: event.type, trusted: event.isTrusted, prevented: event.defaultPrevented,
          key: (event as KeyboardEvent).key, detail: (event as MouseEvent).detail,
          target: target?.className, active: document.activeElement?.className, focused: document.hasFocus(),
          label: player?.querySelector(".video-sound")?.textContent,
          muted: video?.muted, paused: video?.paused, sound: localStorage.getItem("allthing.sound"),
        });
        if (events.length > 64) events.shift();
      });
    };
    for (const kind of ["pointerdown", "pointerup", "touchend", "keydown", "keypress", "keyup", "click", "focus", "blur", "volumechange", "playing", "pause"]) {
      window.addEventListener(kind, record, true);
    }
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
  test.describe(`sound focus at width ${width}`, () => {
    test.use({ viewport: { width, height: 844 }, hasTouch: width === 390 });
    test(`sound controls keep the video in view and preserve focus at width ${width}`, async ({ page, hasTouch }, info) => {
      await page.goto(`/p/${hash}`);
      const player = page.locator(".post .video");
      const video = player.locator("video");
      await expect.poll(() => video.evaluate((v) => v.muted && !v.paused), { timeout: 15000 }).toBe(true);
      const sound = player.getByRole("button", { name: "Sound off · tap for sound", exact: true });
      await expect(sound).toBeVisible();
      // On a touch screen, a tap on the video shows the badge first.
      if (hasTouch) {
        await activateVisible(video, true);
        await expect.poll(() => sound.evaluate((b) => getComputedStyle(b).opacity)).toBe("1");
      }
      const before = await page.evaluate(() => window.scrollY);
      const source = await video.getAttribute("src");
      await activateVisible(sound, hasTouch);
      let failed = true;
      try {
        await expect.poll(() => page.evaluate(() => window.scrollY), { timeout: 2000 }).toBe(before);
        const enabled = player.getByRole("button", { name: "Sound on · tap to mute", exact: true });
        await expect(enabled).toBeFocused();
        await expect.poll(() => video.evaluate((v) => !v.muted && !v.paused)).toBe(true);
        expect(await page.evaluate(() => localStorage.getItem("allthing.sound"))).toBe("on");
        // Hold Space on the pointer-focused control until its keydown has
        // been processed, then release it to activate once.
        // Do not refocus: the same node must keep focus throughout.
        const pointerFocused = await enabled.elementHandle();
        await page.keyboard.down("Space");
        try {
          await expect(enabled).toBeFocused();
          expect(await enabled.evaluate((button, original) => button === original, pointerFocused)).toBe(true);
          await expect(enabled).toHaveAttribute("data-press", "down");
          await expect.poll(() => video.evaluate((v) => !v.muted && !v.paused)).toBe(true);
          // Repeated keydown must not toggle while Space remains held.
          await page.keyboard.down("Space");
          await expect(enabled).toBeFocused();
          await expect.poll(() => video.evaluate((v) => !v.muted && !v.paused)).toBe(true);
        } finally {
          await page.keyboard.up("Space");
        }
        await expect(sound).toBeFocused();
        await expect.poll(() => video.evaluate((v) => v.muted && !v.paused)).toBe(true);
        expect(await page.evaluate(() => localStorage.getItem("allthing.sound"))).toBeNull();
        expect(await page.evaluate(() => window.scrollY)).toBe(before);
        expect(await video.getAttribute("src")).toBe(source);
        // Enter retains the native button action, and another Space release
        // toggles once on the same pointer-focused node.
        await page.keyboard.press("Enter");
        await expect(enabled).toBeFocused();
        await expect.poll(() => video.evaluate((v) => !v.muted && !v.paused)).toBe(true);
        await page.keyboard.press("Space");
        await expect(sound).toBeFocused();
        await expect.poll(() => video.evaluate((v) => v.muted && !v.paused)).toBe(true);
        expect(await page.evaluate(() => localStorage.getItem("allthing.sound"))).toBeNull();
        expect(await page.evaluate(() => window.scrollY)).toBe(before);
        expect(await video.getAttribute("src")).toBe(source);
        failed = false;
      } finally {
        if (failed) {
          await info.attach("sound-input", { contentType: "application/json", body: JSON.stringify(await page.evaluate(() => (window as any).soundInput), null, 2) });
        }
        await page.screenshot({ path: info.outputPath(`video-sound-focus-${width}.png`) });
      }
    });
  });
}
