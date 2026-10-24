// SPDX-License-Identifier: AGPL-3.0-or-later
import assert from "node:assert/strict";
import test from "node:test";
import { endpoint, errorCode, TEST_HEADROOM, WINDOW_MS, quietFor } from "../e2e/helpers/live-request-window.mjs";

test("rolling-window pacing leaves test headroom without delaying an interaction", () => {
  assert.equal(quietFor(Array(49).fill(1000), 1000), 0);
  assert.equal(quietFor(Array(50).fill(1000), 1000), WINDOW_MS + 1);
  assert.equal(quietFor(Array(50).fill(1000), 61_000), 0);
  const bursts = [...Array(25).fill(1000), ...Array(25).fill(2000), ...Array(20).fill(3000)];
  const wait = quietFor(bursts, 3000);
  assert.equal(wait, 58_001);
  assert.equal(quietFor(bursts, 3000 + wait), 0);
  assert.ok(49 + TEST_HEADROOM < 120);
});

test("accounting excludes other origins/assets and strips private query values", () => {
  const origin = "https://allthing.social";
  assert.equal(endpoint("/api/v1/home?private=value", origin), "/api/v1/home");
  assert.equal(endpoint("/api/v1/auth/qr?nonce=secret", origin), "/api/v1/auth/qr");
  assert.equal(endpoint("https://example.com/api/v1/home", origin), null);
  assert.equal(endpoint("/m/123/image.png", origin), null);
  assert.equal(endpoint("/assets/allthing.js", origin), null);
});

test("error diagnostics distinguish busy causes without retaining private bodies", () => {
  const path = "/api/v1/channels";
  assert.equal(errorCode(429, JSON.stringify({ error: "Too many requests; slow down" }), path), "api_rate_limited");
  assert.equal(errorCode(503, JSON.stringify({ error: "Still gathering this page; try again shortly" }), path), "cache_build_pending");
  assert.equal(errorCode(503, JSON.stringify({ error: "Farcaster data source is busy; backing off" }), path), "upstream_busy");
  assert.equal(errorCode(503, JSON.stringify({ error: "request budget exhausted" }), path), "subrequest_budget_exhausted");
  assert.equal(errorCode(502, JSON.stringify({ error: "upstream returned HTTP 503" }), path), "upstream_http_503");
  assert.equal(errorCode(500, JSON.stringify({ error: "secret https://private.invalid/?key=secret" }), path), "api_http_500");
  assert.equal(errorCode(400, JSON.stringify({ nonce: "secret" }), "/api/v1/auth/qr"), "auth_body_omitted");
  assert.equal(errorCode(503, "<html>edge unavailable</html>", path), "non_json_error");
  assert.equal(errorCode(503, "x".repeat(4097), path), "error_body_too_large");
});
