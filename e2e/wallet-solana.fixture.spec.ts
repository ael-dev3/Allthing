// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const user = {
  fid: 42,
  username: "alice",
  display_name: "Alice",
  bio: "Reads conversations.",
  follower_count: 120,
  following_count: 30,
  registered_at: 1759222921,
  eth_addresses: ["0x76d0e7a13248945ee9f808b4a472262b28778942"],
  sol_addresses: ["CdHdAx1wh42nyqujiGTMc94es4pxKQoBYNkybTKEvKcS"],
};
// What the Worker reports when Solana's public RPC turns it away.
const solana = {
  chain: "solana", name: "Solana", kind: "solana", count: 0, capped: false, percent: 0,
  error: "upstream unreachable: not measured (upstream returned HTTP 403)",
};
const base = { chain: "base", name: "Base", kind: "evm", count: 120, capped: false, percent: 100 };
const note = "Solana couldn't be read just now, so its wallets aren't counted.";

async function serve(page: Page, chains: object[]) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: { user, pool: [], notes: {}, meta } }));
  await page.route("**/api/v1/vote?*", (route) => route.fulfill({ json: { fid: 42, onchain: "stay", meta } }));
  await page.route("**/api/v1/wallets?*", (route) => route.fulfill({ json: {
    fid: 42, addresses: [], chains, total: 120, sources: [], fetched_at: Date.now(),
  } }));
}

for (const width of [1280, 390]) {
  test(`wallet activity says when Solana isn't counted at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await serve(page, [base, solana]);
    await page.goto("/u/alice");
    const wallets = page.locator("section.wallets");
    await expect(wallets.locator(".share-list")).toHaveText(/Base\s*100%/);
    await expect(wallets.getByText(note)).toBeVisible();
    // Unread Solana is said once, not as an unread EVM chain.
    await expect(wallets).not.toContainText("Some chains weren't read");
  });

  test(`an account whose only activity is uncounted Solana says so, not "no activity", at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await serve(page, [solana]);
    await page.goto("/u/alice");
    const wallets = page.locator("section.wallets");
    await expect(wallets.getByText(note)).toBeVisible();
    await expect(wallets).not.toContainText("No onchain activity");
    await expect(wallets.locator(".share-bar")).toHaveCount(0);
  });

  test(`EVM wallets with no activity and uncounted Solana say both at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await serve(page, [{ ...base, count: 0, percent: 0 }, solana]);
    await page.goto("/u/alice");
    const wallets = page.locator("section.wallets");
    await expect(wallets.getByText("No activity on the EVM chains counted.")).toBeVisible();
    await expect(wallets.getByText(note)).toBeVisible();
  });
}
