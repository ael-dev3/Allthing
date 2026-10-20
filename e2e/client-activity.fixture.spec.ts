// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";

/*
 * The client bar with the cron's count of accounts active through each app
 * in the past 24 hours (`apps_24h` in the home payload): pointing at an
 * app's segment or legend entry, or tapping it on a phone, opens its card.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const hash = (i: number) => `0x${(i + 1).toString(16).padStart(40, "0")}`;
const pool = Array.from({ length: 25 }, (_, i) => ({
  hash: hash(i), author: { fid: 1, username: "fixture" }, text: `Client activity fixture ${i}`, ts: 1792235681 - i, likes: 25 - i,
}));
// 15 casts from the Farcaster app, 5 from Base App, 2 from Other client, one
// from Fourth app, and one each from two small apps, which make up "Other
// apps".
const appOf = (i: number) => (i < 15 ? 9152 : i < 20 ? 309857 : i < 22 ? 1234 : i < 23 ? 5555 : i < 24 ? 6666 : 7777);
const apps = {
  "9152": { fid: 9152, username: "warpcast", display_name: "Warpcast" },
  "309857": { fid: 309857, username: "baseapp.base.eth", display_name: "Base App" },
  "1234": { fid: 1234, username: "otherclient", display_name: "Other client" },
  "5555": { fid: 5555, username: "fourth", display_name: "Fourth app" },
  "6666": { fid: 6666, username: "small", display_name: "Small app" },
  "7777": { fid: 7777, username: "tiny", display_name: "Tiny app" },
};
// The window ends as the hour in progress began.
const to = Math.floor(Date.now() / 3_600_000) * 3_600;
const hour = `${String(new Date(to * 1000).getUTCHours()).padStart(2, "0")}:00 UTC`;

function counted(unknown = 0) {
  return {
    to, counted: to + 120, active: 6174, unknown, none: 14,
    apps: [{ app: 9152, active: 4812 }, { app: 309857, active: 1203 }, { app: 5555, active: 120 }, { app: 6666, active: 37 }, { app: 1234, active: 1 }],
  };
}

async function serve(page: Page, apps24h: object | null) {
  const origins = Object.fromEntries(pool.map((c, i) => [c.hash, { signer: "0x01", app_fid: appOf(i) }]));
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.get("offset");
    const extra = apps24h && !later ? { apps_24h: apps24h } : {};
    return route.fulfill({ json: { pool: later ? [] : pool, notes: { origins, apps }, meta, ...extra } });
  });
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true,
  });
  return { context, page: await context.newPage() };
}

test("pointing at an app opens its card: its active accounts in the past 24 hours, and its share", async ({ page }) => {
  await serve(page, counted());
  await page.goto("/?sort=top");
  const share = page.getByRole("region", { name: "Apps posting today's trending casts" });
  const legend = share.locator(".client-legend");
  // The legend reads as before.
  await expect(legend.locator("li")).toHaveText(["Farcaster 60%", "Base App 20%", "Other client 8%", "Fourth app 4%", "Other apps 8%"]);
  await legend.getByRole("link", { name: /Farcaster/ }).hover();
  const card = page.getByRole("dialog", { name: "Farcaster: active accounts" });
  await expect(card).toBeVisible();
  await expect(card).toContainText("4,812 active accounts");
  await expect(card).toContainText(`in the 24 hours to ${hour}`);
  await expect(card).toContainText("60% of today's trending casts");
  await expect(card).not.toContainText("so far");
  await expect(card.getByRole("link", { name: "Show only casts from Farcaster" })).toHaveAttribute("href", /client=9152/);
  await expect(card.getByRole("link", { name: "Show only casts from Farcaster" })).toHaveAttribute("href", /sort=top/);
  // Inside the page, above the feed's cards.
  const box = (await card.boundingBox())!;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(1280);
  const covered = await page.evaluate(([x, y]) => document.elementFromPoint(x, y)?.closest(".popover-panel") !== null, [box.x + box.width / 2, box.y + box.height - 8]);
  expect(covered).toBe(true);
  await page.mouse.move(1200, 880);
  await expect(card).toBeHidden();

  // The bar's segments open the same cards, and a click filters.
  const segment = share.locator(".share-bar .seg.client-1");
  await segment.hover();
  const base = page.getByRole("dialog", { name: "Base App: active accounts" });
  await expect(base).toContainText("1,203 active accounts");
  await expect(base).toContainText("20% of today's trending casts");
  // An account is "1 active account".
  await page.mouse.move(1200, 880);
  await legend.getByRole("link", { name: /Other client/ }).hover();
  await expect(page.getByRole("dialog", { name: "Other client: active accounts" })).toContainText("1 active account");
  await page.mouse.move(1200, 880);
  await segment.click();
  await expect(page).toHaveURL(/client=309857/);
  await expect(page.locator(".cast-card:not(.skeleton)")).toHaveCount(5);
});

test("Other apps lists each one's active accounts, and the keyboard opens a card", async ({ page }) => {
  await serve(page, counted());
  await page.goto("/");
  await page.getByRole("button", { name: "Other apps" }).hover();
  const rest = page.getByRole("dialog", { name: "Other apps" });
  // An app the count hasn't seen in the window says nothing of it.
  await expect(rest.getByRole("link")).toHaveText(["Small app 50%37 active accounts", "Tiny app 50%"]);
  await page.mouse.move(1200, 880);
  await expect(rest).toBeHidden();
  // Tab to an app's legend entry: its card opens, and Escape closes it.
  const link = page.locator(".client-legend").getByRole("link", { name: /Base App/ });
  await link.focus();
  await page.keyboard.press("Shift+Tab");
  await page.keyboard.press("Tab");
  await expect(link).toBeFocused();
  const card = page.getByRole("dialog", { name: "Base App: active accounts" });
  await expect(card).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(card).toBeHidden();
  await expect(link).toBeFocused();
});

test("while apps aren't known for every account, a card says so", async ({ page }) => {
  await serve(page, counted(310));
  await page.goto("/");
  const legend = page.locator(".client-legend");
  await legend.getByRole("link", { name: /Farcaster/ }).hover();
  const card = page.getByRole("dialog", { name: "Farcaster: active accounts" });
  await expect(card).toContainText("At least 4,812 active accounts");
  await expect(card).toContainText("Apps are known for 94% of active accounts so far.");
  // An app not seen yet, rather than "at least 0".
  await page.mouse.move(1200, 880);
  await expect(card).toBeHidden();
  await page.getByRole("button", { name: "Other apps" }).hover();
  await expect(page.getByRole("dialog", { name: "Other apps" }).getByRole("link")).toHaveText(["Small app 50%37 active accounts", "Tiny app 50%"]);
});

test("an app the count hasn't seen says so", async ({ page }) => {
  const none = { ...counted(), apps: counted().apps.filter((a) => a.app !== 1234) };
  await serve(page, none);
  await page.goto("/");
  await page.locator(".client-legend").getByRole("link", { name: /Other client/ }).hover();
  await expect(page.getByRole("dialog", { name: "Other client: active accounts" }).locator(".client-card-active")).toHaveText("No active accounts");
  await page.mouse.move(1200, 880);
  await serve(page, { ...none, unknown: 310 });
  await page.reload();
  await page.locator(".client-legend").getByRole("link", { name: /Other client/ }).hover();
  await expect(page.getByRole("dialog", { name: "Other client: active accounts" }).locator(".client-card-active")).toHaveText("No active accounts known yet");
});

test("before the cron has counted by app, the bar is as it was", async ({ page }) => {
  await serve(page, null);
  await page.goto("/");
  const legend = page.locator(".client-legend");
  await expect(legend.locator("li")).toHaveCount(5);
  await legend.getByRole("link", { name: /Farcaster/ }).hover();
  await page.waitForTimeout(400);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.locator(".share-bar .seg")).toHaveCount(5);
  await expect(page.locator(".share-bar a")).toHaveCount(0);
});

test("at 390, a tap opens an app's card, whose link filters", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await serve(page, counted());
  await page.goto("/");
  const legend = page.locator(".client-legend");
  await legend.getByRole("link", { name: /Base App/ }).tap();
  const card = page.getByRole("dialog", { name: "Base App: active accounts" });
  await expect(card).toBeVisible();
  await expect(page).not.toHaveURL(/client=/);
  await expect(card).toContainText("1,203 active accounts");
  const box = (await card.boundingBox())!;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(390);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await card.getByRole("link", { name: "Show only casts from Base App" }).tap();
  await expect(page).toHaveURL(/client=309857/);
  await expect(page.locator(".cast-card:not(.skeleton)")).toHaveCount(5);
  await expect(card).toBeHidden();
  await context.close();
});
