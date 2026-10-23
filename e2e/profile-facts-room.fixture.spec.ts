// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { NOW } from "./fixture-time";

/*
 * A profile's header keeps its height as its vote and wallet activity come,
 * each from an endpoint of its own, after the profile: the vote's row is
 * there from the start, and the wallets' block is one height whatever it
 * says, like a profile card's. An account with no verified wallets, which
 * the profile itself says, shows the line that says so at once, and keeps
 * no room for a chart. Here the vote comes 1.2 s after it's asked for, and
 * the wallets 1.5 s; nothing under the header moves when they do.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = (NOW - 19);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const base = { fid: 42, username: "alice", display_name: "Alice", bio: "Reads conversations.", follower_count: 120, following_count: 30, registered_at: 1759222921 };
const wallets = {
  eth_addresses: ["0x76d0e7a13248945ee9f808b4a472262b28778942", "0x5f1b3d2e8c9a0b7d6e4f3a2b1c0d9e8f7a6b5c4d"],
  sol_addresses: ["CdHdAx1wh42nyqujiGTMc94es4pxKQoBYNkybTKEvKcS"],
};
const evm = (chain: string, name: string, count: number, percent: number, extra: object = {}) =>
  ({ chain, name, kind: "evm", count, capped: false, percent, ...extra });
// Five chains with shares, one carried from an earlier read, one unread, and Solana unread: every note.
const chains = [
  evm("base", "Base", 600, 60, { carried: true }),
  evm("ethereum", "Ethereum", 200, 20),
  evm("optimism", "Optimism", 100, 10),
  evm("arbitrum", "Arbitrum", 60, 6),
  evm("zora", "Zora", 40, 4),
  evm("celo", "Celo", 0, 0, { error: "upstream returned HTTP 429" }),
  { chain: "solana", name: "Solana", kind: "solana", count: 0, capped: false, percent: 0, error: "upstream unreachable: not measured" },
];
const later = (ms: number) => new Promise((done) => setTimeout(done, ms));

/** The profile at once, its vote 1.2 s and its wallets 1.5 s after they're asked for. */
async function serve(page: Page, user: { eth_addresses?: string[] }, vote: "stay" | "fails" = "stay") {
  const asked: string[] = [];
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  const pool = Array.from({ length: 8 }, (_, i) => ({ hash: hex(i + 1), author: base, text: `Cast ${i} by Alice`, ts: ts - i * 60 }));
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: { user, pool, notes: {}, meta } }));
  await page.route("**/api/v1/vote?*", async (route) => {
    asked.push("vote");
    await later(1200);
    return vote === "stay" ? route.fulfill({ json: { fid: 42, onchain: "stay", meta } }) : route.fulfill({ status: 503, json: { error: "Upstream unavailable" } });
  });
  await page.route("**/api/v1/wallets?*", async (route) => {
    asked.push("wallets");
    await later(1500);
    // What the Worker would say: the account's addresses, and what they did.
    if (!user.eth_addresses) return route.fulfill({ json: { fid: 42, addresses: [], chains: [], total: 0, sources: [], fetched_at: Date.now() } });
    const addresses = [...wallets.eth_addresses.map((address) => ({ address, kind: "evm" })), { address: wallets.sol_addresses[0], kind: "solana" }];
    return route.fulfill({ json: { fid: 42, addresses, chains, total: 1000, sources: [], fetched_at: Date.now() } });
  });
  return asked;
}

/** Records layout shifts from the start, and whether each moved only the footer. */
const watchShifts = (page: Page) =>
  page.addInitScript(() => {
    const shifts: { value: number; input: boolean; footer: boolean }[] = [];
    (window as unknown as { __shifts: typeof shifts }).__shifts = shifts;
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries() as unknown as { value: number; hadRecentInput: boolean; sources: { node?: Node }[] }[]) {
        const footer = entry.sources.length > 0 && entry.sources.every((s) => s.node instanceof Element && s.node.closest(".site-footer") !== null);
        shifts.push({ value: entry.value, input: entry.hadRecentInput, footer });
      }
    }).observe({ type: "layout-shift", buffered: true });
  });

/** Layout shift since the `from`th entry, as CLS counts it; the footer's own move, as the page's content takes its placeholder's place, is the page's height while loading, not counted. */
const shiftSince = (page: Page, from: number) =>
  page.evaluate(
    (from) =>
      (window as unknown as { __shifts: { value: number; input: boolean; footer: boolean }[] }).__shifts
        .slice(from)
        .filter((s) => !s.input && !s.footer)
        .reduce((sum, s) => sum + s.value, 0),
    from,
  );

const shiftCount = (page: Page) => page.evaluate(() => (window as unknown as { __shifts: unknown[] }).__shifts.length);

