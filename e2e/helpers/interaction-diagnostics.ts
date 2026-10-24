// SPDX-License-Identifier: AGPL-3.0-or-later
import type { Page } from "@playwright/test";

/** Observe the original Event Timing measurement, retaining bounded context. */
export function interactionDiagnosticsProbe() {
  const root = window as any;
  root.__events = [] as number[];
  const diagnostic = root.__allthingInteractions = { events: [] as object[], tasks: [] as object[], frames: [] as object[] };
  const safeURL = (value: string) => {
    try { const url = new URL(value, location.href); return (url.origin + url.pathname).slice(0, 256); }
    catch { return null; }
  };
  new PerformanceObserver(list => {
    for (const entry of list.getEntries()) {
      const event = entry as any;
      if (!event.interactionId) continue;
      // Never cap the metric itself: its maximum remains the same check.
      root.__events.push(entry.duration);
      if (diagnostic.events.length < 48) diagnostic.events.push({
        name: event.name, start: event.startTime, duration: event.duration,
        processingStart: event.processingStart, processingEnd: event.processingEnd,
        interactionId: event.interactionId,
        target: event.target ? { tag: event.target.tagName, classes: String(event.target.className).slice(0, 120) } : null,
      });
    }
  }).observe({ type: "event", durationThreshold: 16, buffered: false } as PerformanceObserverInit);
  if (PerformanceObserver.supportedEntryTypes.includes("longtask")) {
    new PerformanceObserver(list => {
      for (const task of list.getEntries()) if (diagnostic.tasks.length < 32) {
        diagnostic.tasks.push({ start: task.startTime, duration: task.duration });
      }
    }).observe({ type: "longtask", buffered: false });
  }
  if (PerformanceObserver.supportedEntryTypes.includes("long-animation-frame")) {
    new PerformanceObserver(list => {
      for (const frame of list.getEntries() as any) if (diagnostic.frames.length < 24) {
        diagnostic.frames.push({
          start: frame.startTime, duration: frame.duration, renderStart: frame.renderStart,
          styleAndLayoutStart: frame.styleAndLayoutStart, blockingDuration: frame.blockingDuration,
          scripts: (frame.scripts ?? []).slice(0, 8).map((script: any) => ({
            start: script.startTime, duration: script.duration,
            forcedStyleAndLayoutDuration: script.forcedStyleAndLayoutDuration,
            invokerType: script.invokerType, sourceURL: safeURL(script.sourceURL ?? ""),
          })),
        });
      }
    }).observe({ type: "long-animation-frame", buffered: false });
  }
}

export async function readInteractionDiagnostics(page: Page) {
  return page.evaluate(() => (window as any).__allthingInteractions ?? { unavailable: true });
}
