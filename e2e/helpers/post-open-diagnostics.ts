// SPDX-License-Identifier: AGPL-3.0-or-later
import type { Page } from "@playwright/test";
import { safePerfUrl } from "./perf-diagnostics";

export const POST_OPEN_LIMITS = { frames: 64, longtasks: 64, loafs: 32, scripts: 4, resources: 128 } as const;

/** Metadata only: no extra frame loop, DOM observer, request or page readiness. */
export function postOpenDiagnosticsProbe(limits: typeof POST_OPEN_LIMITS) {
  const state = ((window as any).__allthingPostOpenDiagnostics = {
    limits, hover: null as any, clickHash: null as string | null, click: null as number | null,
    opened: null as number | null, frames: [] as any[], longtasks: [] as any[], loafs: [] as any[],
    counts: { frames: 0, longtasks: 0, loafs: 0 }, available: { longtasks: false, loafs: false },
    observers: [] as PerformanceObserver[],
  });
  const hash = (target: EventTarget | null) => target instanceof Element
    ? target.closest(".cast-card[data-hash]")?.getAttribute("data-hash") ?? null : null;
  document.addEventListener("pointerover", (event) => {
    if (state.click === null && event.isTrusted && event.pointerType === "mouse") {
      const selected = hash(event.target);
      if (selected) state.hover = { at: performance.now(), hash: selected };
    }
  }, { capture: true });
  // This runs before the measurement's capture listener. Attribute lookup
  // therefore adds no work inside its unchanged click-to-text interval.
  document.addEventListener("click", (event) => {
    if (state.click === null && event.isTrusted) state.clickHash = hash(event.target);
  }, { capture: true, once: true });
  const consume = (kind: "longtasks" | "loafs", entries: PerformanceEntry[]) => {
    if (state.click === null) return;
    const end = state.opened === null ? Infinity : state.click + state.opened;
    for (const raw of entries as any[]) {
      if (raw.startTime + raw.duration < state.click || raw.startTime > end) continue;
      state.counts[kind]++;
      if (state[kind].length >= limits[kind]) continue;
      // Retain native entry references only. Projecting script metadata and
      // URLs is deferred until after the measured interaction/assertions.
      state[kind].push(raw);
    }
  };
  for (const [kind, type] of [["longtasks", "longtask"], ["loafs", "long-animation-frame"]] as const) {
    if (!PerformanceObserver.supportedEntryTypes.includes(type)) continue;
    state.available[kind] = true;
    const observer = new PerformanceObserver((list) => consume(kind, list.getEntries()));
    observer.observe({ type, buffered: false });
    (observer as any).__postOpenKind = kind;
    state.observers.push(observer);
  }
  state.consume = consume;
}

type RawPostOpen = {
  click: number | null; opened: number | null; collected: number;
  hover: { at: number; hash: string } | null; clickHash: string | null;
  frames: { at: number; shown: boolean }[];
  longtasks: { start: number; duration: number }[];
  loafs: { start: number; duration: number; blocking: number; render: number; layout: number; firstUI: number;
    scripts: { start: number; duration: number; execution: number; forcedLayout: number; pause: number; source: string; position: number }[] }[];
  resources: { url: string; kind: string; start: number; request: number; response: number; end: number; duration: number; transferBytes: number; encodedBytes: number; decodedBytes: number }[];
  counts: { frames: number; longtasks: number; loafs: number; scripts: number; resources: number };
  available: { longtasks: boolean; loafs: boolean };
};

