// SPDX-License-Identifier: AGPL-3.0-or-later
import { setTimeout as delay } from "node:timers/promises";
import { pathToFileURL } from "node:url";

const sha = /^[a-f0-9]{40}$/i;
const deployedSha = /^[a-f0-9]{7,40}$/i;

// The Worker stamps its commit with seven hex digits; accept a longer stamp too.
export function revisionMatches(actual, expected) {
  return typeof actual === "string" && deployedSha.test(actual) && typeof expected === "string" && sha.test(expected)
    && expected.toLowerCase().startsWith(actual.toLowerCase());
}

export async function waitForDeploy(baseURL, expected, {
  timeoutMs = 300_000,
  pollIntervalMs = 5000,
  requestTimeoutMs = 5000,
  log = () => {},
} = {}) {
  if (typeof expected !== "string" || !sha.test(expected)) throw new TypeError("Expected a full 40-digit Git commit");
  for (const value of [timeoutMs, pollIntervalMs, requestTimeoutMs]) {
    if (!Number.isFinite(value) || value <= 0) throw new TypeError("Wait durations must be positive and finite");
  }
  const url = new URL("/version", baseURL);
  if (!["http:", "https:"].includes(url.protocol)) throw new TypeError("Expected an HTTP(S) site URL");
  const deadline = performance.now() + timeoutMs;
  let last = "no response";
  while (performance.now() < deadline) {
    try {
      const response = await fetch(url, {
        cache: "no-store",
        headers: { accept: "application/json", "cache-control": "no-cache" },
        signal: AbortSignal.timeout(Math.max(1, Math.ceil(Math.min(requestTimeoutMs, deadline - performance.now())))),
      });
      if (!response.ok) {
        last = `HTTP ${response.status}`;
        await response.body?.cancel();
      } else {
        const body = await response.json();
        if (revisionMatches(body?.commit, expected)) {
          log(`Deployment ${body.commit} is live`);
          return body.commit;
        }
        last = typeof body?.commit === "string" && deployedSha.test(body.commit)
          ? `commit ${body.commit}` : "unrecognized version response";
      }
    } catch (error) {
      last = error.name === "TimeoutError" || error.name === "AbortError" ? "version request timed out" : "version request failed";
    }
    log(`Waiting for deployment ${expected.slice(0, 7)}: ${last}`);
    const remaining = deadline - performance.now();
    if (remaining > 0) await delay(Math.min(pollIntervalMs, remaining));
  }
  throw new Error(`Deployment ${expected.slice(0, 7)} was not live within ${timeoutMs / 1000}s (${last})`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [, , baseURL, expected] = process.argv;
  if (!baseURL || !expected) {
    console.error("Usage: node scripts/wait-for-deploy.mjs <site-url> <full-commit>");
    process.exitCode = 1;
  } else {
    try {
      await waitForDeploy(baseURL, expected, { log: console.log });
    } catch (error) {
      console.error(error.message);
      process.exitCode = 1;
    }
  }
}
