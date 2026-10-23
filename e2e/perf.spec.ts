// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { perfDiagnosticsProbe, perfResponses, readPerfDiagnostics } from "./helpers/perf-diagnostics";

/*
 * Performance budget (docs/PERFORMANCE.md), measured in the browser:
 *
 *   - LCP of the home page, cold: < 1.5 s (median of 3 loads)
 *   - opening a post from the feed: < 100 ms from click to the post's text
 *   - INP-style interaction latency (Event Timing): < 100 ms
 *   - the cron's stored copy of the newest casts: built < 150 s ago, with no
 *     reader needed to refresh it
 *
 * CI runs this file against https://allthing.social after each push to main:
 *   E2E_BASE_URL=https://allthing.social npx playwright test e2e/perf.spec.ts
 */

const LCP_BUDGET = 1500;
const OPEN_BUDGET = 100;
const INP_BUDGET = 100;

const STORED_NEW_BUDGET_S = 150;

const median = (values: number[]) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)];

/** Records when the first real cast card is in the DOM, from navigation start:
 * the app's, or one drawn before it came (web/paint.js). */
const firstCastProbe = () => {
  new MutationObserver((_, observer) => {
    if (document.querySelector(".cast-card:not(.skeleton)")) {
      (window as any).__firstCast = performance.now();
      observer.disconnect();
    }
  }).observe(document, { childList: true, subtree: true });
};

async function lcp(page: Page): Promise<number> {
  return page.evaluate(
    () =>
      new Promise<number>((resolve) => {
        let latest = 0;
        new PerformanceObserver((list) => {
          for (const entry of list.getEntries()) latest = entry.startTime;
        }).observe({ type: "largest-contentful-paint", buffered: true });
        setTimeout(() => resolve(latest), 500);
      }),
  );
}

test.describe("performance budget", () => {
  test("home LCP stays under 1.5 s (cold, median of 3)", async ({ browser }, testInfo) => {
    const samples: number[] = [];
    const diagnostics: unknown[] = [];
    try {
      for (let i = 0; i < 3; i++) {
        const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
        try {
          const page = await context.newPage();
          const responses = perfResponses(page);
          await page.addInitScript(firstCastProbe);
          await page.addInitScript(perfDiagnosticsProbe);
          try {
            await page.goto("/");
            await expect(page.locator("#app .cast-card:not(.skeleton)").first()).toBeVisible();
            await page.waitForTimeout(300);
            // Keep the existing budget: the later of browser LCP and first cast.
            const firstCast = await page.evaluate(() => (window as any).__firstCast as number);
            const browserLcp = await lcp(page);
            const value = Math.max(browserLcp, firstCast);
            samples.push(value);
            const detail = { sample: i + 1, value, firstCast, browserLcp, ...await readPerfDiagnostics(page, responses) };
            diagnostics.push(detail);
            console.log(`LCP diagnostic ${JSON.stringify(detail)}`);
          } catch (error) {
            // Capture the last pipeline state even if readiness itself fails.
            const detail = { sample: i + 1, incomplete: true, ...await readPerfDiagnostics(page, responses).catch(() => ({ responses })) };
            diagnostics.push(detail);
            console.log(`LCP diagnostic ${JSON.stringify(detail)}`);
            throw error;
          }
        } finally { await context.close(); }
      }
    } finally {
      await testInfo.attach("home-lcp-diagnostics.json", { body: JSON.stringify({ budget: LCP_BUDGET, samples: diagnostics }, null, 2), contentType: "application/json" });
    }
    const value = median(samples);
    console.log(`LCP samples ${samples.map((s) => s.toFixed(0)).join(", ")} ms; median ${value.toFixed(0)} ms`);
    expect(value).toBeLessThan(LCP_BUDGET);
  });

  test("opening a post from the feed takes under 100 ms", async ({ page }) => {
    await page.goto("/");
    // A card with plain text to click (.cast-title: releases before full text).
    const card = page.locator("#app .cast-card:not(.skeleton):has(.cast-text, .cast-title):not(:has(.cast-text a))").nth(2);
    await expect(card).toBeVisible();
    await card.hover(); // prefetches the post's data
    await page.waitForTimeout(600);
    await page.evaluate(() => {
      (window as any).__openedIn = null;
      document.addEventListener(
        "click",
        () => {
          const t0 = performance.now();
          const check = () =>
            document.querySelector(".post .post-text, .post .mini-app-note")
              ? ((window as any).__openedIn = performance.now() - t0)
              : requestAnimationFrame(check);
          requestAnimationFrame(check);
        },
        { capture: true, once: true },
      );
    });
    await card.locator(".cast-text, .cast-title").first().click({ position: { x: 4, y: 8 } });
    await expect.poll(() => page.evaluate(() => (window as any).__openedIn)).not.toBeNull();
    const opened = await page.evaluate(() => (window as any).__openedIn as number);
    // With prefetched data, the replies are in place too.
    await expect(page.locator(".replies .comments, .replies .empty").first()).toBeVisible({ timeout: 5000 });
    console.log(`post opened in ${opened.toFixed(0)} ms`);
    expect(opened).toBeLessThan(OPEN_BUDGET);
  });

  test("interactions respond within 100 ms (Event Timing)", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("#app .cast-card:not(.skeleton)").first()).toBeVisible();
    // Interactions after the page has settled (first images and prefetches).
    await page.waitForLoadState("networkidle").catch(() => {});
    await page.waitForTimeout(1000);
    await page.evaluate(() => {
      (window as any).__events = [] as number[];
      new PerformanceObserver((list) => {
        // Interactions only (clicks, taps, keys), as INP counts them.
        for (const entry of list.getEntries()) if ((entry as any).interactionId) (window as any).__events.push(entry.duration);
        }).observe({ type: "event", durationThreshold: 16, buffered: false } as PerformanceObserverInit);
    });
    await page.locator("#app .sort-tabs a", { hasText: "new" }).click();
    await page.locator("#app .sort-tabs a", { hasText: "top" }).click();
    await page.locator("#app .cast-card:not(.skeleton)").first().locator(".stat-likes .stat-count").first().focus();
    await page.keyboard.press("Escape");
    await page.waitForTimeout(500);
    const worst = await page.evaluate(() => Math.max(0, ...((window as any).__events as number[])));
    console.log(`slowest interaction ${worst.toFixed(0)} ms`);
    expect(worst).toBeLessThan(INP_BUDGET);
  });
});

// The cron rebuilds and stores "new" every minute, so a data center with no
// recent list of its own serves a copy at most about a minute old. /status
// reads that copy and makes no upstream call, so this doesn't refresh it.
test("the cron keeps the stored copy of the newest casts fresh", async ({ request }) => {
  const response = await request.get("/api/v1/status");
  expect(response.ok()).toBe(true);
  const { freshness } = await response.json();
  test.skip(!("new_stored" in freshness), "the release live now stores no copy (deploys race this check)");
  expect(freshness.new_stored, "the cron has stored a copy").not.toBeNull();
  expect(freshness.new_stored.built_age_s).toBeLessThan(STORED_NEW_BUDGET_S);
  expect(freshness.new_stored.casts).toBeGreaterThan(0);
});
