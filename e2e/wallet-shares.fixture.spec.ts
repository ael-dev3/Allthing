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
};
const evm = (chain: string, name: string, count: number, percent: number) =>
  ({ chain, name, kind: "evm", count, capped: false, percent });

async function serve(page: Page, chains: object[]) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: { user, pool: [], notes: {}, meta } }));
  await page.route("**/api/v1/vote?*", (route) => route.fulfill({ json: { fid: 42, onchain: "stay", meta } }));
  await page.route("**/api/v1/wallets?*", (route) => route.fulfill({ json: {
    fid: 42,
    addresses: [{ address: user.eth_addresses[0], kind: "evm", primary: true }],
    chains,
    total: 33090,
    sources: [],
    fetched_at: Date.now(),
  } }));
}

for (const width of [1280, 390]) {
  test(`wallet shares follow the current rules in a payload cached under older ones at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    // As the edge cached it before chains under 1% were left out: Optimism's
    // 95 of 33,090 transactions (0.3%) rounded up to 1%.
    await serve(page, [
      evm("base", "Base", 32520, 98),
      evm("ethereum", "Ethereum", 388, 1),
      evm("optimism", "Optimism", 95, 1),
      evm("monad", "Monad", 66, 0),
      evm("arbitrum", "Arbitrum", 19, 0),
    ]);
    await page.goto("/u/alice");
    const wallets = page.locator("section.wallets");
    await expect(wallets.locator(".share-list li")).toHaveCount(2);
    await expect(wallets.locator(".share-list")).toHaveText(/Base\s*99%\s*Ethereum\s*1%/);
    await expect(wallets.locator(".share-bar")).toHaveAttribute("aria-label", "Base 99%, Ethereum 1%");
    await expect(wallets).not.toContainText("Optimism");
  });
}

// 0xael.eth's counts as cross-checked against each chain's RPC on 20 October
// 2026 (docs/WALLETS.md), with Solana unread: the profile and its card agree.
const crossChecked = [
  evm("base", "Base", 32525, 99),
  evm("ethereum", "Ethereum", 388, 1),
  evm("optimism", "Optimism", 95, 0),
  evm("monad", "Monad", 66, 0),
  evm("arbitrum", "Arbitrum", 19, 0),
  evm("celo", "Celo", 2, 0),
  { chain: "solana", name: "Solana", kind: "solana", count: 0, capped: false, percent: 0, error: "upstream unreachable: not measured" },
];

for (const width of [1280, 390]) {
  test(`at ${width}, the profile and its card show the cross-checked shares: Base 99%, Ethereum 1%`, async ({ browser }, testInfo) => {
    const phone = width === 390;
    const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width, height: 900 }, isMobile: phone, hasTouch: phone });
    const page = await context.newPage();
    await serve(page, crossChecked);
    // One cast by the account, so the profile lists a card whose author opens the card.
    await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: {
      user, pool: [{ hash: `0x${"5".repeat(40)}`, author: { fid: 42, username: "alice", display_name: "Alice" }, text: "Wallets fixture", ts: 1792235681 }], notes: {}, meta,
    } }));
    await page.goto("/u/alice");
    const wallets = page.locator("section.wallets");
    await expect(wallets.locator(".share-list li")).toHaveText([/^Base\s*99%/, /^Ethereum\s*1%/]);
    await expect(wallets.locator(".share-bar")).toHaveAttribute("aria-label", "Base 99%, Ethereum 1%");
    await expect(wallets).toContainText("Solana couldn't be read just now, so its wallets aren't counted.");
    const author = page.locator(".cast-card").first().getByRole("link", { name: "Alice" }).first();
    if (phone) await author.tap();
    else await author.hover();
    const card = page.locator(".popover-panel.profile-card");
    await expect(card.locator(".share-list li")).toHaveText([/^Base\s*99%/, /^Ethereum\s*1%/]);
    await expect(card.locator(".share-bar")).toHaveAttribute("aria-label", "Base 99%, Ethereum 1%");
    await expect(card.locator(".share-note")).toContainText("partial");
    const box = (await card.boundingBox())!;
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(width);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    await context.close();
  });
}
