// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import path from "node:path";
import { NOW } from "./fixture-time";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const stream = "https://mse.example/master.m3u8";
const hash = `0x${"a".repeat(40)}`;

async function serve(page: Page, blocked = "") {
  let release!: () => void;
  const pending = new Promise<void>((resolve) => { release = resolve; });
  const meta = { backend: "hypersnap", fetched_at: Date.now() };
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture" } }));
  await page.route("**/api/v1/home**", (route) => route.fulfill({ json: { pool: [], meta } }));
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
    thread: { cast: { hash, author: { fid: 1, username: "fixture" }, text: "MSE lifecycle fixture",
      ts: (NOW - 19), embeds: [{ kind: "video", url: stream }] } }, meta,
  } }));
  await page.route("https://mse.example/**", async (route) => {
    const file = new URL(route.request().url()).pathname.slice(1);
    if (file === blocked) await pending;
    await route.fulfill({ body: readFileSync(path.join(__dirname, "fixtures/hls", file)),
      contentType: file.endsWith("m3u8") ? "application/vnd.apple.mpegurl" : "video/mp4",
      headers: { "access-control-allow-origin": "*" } }).catch(() => {});
  });
  return release;
}

// Hold the actual player at an event boundary. These EventTargets never
// dispatch the chosen event, so removing their listeners is observable.
async function holdStage(page: Page, stage: string) {
  await page.addInitScript((stage) => {
    const state = { listeners: [] as { target: EventTarget; type: string; callback: unknown; active: boolean }[] };
    (window as any).__mse = state;
    class Buffer extends EventTarget {
      updating = false;
      buffered = { length: 0 };
      appendBuffer() { if (stage !== "updateend") queueMicrotask(() => this.dispatchEvent(new Event("updateend"))); }
      remove() { queueMicrotask(() => this.dispatchEvent(new Event("updateend"))); }
    }
    class Source extends EventTarget {
      static isTypeSupported() { return true; }
      readyState = stage === "sourceopen" ? "closed" : "open";
      duration = 0;
      addSourceBuffer() { return new Buffer(); }
      endOfStream() {}
    }
    (window as any).MediaSource = Source;
    const create = URL.createObjectURL.bind(URL);
    URL.createObjectURL = (value) => create(value instanceof Source ? new Blob() : value);
    const canPlay = HTMLMediaElement.prototype.canPlayType;
    HTMLMediaElement.prototype.canPlayType = function (type) {
      return type.includes("mpegurl") ? "" : canPlay.call(this, type);
    };
    const add = EventTarget.prototype.addEventListener, remove = EventTarget.prototype.removeEventListener;
    EventTarget.prototype.addEventListener = function (type, callback, options) {
      if ((this instanceof Source || this instanceof Buffer) && ["sourceopen", "updateend"].includes(type)) {
        state.listeners.push({ target: this, type, callback, active: true });
      }
      return add.call(this, type, callback, options);
    };
    EventTarget.prototype.removeEventListener = function (type, callback, options) {
      for (const listener of state.listeners) {
        if (listener.target === this && listener.type === type && listener.callback === callback) listener.active = false;
      }
      return remove.call(this, type, callback, options);
    };
  }, stage);
}

for (const stage of ["sourceopen", "updateend"]) {
  test(`leaving an MSE player removes a pending ${stage} listener`, async ({ page }) => {
    await holdStage(page, stage);
    await serve(page);
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto(`/p/${hash}`);
    await page.locator(".video-play").click();
    const waiting = () => page.evaluate((stage) => (window as any).__mse.listeners
      .filter((listener: any) => listener.type === stage && listener.active).length, stage);
    await expect.poll(waiting).toBe(1);
    await page.getByRole("link", { name: "Allthing, home", exact: true }).click();
    await expect(page.locator("video")).toHaveCount(0);
    await expect.poll(waiting, { timeout: 2000 }).toBe(0);
    expect(errors).toEqual([]);
  });
}

for (const [stage, file] of [["playlist", "stream_0.m3u8"], ["segment", "seg_0_0.m4s"]]) {
  test(`leaving an MSE player aborts its pending ${stage} fetch`, async ({ page }) => {
    await holdStage(page, stage);
    const release = await serve(page, file);
    let aborted = false;
    page.on("requestfailed", (request) => {
      if (request.url() === `https://mse.example/${file}`) aborted = true;
    });
    try {
      const requested = page.waitForRequest(`https://mse.example/${file}`);
      await page.goto(`/p/${hash}`);
      await page.locator(".video-play").click();
      await requested;
      await page.getByRole("link", { name: "Allthing, home", exact: true }).click();
      await expect(page.locator("video")).toHaveCount(0);
      await expect.poll(() => aborted, { timeout: 2000 }).toBe(true);
    } finally { release(); }
  });
}

for (const width of [1280, 390]) {
  test(`the real MSE player still decodes and advances the HLS fixture at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    // Some Chromium versions now play HLS natively. Exercise the Rust MSE
    // path explicitly rather than accidentally testing the native fallback.
    await page.addInitScript(() => {
      const canPlay = HTMLMediaElement.prototype.canPlayType;
      HTMLMediaElement.prototype.canPlayType = function (type) {
        return type.includes("mpegurl") ? "" : canPlay.call(this, type);
      };
    });
    await serve(page);
    await page.goto(`/p/${hash}`);
    await page.locator(".video-play").click();
    await expect(page.locator("video")).toHaveAttribute("src", /^blob:/);
    await expect.poll(() => page.locator("video").evaluate((video) => video.currentTime), { timeout: 15000 }).toBeGreaterThan(0.5);
    await expect(page.locator("video")).not.toHaveAttribute("data-error");
  });
}
