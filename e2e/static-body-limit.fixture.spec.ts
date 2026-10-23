// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { isoAgo } from "./fixture-time";

test.skip(!process.env.E2E_PAGES, "requires the static-host payload builder");

async function provider(page: Page, mode: string) {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript((mode) => {
    const stats = { requested: false, pulls: 0, cancelled: false, aborted: false, errored: false };
    (window as any).__body = stats;
    const nativeFetch = window.fetch.bind(window);
    window.fetch = (input, init) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      if (!url.startsWith("https://cap.example/")) return nativeFetch(input, init);
      stats.requested = true;
      const encoder = new TextEncoder();
      const size = mode === "large-chunk" ? 4 * 131072 : 65536;
      const chunk = new Uint8Array(size).fill(32);
      if (mode === "large-chunk") {
        chunk.set(encoder.encode("<html><head>"));
        chunk.set(encoder.encode("<meta name='fc:frame' content='vNext'></head>"), size - 100);
      } else if (mode === "error") {
        // The head hasn't ended when the body fails, so the read goes on to
        // the failing pull: a read stops at `</head>`.
        chunk.set(encoder.encode("<html><head><meta name='fc:frame' content='vNext'>"));
      } else {
        chunk.set(encoder.encode("<html><head><meta name='fc:frame' content='vNext'></head>"));
      }
      let controller: ReadableStreamDefaultController<Uint8Array>;
      const body = new ReadableStream<Uint8Array>({
        start(c) { controller = c; },
        pull(c) {
          stats.pulls++;
          if (mode === "error" && stats.pulls > 1) {
            stats.errored = true;
            c.error(new Error("Fixture body failure"));
            return;
          }
          if (stats.pulls <= (mode === "large-chunk" ? 1 : 3)) c.enqueue(chunk);
          else return new Promise<void>(() => {});
        },
        cancel() { stats.cancelled = true; },
      });
      (window as any).__stream = body;
      // Model the fetch signal's effect on a stalled body as well. Reaching
      // the byte cap should cancel first, without waiting for this timeout.
      const signal = input instanceof Request ? input.signal : init?.signal;
      signal?.addEventListener("abort", () => {
        stats.aborted = true;
        if (!stats.cancelled) controller.error(new DOMException("Aborted", "AbortError"));
      }, { once: true });
      return Promise.resolve(new Response(body, { status: 200 }));
    };
  }, mode);
  await page.route("https://haatz.quilibrium.com/**", (route) => {
    const url = new URL(route.request().url());
    if (!url.pathname.endsWith("/feed/trending")) {
      return route.fulfill({ status: 404, json: {}, headers: { "access-control-allow-origin": "*" } });
    }
    const cast = { hash: `0x${"a".repeat(40)}`, author: { fid: 1, username: "fixture" },
      text: "Static body fixture", timestamp: isoAgo(3_600),
      embeds: mode === "json" ? [] : [{ url: `https://cap.example/${mode}` }] };
    return route.fulfill({ json: { casts: url.searchParams.has("cursor") ? [] : [cast],
      // Uncapped JSON must still be read completely before parsing.
      ...(mode === "json" ? { padding: "x".repeat(200000) } : {}) },
      headers: { "access-control-allow-origin": "*" } });
  });
  await page.goto("/Allthing/");
  return errors;
}

test("a capped link body stops reading and cancels before a stalled tail", async ({ page }) => {
  const errors = await provider(page, "chunks");
  await expect.poll(() => page.evaluate(() => (window as any).__body.cancelled), { timeout: 2000 }).toBe(true);
  await expect(page.getByText("Nothing here yet.", { exact: true })).toBeVisible();
  const stats = await page.evaluate(() => (window as any).__body);
  expect(stats.requested).toBe(true);
  expect(stats.aborted).toBe(false);
  expect(stats.pulls).toBeLessThanOrEqual(3);
  expect(await page.evaluate(() => (window as any).__stream.locked)).toBe(false);
  expect(errors).toEqual([]);
});

test("a chunk larger than the limit is truncated before metadata parsing", async ({ page }) => {
  const errors = await provider(page, "large-chunk");
  await expect(page.getByText("Static body fixture", { exact: true })).toBeVisible();
  const stats = await page.evaluate(() => (window as any).__body);
  expect(stats.cancelled).toBe(true);
  expect(stats.aborted).toBe(false);
  expect(errors).toEqual([]);
});

test("ordinary JSON responses larger than the link limit remain complete", async ({ page }) => {
  const errors = await provider(page, "json");
  await expect(page.getByText("Static body fixture", { exact: true })).toBeVisible();
  expect(errors).toEqual([]);
});

test("a failed body read releases its reader without an unhandled rejection", async ({ page }) => {
  const errors = await provider(page, "error");
  await expect(page.getByText("Static body fixture", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__body.errored)).toBe(true);
  expect(await page.evaluate(() => (window as any).__stream.locked)).toBe(false);
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  expect(errors).toEqual([]);
});
