// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser } from "@playwright/test";

/*
 * Speed report (docs/PERFORMANCE.md#speed-report): cold loads of the front
 * page on a live site under fixed network and CPU profiles, for before/after
 * numbers. It reports and asserts nothing about the budget; the perf spec
 * does that. On demand only: CI's `speed` job runs it from a manual run.
 *
 *   E2E_SPEED=1 E2E_BASE_URL=https://allthing.social npx playwright test e2e/speed-report.spec.ts
 */

test.skip(!process.env.E2E_SPEED || !process.env.E2E_BASE_URL, "on demand, against a live site");

const RUNS = Number(process.env.E2E_SPEED_RUNS ?? 7);

/** A desktop on a 10 Mbps line, and a phone on "slow 4G" with its CPU slowed 4×. */
const profiles = [
  { width: 1280, height: 900, mobile: false, rttMs: 40, downKbps: 10_000, upKbps: 5_000, cpu: 1 },
  { width: 390, height: 844, mobile: true, rttMs: 150, downKbps: 1_600, upKbps: 750, cpu: 4 },
];

type Sample = { ttfb: number; wasmKB: number; wasm: number; data: number; firstCast: number; appCast: number; lcp: number; inp: number };

const median = (values: number[]) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)];

async function load(browser: Browser, profile: (typeof profiles)[number]): Promise<Sample> {
  const context = await browser.newContext({
    viewport: { width: profile.width, height: profile.height },
    isMobile: profile.mobile,
    hasTouch: profile.mobile,
  });
  const page = await context.newPage();
  const cdp = await context.newCDPSession(page);
  await cdp.send("Network.enable");
  await cdp.send("Network.emulateNetworkConditions", {
    offline: false,
    latency: profile.rttMs,
    downloadThroughput: (profile.downKbps * 1000) / 8,
    uploadThroughput: (profile.upKbps * 1000) / 8,
  });
  await cdp.send("Emulation.setCPUThrottlingRate", { rate: profile.cpu });
  await page.addInitScript(() => {
    const seen = ((window as any).__speed = { firstCast: 0, appCast: 0, lcp: 0, interactions: [] as number[] });
    // The first cast on screen, drawn early (web/paint.js) or by the app, and
    // the app's own first, which shows once the early ones have given way.
    new MutationObserver((_, observer) => {
      if (!seen.firstCast && document.querySelector(".cast-card:not(.skeleton)")) seen.firstCast = performance.now();
      if (!seen.appCast && document.querySelector("#app .cast-card:not(.skeleton)") && !document.querySelector("#boot[data-painted]")) {
        seen.appCast = performance.now();
        observer.disconnect();
      }
    }).observe(document, { childList: true, subtree: true });
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) seen.lcp = entry.startTime;
    }).observe({ type: "largest-contentful-paint", buffered: true });
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries() as PerformanceEventTiming[]) {
        if (entry.interactionId) seen.interactions.push(entry.duration);
      }
    }).observe({ type: "event", durationThreshold: 16, buffered: true } as PerformanceObserverInit);
  });
  await page.goto("/", { waitUntil: "commit" });
  await expect(page.locator("#app .cast-card:not(.skeleton)").first()).toBeVisible({ timeout: 120_000 });
  await page.waitForTimeout(1000);
  const sample = await page.evaluate(() => {
    const seen = (window as any).__speed;
    const nav = performance.getEntriesByType("navigation")[0] as PerformanceNavigationTiming;
    const resources = performance.getEntriesByType("resource") as PerformanceResourceTiming[];
    const wasm = resources.find((r) => r.name.includes(".wasm"));
    const data = resources.find((r) => r.name.includes("/api/v1/home"));
    return {
      ttfb: nav.responseStart,
      wasmKB: (wasm?.encodedBodySize ?? 0) / 1024,
      wasm: wasm?.responseEnd ?? 0,
      data: data?.responseEnd ?? 0,
      firstCast: seen.firstCast,
      appCast: seen.appCast,
      // As the perf spec counts it: the later of the LCP entry and the first card.
      lcp: Math.max(seen.lcp, seen.firstCast),
      inp: 0,
    };
  });
  // The slowest of two sort-tab switches, as Event Timing reports it.
  const before = await page.evaluate(() => (window as any).__speed.interactions.length);
  for (const tab of [/^new$/i, /^hot$/i]) {
    await page.locator("#app .sort-row a").filter({ hasText: tab }).first().click({ noWaitAfter: true });
    await page.waitForTimeout(1500);
  }
  sample.inp = await page.evaluate((from) => Math.max(0, ...(window as any).__speed.interactions.slice(from)), before);
  await context.close();
  return sample;
}

for (const profile of profiles) {
  test(`front page at ${profile.width} px`, async ({ browser, request }, testInfo) => {
    test.setTimeout(RUNS * 150_000);
    const deployed = async () => (await (await request.get("/version")).json()).commit as string;
    const commit = await deployed();
    const samples: Sample[] = [];
    for (let run = 0; run < RUNS; run++) samples.push(await load(browser, profile));
    // A deploy during the runs would mix two releases' numbers.
    expect(await deployed(), "the deployed commit changed during the runs").toBe(commit);
    const at = (key: keyof Sample) => median(samples.map((s) => s[key]));
    const network = `${profile.downKbps / 1000} Mbps, ${profile.rttMs} ms RTT, CPU ${profile.cpu}×`;
    console.log(
      `speed ${profile.width} px (${network}) on ${commit}, median of ${RUNS}: ` +
        `TTFB ${at("ttfb").toFixed(0)} ms, WebAssembly ${at("wasmKB").toFixed(1)} KB in by ${at("wasm").toFixed(0)} ms, ` +
        `data in by ${at("data").toFixed(0)} ms, first cast ${at("firstCast").toFixed(0)} ms, the app's ${at("appCast").toFixed(0)} ms, LCP ${at("lcp").toFixed(0)} ms, ` +
        `slowest interaction ${at("inp").toFixed(0)} ms`,
    );
    console.log(`  first cast samples: ${samples.map((s) => s.firstCast.toFixed(0)).join(", ")} ms`);
    await testInfo.attach(`speed-${profile.width}.json`, {
      body: JSON.stringify({ commit, profile, samples }, null, 2),
      contentType: "application/json",
    });
  });
}
