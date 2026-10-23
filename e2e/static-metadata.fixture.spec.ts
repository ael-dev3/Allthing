// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";
import { isoAgo } from "./fixture-time";

test.skip(!process.env.E2E_PAGES, "uses the browser payload builder");

for (const width of [1280, 390]) {
  for (const realMetadata of [false, true]) {
    test(`fallback text ${realMetadata ? "followed by real metadata hides" : "alone preserves"} a linked cast at width ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 844 });
      const text = "An ordinary post linking a fallback example";
      const link = "https://fallback-page.example.test/";
      const errors: string[] = [];
      let reads = 0;
      page.on("pageerror", (error) => errors.push(error.message));
      await page.route("https://haatz.quilibrium.com/**", (route) => {
        const url = new URL(route.request().url());
        if (url.pathname.endsWith("/feed/trending")) {
          return route.fulfill({ json: { casts: url.searchParams.has("cursor") ? [] : [{
            hash: `0x${"a".repeat(40)}`, author: { fid: 1, username: "ordinary" }, text,
            timestamp: isoAgo(3_600), embeds: [{ url: link }],
          }] }, headers: { "access-control-allow-origin": "*" } });
        }
        return route.fulfill({ status: 404, json: {}, headers: { "access-control-allow-origin": "*" } });
      });
      await page.route(link, (route) => {
        reads++;
        return route.fulfill({ contentType: "text/html",
          body: `<html><head><noframes><meta name='fc:frame'></noframes>${realMetadata ? "<meta name='fc:frame'>" : ""}</head><body></body></html>`,
          headers: { "access-control-allow-origin": "*" } });
      });
      await page.goto("/Allthing/");
      if (realMetadata) {
        await expect(page.getByText("Nothing here yet.", { exact: true })).toBeVisible();
        await expect(page.getByText(text, { exact: true })).toHaveCount(0);
      } else {
        await expect(page.getByText(text, { exact: true })).toBeVisible();
      }
      expect(reads).toBe(1);
      expect(errors).toEqual([]);
    });
  }
}
