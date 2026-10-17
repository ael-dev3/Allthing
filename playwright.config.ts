import { defineConfig, devices } from "@playwright/test";

/*
 * End-to-end tests reading live Farcaster data, against either build:
 *
 *   npm run build && npm run test:e2e                     # server build
 *   ALLTHING_BASE_PATH=/Allthing npm run build:static \
 *     && E2E_STATIC=1 npm run test:e2e                    # static export
 *   E2E_STATIC=1 E2E_BASE_URL=https://ael-dev3.github.io/Allthing/ npm run test:e2e
 *
 * With E2E_BASE_URL set, no local server is started.
 */

const staticMode = process.env.E2E_STATIC === "1";
const PORT = Number(process.env.E2E_PORT ?? (staticMode ? 3200 : 3100));
const basePath = staticMode ? (process.env.E2E_BASE_PATH ?? "/Allthing") : "";
const localURL = `http://localhost:${PORT}${basePath}/`;
const baseURL = process.env.E2E_BASE_URL ?? localURL;

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
        command: staticMode
          ? `node scripts/serve-static.mjs out ${basePath} ${PORT}`
          : `npm run start -- -p ${PORT}`,
        url: localURL,
        reuseExistingServer: !process.env.CI,
        timeout: 120_000,
        env: proxy ? { NODE_USE_ENV_PROXY: "1" } : {},
      },
});
