// SPDX-License-Identifier: AGPL-3.0-or-later
import { setTimeout as pause } from "node:timers/promises";
import { test, type APIRequestContext, type APIResponse, type BrowserContext, type Request, type Response } from "@playwright/test";
import { endpoint, errorCode, RETAINED, TEST_HEADROOM, WINDOW_MS, quietFor } from "./live-request-window.mjs";

type Attempt = { method: string; path: string; at: number; status?: number; error_code?: string; retry_after?: number; cache?: string };
type Answer = { attempt: Attempt; response: Response | APIResponse };
const recent: number[] = [];

/** Live Worker tests run alone with one worker. Pace between assertions,
 * never in an interaction's timing window. Routed attempts are counted
 * conservatively too. No answer is retried or accepted by this helper. */
export function livePacing(baseURL: string | undefined) {
  if (!baseURL) return;
  const origin = new URL(baseURL).origin;
  let attempts: Attempt[] = [];
  let answers: Answer[] = [];
  let restore: (() => void)[] = [];

  test.beforeAll(async () => {
    // Also clears the previous worker's window after an unexpected failure.
    await pause(WINDOW_MS + 1);
    recent.length = 0;
  });
  test.beforeEach(async ({ browser, context, request }) => {
    attempts = []; answers = []; restore = [];
    const contexts = new WeakSet<BrowserContext>();
    const clients = new WeakSet<APIRequestContext>();
    const record = (url: string, method: string) => {
      const path = endpoint(url, origin);
      if (!path) return null;
      const at = performance.now();
      recent.push(at);
      const attempt = { method, path, at };
      attempts.push(attempt);
      return attempt;
    };
    const api = (client: APIRequestContext) => {
      if (clients.has(client)) return;
      clients.add(client);
      const fetch = client.fetch;
      client.fetch = async function (...args: Parameters<typeof fetch>) {
        const input = args[0];
        const attempt = record(typeof input === "string" ? input : input.url(), (args[1]?.method ?? (typeof input === "string" ? "GET" : input.method())).toUpperCase());
        try {
          const response = await Reflect.apply(fetch, this, args);
          if (attempt) answers.push({ attempt, response });
          return response;
        } catch (error) {
          if (attempt) attempt.error_code = "network_failed";
          throw error;
        }
      };
      restore.push(() => { client.fetch = fetch; });
    };
    const watch = (context: BrowserContext) => {
      if (contexts.has(context)) return;
      contexts.add(context);
      api(context.request);
      const requests = new WeakMap<Request, Attempt>();
      const sent = (request: Request) => { const attempt = record(request.url(), request.method()); if (attempt) requests.set(request, attempt); };
      const answered = (response: Response) => { const attempt = requests.get(response.request()); if (attempt) answers.push({ attempt, response }); };
      const failed = (request: Request) => { const attempt = requests.get(request); if (attempt) attempt.error_code = "network_failed"; };
      context.on("request", sent); context.on("response", answered); context.on("requestfailed", failed);
      restore.push(() => { context.off("request", sent); context.off("response", answered); context.off("requestfailed", failed); });
    };
    api(request); watch(context);
    const newContext = browser.newContext;
    browser.newContext = async function (...args: Parameters<typeof newContext>) {
      const context = await Reflect.apply(newContext, this, args);
      watch(context);
      return context;
    };
    restore.push(() => { browser.newContext = newContext; });
  });

  test.afterEach(async ({}, info) => {
    const started = performance.now();
    const deadline = started + WINDOW_MS + 1000;
    let failure: Error | undefined;
    try {
      for (;;) {
        const now = performance.now();
        const wait = quietFor(recent, now);
        if (!wait) break;
        if (now + wait > deadline) throw new Error("The live API window did not quiet within 61 seconds");
        await pause(wait);
      }
      if (attempts.length > TEST_HEADROOM) throw new Error(`Live test used ${attempts.length} API attempts; review its ${TEST_HEADROOM}-request quota headroom`);
    } catch (error) { failure = error as Error; }
    finally { restore.reverse().forEach((undo) => undo()); }

    // Read only failed bodies after measurements; diagnostics have one shared
    // one-second ceiling, not another unbounded wait for a broken service.
    const bad: Attempt[] = [];
    const pending = answers.map(async ({ attempt, response }) => {
      attempt.status = response.status();
      if (attempt.status < 400 || bad.length >= 24) return;
      bad.push(attempt);
      attempt.error_code = "error_body_unavailable";
      try {
        const headers = await response.headers();
        const retry = headers["retry-after"];
        if (/^\d{1,5}$/.test(retry ?? "")) attempt.retry_after = Number(retry);
        const cache = headers["x-allthing-cache"];
        if (["fresh", "miss", "revalidating", "stale-if-error", "skipped"].includes(cache)) attempt.cache = cache;
        if (attempt.path.startsWith("/api/v1/auth/")) attempt.error_code = "auth_body_omitted";
        else if (Number(headers["content-length"] ?? 0) > 4096) attempt.error_code = "error_body_too_large";
        else attempt.error_code = errorCode(attempt.status, await response.text(), attempt.path);
      } catch { /* The failed status is still retained when its body vanished. */ }
    });
    let timer: ReturnType<typeof setTimeout> | undefined;
    await Promise.race([Promise.all(pending), new Promise<void>((done) => { timer = setTimeout(done, 1000); })]);
    clearTimeout(timer);
    const now = performance.now();
    const counts: Record<string, number> = {};
    for (const attempt of attempts) {
      const key = `${attempt.method} ${attempt.path}`;
      const bounded = key in counts || Object.keys(counts).length < 64 ? key : "other API paths";
      counts[bounded] = (counts[bounded] ?? 0) + 1;
    }
    const stripped = ({ at, ...attempt }: Attempt) => attempt;
    await info.attach("live-api-window", { contentType: "application/json", body: JSON.stringify({
      scope: "same-origin API attempts, routed fixtures counted conservatively; no queries, identity or successful bodies",
      attempts: attempts.length, counts, between_tests_ms: Math.round(now - started),
      initial_quiet_ms: WINDOW_MS + 1,
      rolling_60s_attempts: recent.filter((at) => at > now - WINDOW_MS).length,
      next_test_window_below: RETAINED, individual_test_headroom: TEST_HEADROOM,
      network_failures: attempts.filter((attempt) => attempt.error_code === "network_failed").slice(0, 24).map(stripped),
      bad_responses: bad.map(stripped),
    }, null, 2) });
    recent.splice(0, recent.length, ...recent.filter((at) => at > now - WINDOW_MS));
    if (!failure && answers.some(({ response }) => response.status() === 429)) {
      failure = new Error("The live suite hit the API rate limit; inspect live-api-window before continuing");
    }
    if (failure) throw failure;
  });
}
