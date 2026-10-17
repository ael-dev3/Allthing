import { defineConfig, devices } from "@playwright/test";

/*
 * End-to-end tests against a production build reading live Farcaster data.
 *
 *   npm run build && npm run test:e2e
 *
 * Set E2E_BASE_URL to test an already-running deployment instead.
 */

const PORT = Number(process.env.E2E_PORT ?? 3100);
const baseURL = process.env.E2E_BASE_URL ?? `http://localhost:${PORT}`;

// Behind an HTTPS proxy (corporate networks, sandboxes) Chromium needs it
// passed explicitly, and Node's fetch only honours it with NODE_USE_ENV_PROXY.
const proxy = process.env.HTTPS_PROXY ?? process.env.https_proxy;

export default defineConfig({
  testDir: "e2e",
  timeout: 60_000,
  expect: { timeout: 20_000 },
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
        command: `npm run start -- -p ${PORT}`,
        url: baseURL,
        reuseExistingServer: !process.env.CI,
        timeout: 120_000,
        env: proxy ? { NODE_USE_ENV_PROXY: "1" } : {},
      },
});
