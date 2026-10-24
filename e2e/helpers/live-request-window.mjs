// SPDX-License-Identifier: AGPL-3.0-or-later
export const WINDOW_MS = 60_000;
export const RETAINED = 50;
export const TEST_HEADROOM = 60;

/** Harness-only delay until fewer than fifty API attempts remain. */
export function quietFor(attempts, now) {
  const recent = attempts.filter((at) => at > now - WINDOW_MS).toSorted((a, b) => a - b);
  return recent.length < RETAINED ? 0 : Math.max(0, recent[recent.length - RETAINED] + WINDOW_MS + 1 - now);
}

/** Never keep a query, nonce, cookie, key or successful response body. */
export function endpoint(value, origin) {
  const url = new URL(value, origin);
  return url.origin === origin && url.pathname.startsWith("/api/") ? url.pathname.slice(0, 96) : null;
}

/** Fixed diagnostic vocabulary, rather than arbitrary upstream error text. */
export function errorCode(status, body, path) {
  if (path.startsWith("/api/v1/auth/")) return "auth_body_omitted";
  if (body.length > 4096) return "error_body_too_large";
  let error;
  try { error = JSON.parse(body).error; } catch { return "non_json_error"; }
  if (error === "Too many requests; slow down") return "api_rate_limited";
  if (error === "Still gathering this page; try again shortly") return "cache_build_pending";
  if (error === "Farcaster data source is busy; backing off") return "upstream_busy";
  if (error === "request budget exhausted") return "subrequest_budget_exhausted";
  if (/^upstream returned HTTP \d{3}$/.test(error ?? "")) return `upstream_http_${error.slice(-3)}`;
  return `api_http_${status}`;
}
