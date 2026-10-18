// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import path from "node:path";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const stream = "https://renditions.example/master.m3u8";
const hash = `0x${"a".repeat(40)}`;

async function serve(page: Page, upgrade: boolean) {
  await page.addInitScript(() => {
    const canPlay = HTMLMediaElement.prototype.canPlayType;
    HTMLMediaElement.prototype.canPlayType = function (type) {
      return type.includes("mpegurl") ? "" : canPlay.call(this, type);
    };
  });
  const requests: string[] = [];
  const meta = { backend: "hypersnap", fetched_at: Date.now() };
  const fixture = path.join(__dirname, "fixtures/hls");
  let master = readFileSync(path.join(fixture, "master.m3u8"), "utf8");
  if (upgrade) master = master.replace("320x180", "854x480").replace("stream_0.m3u8", "stream_0.m3u8?rendition=480") +
    '\n#EXT-X-STREAM-INF:BANDWIDTH=500000,RESOLUTION=1280x720,CODECS="vp09.00.10.08,opus",AUDIO="group_aud"\nstream_0.m3u8?rendition=720\n';
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture" } }));
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
    thread: { cast: { hash, author: { fid: 1, username: "fixture" }, text: "Rendition fixture", ts: 1792235681,
      embeds: [{ kind: "video", url: stream }] } }, meta,
  } }));
  await page.route("https://renditions.example/**", (route) => {
    const url = new URL(route.request().url());
    requests.push(url.pathname + url.search);
    return route.fulfill({ body: url.pathname === "/master.m3u8" ? master : readFileSync(path.join(fixture, url.pathname.slice(1))),
      contentType: url.pathname.endsWith("m3u8") ? "application/vnd.apple.mpegurl" : "video/mp4",
      headers: { "access-control-allow-origin": "*" } });
  });
  await page.goto(`/p/${hash}`);
  await expect(page.locator("video")).toHaveAttribute("src", /^blob:/);
  await expect.poll(() => page.locator("video").evaluate((video) => video.currentTime), { timeout: 15000 }).toBeGreaterThan(0.5);
  return requests;
}

for (const width of [1280, 390]) {
  test(`playing a low-resolution autoplay keeps its source and buffered frames at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    const requests = await serve(page, false);
    const source = await page.locator("video").getAttribute("src");
    await page.locator(".video-play").click();
    await expect(page.locator("video")).toHaveAttribute("src", source!, { timeout: 2000 });
    await expect.poll(() => page.locator("video").evaluate((video) => !video.paused && video.currentTime > 0.7)).toBe(true);
    expect(requests.filter((url) => url === "/stream_0.m3u8")).toHaveLength(1);
  });

  test(`a genuinely higher rendition still replaces the preview at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    const requests = await serve(page, true);
    const source = await page.locator("video").getAttribute("src");
    expect(requests).toContain("/stream_0.m3u8?rendition=480");
    await page.locator(".video-play").click();
    await expect(page.locator("video")).not.toHaveAttribute("src", source!);
    await expect.poll(() => requests.includes("/stream_0.m3u8?rendition=720")).toBe(true);
    await expect.poll(() => page.locator("video").evaluate((video) => !video.paused && video.currentTime > 0.7)).toBe(true);
  });
}
