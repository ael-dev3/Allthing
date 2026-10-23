// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";
import { perfDiagnosticsProbe, perfResponses, readPerfDiagnostics, safePerfUrl } from "./helpers/perf-diagnostics";

test("cold-load diagnostics retain LCP, resource phases and cache evidence without URL credentials or queries", async ({ page }) => {
  expect(safePerfUrl("https://reader:password@example.com:8443/a/b?token=secret#private")).toBe("https://example.com:8443/a/b");
  expect(safePerfUrl("data:image/png;base64,private")).toBe("data:[omitted]");
  expect(safePerfUrl("not a URL")).toBe("[invalid URL]");
  const responses = perfResponses(page);
  await page.addInitScript(perfDiagnosticsProbe);
  await page.route("**/__perfdiag?*", (route) => route.fulfill({ contentType: "text/html", body: '<!doctype html><link rel="stylesheet" href="/__perfdiag.css?css-secret"><img src="/__perfdiag.png?image-secret" width="320" height="250"><div id="app"><p class="cast-card">A cold-load diagnostic fixture</p></div>' }));
  await page.route("**/__perfdiag.css?*", (route) => route.fulfill({ contentType: "text/css", body: "body{margin:0}img{display:block}" }));
  await page.route("**/__perfdiag.png?*", (route) => route.fulfill({ contentType: "image/png", body: readFileSync("web/pfp.png") }));
  await page.route("**/api/v1/home?*", (route) => route.fulfill({ json: { pool: [] }, headers: { "x-allthing-cache": "fresh", "cf-cache-status": "HIT", "cf-ray": "fixture-BEG", "server-timing": 'fresh;dur=3;desc="https://user:private@example.com/?header-secret"' } }));
  await page.goto("/__perfdiag?document-secret");
  await page.evaluate(async () => { await (await fetch("/api/v1/home?api-secret")).json(); });
  await expect.poll(() => page.evaluate(() => (window as any).__allthingPerfDiagnostics.lcp?.element?.tag), { timeout: 5000 }).toBe("IMG");
  // A real main-thread pause: its observation must survive in the report.
  await page.evaluate(() => { setTimeout(() => { const end = performance.now() + 80; while (performance.now() < end) {} }, 0); });
  await expect.poll(() => page.evaluate(() => (window as any).__allthingPerfDiagnostics.longtasks.length), { timeout: 5000 }).toBeGreaterThan(0);
  const result = await readPerfDiagnostics(page, responses);
  expect(result.lcp.time).toBeGreaterThan(0);
  expect(result.appCast).toBeGreaterThan(0);
  expect(result.lcp.element.tag).toBe("IMG");
  expect(result.lcpResource?.url).toMatch(/\/__perfdiag\.png$/);
  expect(result.navigation.response).toBeGreaterThan(0);
  expect(result.resources.some((resource) => resource.url.endsWith("/api/v1/home") && resource.end >= resource.start)).toBe(true);
  expect(result.responses.find((response) => response.url.endsWith("/api/v1/home"))).toMatchObject({ cache: "fresh", cfCache: "HIT", colo: "BEG", serverTiming: [{ name: "fresh", duration: 3 }] });
  expect(result.longtasks.some((task: { duration: number }) => task.duration >= 50)).toBe(true);
  const serialized = JSON.stringify(result);
  expect(serialized).not.toMatch(/secret|password|private|reader:/);
});
