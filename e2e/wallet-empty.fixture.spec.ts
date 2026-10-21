// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const user = { fid: 42, username: "alice", display_name: "Alice", bio: "Reads conversations.", follower_count: 120,
  following_count: 30, eth_addresses: ["0x76d0e7a13248945ee9f808b4a472262b28778942"] };
const chain = (name: string, error?: string, carried = false) => ({ chain: name, name, kind: "evm", count: 0, percent: 0, error, carried });
const cases: { name: string; chains: object[]; expected: string; absent: string; behind?: string }[] = [
  { name: "all chains unread", chains: [chain("base", "temporarily unavailable"), chain("ethereum", "upstream busy")],
    expected: "Wallet activity couldn't be read just now.", absent: "No onchain activity" },
  { name: "zero on the chains read and another unread", chains: [chain("base"), chain("ethereum", "upstream busy")],
    expected: "No activity on the chains counted.", absent: "No onchain activity" },
  { name: "a carried zero count", chains: [chain("base", undefined, true)],
    expected: "Some counts are from an earlier read", absent: "Some chains weren't read just now", behind: "earlier counts" },
  { name: "known zero counts", chains: [chain("base"), chain("ethereum")],
    expected: "No onchain activity from verified wallets.", absent: "couldn't be read" },
];

for (const width of [1280, 390]) {
  for (const scenario of cases) {
    test(`${scenario.name} is described accurately at width ${width}`, async ({ page }, info) => {
      await page.setViewportSize({ width, height: 844 });
      await page.route("**/api/v1/**", route => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
      await page.route("**/api/v1/user?*", route => route.fulfill({ json: { user, pool: [], notes: {}, meta } }));
      await page.route("**/api/v1/vote?*", route => route.fulfill({ json: { fid: 42, onchain: "stay", meta } }));
      await page.route("**/api/v1/wallets?*", route => route.fulfill({ json: { fid: 42,
        addresses: [{ address: user.eth_addresses[0], kind: "evm", primary: true }], chains: scenario.chains,
        total: 0, sources: [], fetched_at: Date.now() } }));
      await page.goto("/u/alice");
      const wallets = page.getByRole("region", { name: "Wallet activity by chain", exact: true });
      await expect(wallets).toBeVisible();
      await expect(wallets.locator(".share-bar.loading")).toHaveCount(0);
      await expect(wallets).toContainText(/No activity|No onchain activity|couldn't be read|counts are from/);
      // A note beside the message is behind its word, which shows it when pointed at.
      if (scenario.behind) await wallets.getByRole("button", { name: `${scenario.behind}: why` }).hover();
      const notes = page.locator(".popover-panel .share-notes");
      if (scenario.behind) await expect(notes).toBeVisible();
      const text = (await wallets.innerText()) + (scenario.behind ? await notes.innerText() : "");
      await page.screenshot({ path: info.outputPath(`wallet-${width}.png`) });
      expect(text).toContain(scenario.expected);
      expect(text).not.toContain(scenario.absent);
      await expect(wallets.locator(".share-bar")).toHaveCount(0);
    });
  }
}
