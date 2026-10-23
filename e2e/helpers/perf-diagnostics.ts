// SPDX-License-Identifier: AGPL-3.0-or-later
import type { Page } from "@playwright/test";

/** URLs in reports never retain credentials, queries, fragments or data bodies. */
export function safePerfUrl(raw: string): string {
  try {
    const url = new URL(raw);
    return /^https?:$/.test(url.protocol) ? url.origin + url.pathname : `${url.protocol}[omitted]`;
  } catch { return "[invalid URL]"; }
}

/** Observe without warming, blocking, fetching or changing browser priorities. */
export function perfDiagnosticsProbe() {
  const state = ((window as any).__allthingPerfDiagnostics = { lcp: null as any, appCast: 0, longtasks: [] as any[] });
  new MutationObserver((_, observer) => {
    if (document.querySelector("#app .cast-card:not(.skeleton)") && !document.querySelector("#boot[data-painted]")) {
      state.appCast = performance.now();
      observer.disconnect();
    }
  }).observe(document, { childList: true, subtree: true });
  new PerformanceObserver((list) => {
    for (const entry of list.getEntries() as any[]) {
      const element = entry.element as HTMLImageElement | null;
      state.lcp = {
        time: entry.startTime, render: entry.renderTime, load: entry.loadTime, size: entry.size, url: entry.url,
        element: element ? { tag: element.tagName, classes: element.getAttribute("class"), parentClasses: element.parentElement?.getAttribute("class"), loading: element.getAttribute("loading"), priority: element.getAttribute("fetchpriority"), width: element.getAttribute("width"), height: element.getAttribute("height"), naturalWidth: element.naturalWidth, naturalHeight: element.naturalHeight } : null,
      };
    }
  }).observe({ type: "largest-contentful-paint", buffered: true });
  new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) {
      if (state.longtasks.length < 100) state.longtasks.push({ start: entry.startTime, duration: entry.duration });
    }
  }).observe({ type: "longtask", buffered: true });
}

/** Descriptions can contain URLs; retain only metric names and numeric durations. */
export function perfServerTiming(raw?: string) {
  // A quoted description may contain commas or strings resembling dur=.
  const parts = (raw ?? "").match(/(?:[^,"]|"(?:\\.|[^"\\])*")+/g) ?? [];
  return parts.flatMap((part) => {
    const name = part.trim().match(/^([A-Za-z0-9_-]+)/)?.[1];
    if (!name) return [];
    const parameters = part.replace(/"(?:\\.|[^"\\])*"/g, '""');
    const duration = Number(parameters.match(/;\s*dur=([0-9.]+)/)?.[1] ?? 0);
    return [{ name, duration: Number.isFinite(duration) ? duration : 0 }];
  });
}

type ResponseTiming = { url: string; status: number; cache?: string; cfCache?: string; age?: string; serverTiming: ReturnType<typeof perfServerTiming>; colo?: string };

/** A bounded metadata log, read from the response event's existing headers. */
export function perfResponses(page: Page): ResponseTiming[] {
  const log: ResponseTiming[] = [];
  page.on("response", (response) => {
    if (log.length >= 256) return;
    const headers = response.headers();
    const colo = headers["cf-ray"]?.split("-").pop();
    log.push({ url: safePerfUrl(response.url()), status: response.status(), cache: headers["x-allthing-cache"], cfCache: headers["cf-cache-status"], age: headers.age, serverTiming: perfServerTiming(headers["server-timing"]), colo: colo && /^[A-Z]{3}$/.test(colo) ? colo : undefined });
  });
  return log;
}

/** Resource phases use navigation-relative milliseconds; zero can mean missing TAO. */
export async function readPerfDiagnostics(page: Page, responses: ResponseTiming[]) {
  const sample = await page.evaluate(() => {
    const state = (window as any).__allthingPerfDiagnostics;
    const navigation = performance.getEntriesByType("navigation")[0] as PerformanceNavigationTiming;
    const resources = performance.getEntriesByType("resource") as PerformanceResourceTiming[];
    const resource = (entry: PerformanceResourceTiming) => ({
      url: entry.name, kind: entry.initiatorType, start: entry.startTime, request: entry.requestStart,
      response: entry.responseStart, end: entry.responseEnd, duration: entry.duration,
      transferBytes: entry.transferSize, encodedBytes: entry.encodedBodySize, decodedBytes: entry.decodedBodySize,
      serverTiming: entry.serverTiming.map(({ name, duration }) => ({ name, duration })),
    });
    return {
      navigation: { start: navigation.startTime, dnsStart: navigation.domainLookupStart, dnsEnd: navigation.domainLookupEnd, connectStart: navigation.connectStart, tlsStart: navigation.secureConnectionStart, connectEnd: navigation.connectEnd, request: navigation.requestStart, response: navigation.responseStart, end: navigation.responseEnd, dom: navigation.domContentLoadedEventEnd, load: navigation.loadEventEnd },
      lcp: state.lcp,
      appCast: state.appCast,
      lcpResource: resources.find((entry) => entry.name === state.lcp?.url) ? resource(resources.find((entry) => entry.name === state.lcp.url)!) : null,
      resources: resources.filter((entry) => new URL(entry.name).origin === location.origin && /\/api\/v1\/home|\.wasm|\.(?:js|css|woff2)(?:\?|$)/.test(entry.name)).map(resource),
      longtasks: state.longtasks,
    };
  });
  const cleanResource = (entry: NonNullable<typeof sample.lcpResource>) => ({ ...entry, url: safePerfUrl(entry.url) });
  return {
    ...sample,
    lcp: sample.lcp ? { ...sample.lcp, url: sample.lcp.url ? safePerfUrl(sample.lcp.url) : "" } : null,
    lcpResource: sample.lcpResource ? cleanResource(sample.lcpResource) : null,
    resources: sample.resources.map(cleanResource),
    responses,
  };
}