/** Once the entrances on the page have run: their transforms move what's measured. */
const settled = (page: Page) =>
  expect
    .poll(() =>
      page.evaluate(() =>
        document.getAnimations().filter((a) => a.effect?.getTiming().iterations !== Infinity).every((a) => a.playState === "finished"),
      ),
    )
    .toBe(true);

/** Where the profile's tabs, and its first cast, are laid out on the page. */
const below = (page: Page) =>
  page.evaluate(() => {
    const top = (selector: string) => Math.round(document.querySelector(selector)!.getBoundingClientRect().top + scrollY);
    return { tabs: top("nav.tabs"), cast: top(".cast-list .cast-card[data-hash]") };
  });

for (const width of [1280, 390]) {
  test.describe(`at width ${width}`, () => {
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 900 });
      await watchShifts(page);
    });

    test("an account with wallets keeps its header's height as its vote and wallet activity come", async ({ page }) => {
      await serve(page, { ...base, ...wallets });
      await page.goto("/u/alice");
      const header = page.locator("section.profile");
      await expect(page.locator(".cast-list .cast-card[data-hash]").first()).toBeVisible();
      await settled(page);
      // Neither has come yet: the vote's row and the wallets' block are there, loading.
      await expect(header.locator(".vote-loading")).toHaveCount(1);
      await expect(header.locator(".share-bar.loading")).toHaveCount(1);
      const before = await below(page);
      const from = await shiftCount(page);
      await expect(header.locator(".vote-yes")).toBeVisible({ timeout: 10_000 });
      await expect(header.locator(".share-bar:not(.loading)")).toBeVisible({ timeout: 10_000 });
      await settled(page);
      expect(await below(page)).toEqual(before);
      expect(await shiftSince(page, from)).toBeLessThan(0.05);
      // What came: the largest chains by name, as many as the line has room
      // for beside the notes' word, the rest together, so what shows adds
      // up to 100; and nothing cut off, with room to spare.
      const shares = header.locator(".profile-wallets .share-list li:visible");
      const named = width === 390 ? [/^Base\s*60%$/, /^\+4 40%$/] : [/^Base\s*60%$/, /^Ethereum\s*20%$/, /^Optimism\s*10%$/, /^Arbitrum\s*6%$/, /^\+1 4%$/];
      await expect(shares).toHaveText(named);
      const room = await header.locator(".profile-wallets .share-list").evaluate((list) => {
        const shown = [...list.querySelectorAll("li")].filter((li) => li.getClientRects().length > 0);
        return list.getBoundingClientRect().right - shown[shown.length - 1].getBoundingClientRect().right;
      });
      expect(room).toBeGreaterThanOrEqual(8);
      await expect(header.locator(".profile-wallets .share-bar")).toHaveAttribute("aria-label", "Base 60%, Ethereum 20%, Optimism 10%, Arbitrum 6%, Zora 4%");
      const word = header.getByRole("button", { name: "partial: why" });
      await word.hover();
      const notes = page.locator(".popover-panel .share-notes");
      await expect(notes).toContainText("Some chains weren't read just now, so the shares cover the rest.");
      await expect(notes).toContainText("Some counts are from an earlier read");
      await expect(notes).toContainText("Solana couldn't be read just now, so its wallets aren't counted.");
    });

    test("an account with no verified wallets says so at once, and keeps no room for a chart", async ({ page }) => {
      const asked = await serve(page, base);
      await page.goto("/u/alice");
      await expect(page.locator("section.profile .profile-name")).toHaveText("Alice");
      const section = page.getByRole("region", { name: "Wallet activity by chain", exact: true });
      await expect(section).toContainText("No onchain activity from verified wallets.", { timeout: 500 });
      await expect(section.locator(".share-bar")).toHaveCount(0);
      await expect(page.locator(".cast-list .cast-card[data-hash]").first()).toBeVisible();
      await settled(page);
      const box = (await section.locator(".share-message").boundingBox())!;
      expect(box.height).toBeLessThanOrEqual(24);
      const before = await below(page);
      const from = await shiftCount(page);
      await expect(page.locator("section.profile .vote-yes")).toBeVisible({ timeout: 10_000 });
      await settled(page);
      expect(await below(page)).toEqual(before);
      expect(await shiftSince(page, from)).toBeLessThan(0.05);
      // It had nothing to ask.
      expect(asked).toEqual(["vote"]);
    });

    test("a vote that can't be read keeps its row, and says so", async ({ page }) => {
      await serve(page, { ...base, ...wallets }, "fails");
      await page.goto("/u/alice");
      await expect(page.locator(".cast-list .cast-card[data-hash]").first()).toBeVisible();
      await settled(page);
      const before = await below(page);
      const vote = page.locator("section.profile .vote-fact");
      await expect(vote.locator(".vote-unknown")).toHaveText("Unknown", { timeout: 10_000 });
      await expect(page.locator("section.profile .share-bar:not(.loading)")).toBeVisible({ timeout: 10_000 });
      await settled(page);
      expect(await below(page)).toEqual(before);
    });
  });
}
