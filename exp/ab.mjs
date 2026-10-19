// A/B two builds of the app on live data: serve each build's page and assets
// locally, the API from production, alternating runs, with CPU throttling.
import { chromium } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";
const dir = "/tmp/ab";
const builds = ["base", "erase"];
const runs = Number(process.env.RUNS || 7);
const cpu = Number(process.env.CPU || 4);
const types = { ".js": "text/javascript", ".css": "text/css", ".wasm": "application/wasm", ".woff2": "font/woff2", ".svg": "image/svg+xml", ".png": "image/png", ".html": "text/html" };
const browser = await chromium.launch();
const results = {};
for (const width of [1280, 390]) {
  for (let i = 0; i < runs; i++) {
    for (const build of (i % 2 ? [...builds].reverse() : builds)) {
      const root = path.join(dir, build);
      const ctx = await browser.newContext({ viewport: { width, height: width === 390 ? 844 : 900 }, isMobile: width === 390, hasTouch: width === 390 });
      const page = await ctx.newPage();
      await page.route("https://allthing.social/**", async (route) => {
        const req = route.request();
        const url = new URL(req.url());
        if (req.resourceType() === "document") return route.fulfill({ contentType: "text/html", body: fs.readFileSync(path.join(root, "index.html")) });
        let file = null;
        if (url.pathname.startsWith("/assets/")) file = path.join(root, url.pathname);
        if (url.pathname.startsWith("/wasm/")) file = path.join(root, "assets", path.basename(url.pathname));
        if (file && fs.existsSync(file)) return route.fulfill({ contentType: types[path.extname(file)] || "application/octet-stream", body: fs.readFileSync(file) });
        return route.continue();
      });
      const cdp = await ctx.newCDPSession(page);
      await cdp.send("Emulation.setCPUThrottlingRate", { rate: cpu });
      await page.addInitScript(() => {
        const t = (window.__t = { events: [] });
        const wrap = (obj, name) => { const f = obj[name]; obj[name] = function (...a) { const p = f.apply(this, a); return Promise.resolve(p).then((v) => { t[name] ??= performance.now(); return v; }); }; };
        wrap(WebAssembly, "compile"); wrap(WebAssembly, "instantiate");
        new MutationObserver(() => { if (!t.firstCard && document.querySelector(".cast-card:not(.skeleton)")) t.firstCard = performance.now(); }).observe(document, { childList: true, subtree: true });
        new PerformanceObserver((list) => { for (const e of list.getEntries()) if (e.interactionId) t.events.push({ name: e.name, duration: e.duration }); }).observe({ type: "event", durationThreshold: 16, buffered: true });
        let lcp = 0; new PerformanceObserver((l) => { for (const e of l.getEntries()) lcp = e.startTime; t.lcp = lcp; }).observe({ type: "largest-contentful-paint", buffered: true });
        t.long = 0; new PerformanceObserver((l) => { for (const e of l.getEntries()) t.long += e.duration; }).observe({ type: "longtask", buffered: true });
      });
      await page.goto("https://allthing.social/", { waitUntil: "commit" });
      await page.waitForSelector(".cast-card:not(.skeleton)", { timeout: 90000 });
      await page.waitForTimeout(1500);
      const boot = await page.evaluate(() => { const d = performance.getEntriesByType("resource").find((x) => /\/api\/v1\/home/.test(x.name)); const t = window.__t; return { compiled: t.compile, instantiated: t.instantiate, dataEnd: d?.responseEnd, firstCard: t.firstCard, lcp: t.lcp, long: t.long }; });
      // An interaction: switch to "New", then back to "Hot".
      const tabs = page.locator(".sort-row a, .sort-tabs a");
      const before = (await page.evaluate(() => window.__t.events.length));
      await tabs.filter({ hasText: /^new$/i }).first().click();
      await page.waitForTimeout(1500);
      await tabs.filter({ hasText: /^hot$/i }).first().click();
      await page.waitForTimeout(1500);
      const events = await page.evaluate((n) => window.__t.events.slice(n).filter((e) => e.name === "click" || e.name === "pointerup" || e.name === "pointerdown"), before);
      const inp = Math.max(0, ...events.map((e) => e.duration));
      const key = `${width} ${build}`;
      (results[key] ||= []).push({ compile: Math.round(boot.compiled - 0), afterData: Math.round(boot.firstCard - Math.max(boot.dataEnd, boot.instantiated)), instantiateToCard: Math.round(boot.firstCard - boot.instantiated), lcp: Math.round(boot.lcp), long: Math.round(boot.long), inp: Math.round(inp) });
      await ctx.close();
    }
  }
}
const med = (a) => { const s = [...a].sort((x, y) => x - y); return s[Math.floor(s.length / 2)]; };
for (const [key, rows] of Object.entries(results)) {
  const m = Object.fromEntries(Object.keys(rows[0]).map((k) => [k, med(rows.map((r) => r[k]))]));
  console.log(key.padEnd(11), JSON.stringify(m), " runs:", rows.map((r) => r.instantiateToCard).join(","));
}
await browser.close();
