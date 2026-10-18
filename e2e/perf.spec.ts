// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

/*
 * Performance budget (docs/PERFORMANCE.md), measured in the browser:
 *
 *   - LCP of the home page, cold: < 1.5 s (median of 3 loads)
 *   - opening a post from the feed: < 100 ms from click to the post's text
 *   - INP-style interaction latency (Event Timing): < 100 ms
 *
 * CI runs this file against https://allthing.social after each push to main:
 *   E2E_BASE_URL=https://allthing.social npx playwright test e2e/perf.spec.ts
 */

const LCP_BUDGET = 1500;
const OPEN_BUDGET = 100;
const INP_BUDGET = 100;

const median = (values: number[]) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)];

/** Records when the first real cast card is in the DOM, from navigation start. */
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
  test("home LCP stays under 1.5 s (cold, median of 3)", async ({ browser }) => {
    const samples: number[] = [];
    for (let i = 0; i < 3; i++) {
      const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
      const page = await context.newPage();
      await page.addInitScript(firstCastProbe);
      await page.goto("/");
      await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
      await page.waitForTimeout(300);
      // The page's largest content is the feed: count it from when the first
      // cast appears, even if the browser's LCP entry picked something earlier.
      const firstCast = await page.evaluate(() => (window as any).__firstCast as number);
      samples.push(Math.max(await lcp(page), firstCast));
      await context.close();
    }
    const value = median(samples);
    console.log(`LCP samples ${samples.map((s) => s.toFixed(0)).join(", ")} ms; median ${value.toFixed(0)} ms`);
    expect(value).toBeLessThan(LCP_BUDGET);
  });

  test("opening a post from the feed takes under 100 ms", async ({ page }) => {
    await page.goto("/");
    // A card with plain text to click (.cast-title: releases before full text).
    const card = page.locator(".cast-card:not(.skeleton):has(.cast-text, .cast-title):not(:has(.cast-text a))").nth(2);
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
    await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
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
    await page.locator(".sort-tabs a", { hasText: "new" }).click();
    await page.locator(".sort-tabs a", { hasText: "top" }).click();
    await page.locator(".cast-card:not(.skeleton)").first().locator(".stat-likes").first().focus();
    await page.keyboard.press("Escape");
    await page.waitForTimeout(500);
    const worst = await page.evaluate(() => Math.max(0, ...((window as any).__events as number[])));
    console.log(`slowest interaction ${worst.toFixed(0)} ms`);
    expect(worst).toBeLessThan(INP_BUDGET);
  });
});
