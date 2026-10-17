// SPDX-License-Identifier: AGPL-3.0-or-later
import { defineConfig, devices } from "@playwright/test";

/*
 * End-to-end tests against live Farcaster data.
 *
 *   npm run build && npm run test:e2e          # local: wrangler dev serves dist/ + the Worker
 *   E2E_BASE_URL=https://allthing.social npm run test:e2e
 *
 * With E2E_BASE_URL set, no local server is started.
 */

const PORT = Number(process.env.E2E_PORT ?? 8787);
const localURL = `http://127.0.0.1:${PORT}`;
const baseURL = process.env.E2E_BASE_URL ?? localURL;

// Behind an HTTPS proxy (corporate networks, sandboxes) Chromium needs it
// passed explicitly.
const proxy = process.env.HTTPS_PROXY ?? process.env.https_proxy;

export default defineConfig({
  testDir: "e2e",
  timeout: 90_000,
  expect: { timeout: 25_000 },
  fullyParallel: false,
  workers: 1,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL,
    viewport: { width: 1280, height: 900 },
    launchOptions: proxy
      ? { args: [`--proxy-server=${proxy}`, "--proxy-bypass-list=localhost;127.0.0.1"] }
      : undefined,
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"], viewport: { width: 1280, height: 900 } } }],
  webServer: process.env.E2E_BASE_URL
    ? undefined
    : {
        command: `npx wrangler dev --port ${PORT} --ip 127.0.0.1`,
        url: `${localURL}/api/v1/status`,
        reuseExistingServer: true,
        timeout: 120_000,
      },
});