/** Runs in Node, after the measured interaction and its original assertions. */
export function sanitizePostOpenDiagnostics(raw: RawPostOpen) {
  const relative = (at: number) => Number.isFinite(at) && raw.click !== null ? at - raw.click : null;
  const hash = (value?: string | null) => /^0x[0-9a-f]{40}$/i.test(value ?? "") ? value : null;
  const url = (value: string) => safePerfUrl(value).slice(0, 2048);
  const count = (observed: number, retained: number, limit: number) => ({ observed, retained, omitted: Math.max(0, observed - retained), limit });
  const frames = raw.frames.slice(0, POST_OPEN_LIMITS.frames).map((frame) => ({ at: relative(frame.at), shown: frame.shown }));
  const longtasks = raw.longtasks.slice(0, POST_OPEN_LIMITS.longtasks).map((task) => ({ start: relative(task.start), duration: task.duration }));
  const loafs = raw.loafs.slice(0, POST_OPEN_LIMITS.loafs).map((frame) => ({
    start: relative(frame.start), duration: frame.duration, blocking: frame.blocking,
    render: frame.render ? relative(frame.render) : null, layout: frame.layout ? relative(frame.layout) : null,
    firstUI: frame.firstUI ? relative(frame.firstUI) : null,
    scripts: frame.scripts.slice(0, POST_OPEN_LIMITS.scripts).map((script) => ({
      start: relative(script.start), duration: script.duration, execution: relative(script.execution),
      forcedLayout: script.forcedLayout, pause: script.pause, source: url(script.source), position: script.position,
    })),
  }));
  const resources = raw.resources.slice(0, POST_OPEN_LIMITS.resources).map((resource) => {
    const parsed = new URL(resource.url);
    return {
      url: url(resource.url), kind: resource.kind.slice(0, 32),
      selectedPost: !!raw.clickHash && parsed.pathname === "/api/v1/cast" && parsed.searchParams.get("hash") === raw.clickHash,
      start: relative(resource.start), request: resource.request ? relative(resource.request) : null,
      response: resource.response ? relative(resource.response) : null, end: relative(resource.end), duration: resource.duration,
      transferBytes: resource.transferBytes, encodedBytes: resource.encodedBytes, decodedBytes: resource.decodedBytes,
    };
  });
  return {
    version: 1, clock: "milliseconds relative to the original click capture t0", opened: raw.opened,
    resourceScope: "completed entries still in the browser timing buffer; in-flight or previously evicted entries are unavailable",
    collected: relative(raw.collected), hover: raw.hover ? { at: relative(raw.hover.at), hash: hash(raw.hover.hash) } : null,
    clickHash: hash(raw.clickHash), available: raw.available, frames, longtasks, loafs, resources,
    bounds: {
      frames: count(raw.counts.frames, frames.length, POST_OPEN_LIMITS.frames),
      longtasks: count(raw.counts.longtasks, longtasks.length, POST_OPEN_LIMITS.longtasks),
      loafs: count(raw.counts.loafs, loafs.length, POST_OPEN_LIMITS.loafs),
      scripts: { ...count(raw.counts.scripts, loafs.reduce((sum, frame) => sum + frame.scripts.length, 0), POST_OPEN_LIMITS.loafs * POST_OPEN_LIMITS.scripts), perFrameLimit: POST_OPEN_LIMITS.scripts, scope: "retained long-animation frames; omitted frames' scripts are unavailable" },
      resources: count(raw.counts.resources, resources.length, POST_OPEN_LIMITS.resources),
    },
  };
}

export async function readPostOpenDiagnostics(page: Page) {
  const raw = await page.evaluate(() => {
    const state = (window as any).__allthingPostOpenDiagnostics;
    if (!state) return null;
    for (const observer of state.observers) {
      state.consume((observer as any).__postOpenKind, observer.takeRecords());
      observer.disconnect();
    }
    // Reading completed resource entries happens after the metric and all
    // assertions. Keep pre-click prefetch phases too, including split chunks.
    const kept: { entry: PerformanceResourceTiming; priority: number }[] = [];
    let observed = 0;
    for (const entry of performance.getEntriesByType("resource") as PerformanceResourceTiming[]) {
      const url = new URL(entry.name);
      if (url.origin !== location.origin || !(url.pathname === "/api/v1/cast" || /\.(?:wasm|js)$/.test(url.pathname))) continue;
      observed++;
      const priority = url.pathname === "/api/v1/cast" && state.clickHash && url.searchParams.get("hash") === state.clickHash
        ? 0 : /\/split_post\.(?:wasm|js)$/.test(url.pathname) ? 1 : 2;
      if (kept.length < state.limits.resources) kept.push({ entry, priority });
      else {
        // Keep the selected post and its split module even when earlier
        // generic code chunks have filled the diagnostic resource cap.
        const replace = kept.findIndex((item) => item.priority > priority);
        if (replace >= 0) kept[replace] = { entry, priority };
      }
    }
    const resources = kept.sort((a, b) => a.priority - b.priority || a.entry.startTime - b.entry.startTime).map(({ entry }) => ({
      url: entry.name, kind: entry.initiatorType, start: entry.startTime, request: entry.requestStart,
        response: entry.responseStart, end: entry.responseEnd, duration: entry.duration,
      transferBytes: entry.transferSize, encodedBytes: entry.encodedBodySize, decodedBytes: entry.decodedBodySize,
    }));
    let scripts = 0;
    const loafs = state.loafs.map((entry: any) => {
      scripts += entry.scripts?.length ?? 0;
      return {
        start: entry.startTime, duration: entry.duration, blocking: entry.blockingDuration,
        render: entry.renderStart, layout: entry.styleAndLayoutStart, firstUI: entry.firstUIEventTimestamp,
        scripts: (entry.scripts ?? []).slice(0, state.limits.scripts).map((script: any) => ({
          start: script.startTime, duration: script.duration, execution: script.executionStart,
          forcedLayout: script.forcedStyleAndLayoutDuration, pause: script.pauseDuration,
          source: String(script.sourceURL ?? "").slice(0, 4096), position: script.sourceCharPosition,
        })),
      };
    });
    return {
      click: state.click, opened: state.opened, collected: performance.now(),
      hover: state.hover, clickHash: state.clickHash, frames: state.frames, loafs,
      longtasks: state.longtasks.map((entry: PerformanceEntry) => ({ start: entry.startTime, duration: entry.duration })),
      resources, counts: { ...state.counts, scripts, resources: observed }, available: state.available,
    } as RawPostOpen;
  });
  return raw ? sanitizePostOpenDiagnostics(raw) : { unavailable: "post-opening probe did not initialize" };
}
