// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Route } from "@playwright/test";
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

test("autoplay goes to the topmost video in view, and to the next once it scrolls away", async ({ page }) => {
  // Tall enough for the post's video and the reply's video to be in view together.
  await page.setViewportSize({ width: page.viewportSize()!.width, height: 1500 });
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
    thread: {
      cast: withVideo(hash, "The post", "https://media.example/post.m3u8"),
      children: [{ cast: withVideo(`0x${"b".repeat(40)}`, "A reply", "https://media.example/reply.m3u8") }],
    },
    meta,
  } }));
  // The post's playlist arrives after the reply's, so the reply could start first.
  await page.route("https://media.example/**", async (route: Route) => {
    const name = new URL(route.request().url()).pathname.split("/").pop() ?? "";
    if (name === "post.m3u8") await new Promise((done) => setTimeout(done, 800));
    const file = name.endsWith(".m3u8") && !name.startsWith("stream_") ? "master.m3u8" : name;
    const contentType = file.endsWith(".m3u8") ? "application/vnd.apple.mpegurl" : "video/mp4";
    await route.fulfill({ body: readFileSync(path.join(fixtures, file)), contentType, headers: { "access-control-allow-origin": "*" } });
  });
  // Room below the thread to scroll the post away.
  await page.addInitScript(() => {
    document.addEventListener("DOMContentLoaded", () => {
      const style = document.createElement("style");
      style.textContent = ".replies { margin-bottom: 3000px; }";
      document.head.append(style);
    });
  });
  await page.goto(`/p/${hash}`);
  const post = page.locator(".post .video video");
  const reply = page.locator(".comment .video video");
  await expect(reply).toBeAttached();
  await expect.poll(() => post.evaluate((v: HTMLVideoElement) => !v.paused), { timeout: 15_000 }).toBe(true);
  // Give the reply's video every chance to take over: it must not.
  await page.waitForTimeout(1500);
  expect(await post.evaluate((v: HTMLVideoElement) => !v.paused), "the post's video plays").toBe(true);
  expect(await reply.evaluate((v: HTMLVideoElement) => v.paused), "the reply's video waits").toBe(true);

  // Scroll the post's video away: the reply's, still in view, plays.
  const top = await reply.evaluate((v) => v.getBoundingClientRect().top + window.scrollY);
  await page.evaluate((y) => window.scrollTo(0, y - 100), top);
  await expect.poll(() => reply.evaluate((v: HTMLVideoElement) => !v.paused), { timeout: 15_000 }).toBe(true);
  expect(await post.evaluate((v: HTMLVideoElement) => v.paused)).toBe(true);
});
