// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { NOW } from "./fixture-time";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const hash = (i: number) => `0x${(i + 1).toString(16).padStart(40, "0")}`;
const pool = Array.from({ length: 25 }, (_, i) => ({
  hash: hash(i), author: { fid: 1, username: "fixture" }, text: `Client share fixture ${i}`, ts: (NOW - 19) - i, likes: 25 - i,
}));
// 15 casts from the Farcaster app (its account is still named Warpcast), 6
// from Base App, 4 from another app.
const appOf = (i: number) => (i < 15 ? 9152 : i < 21 ? 309857 : 1234);
const apps = {
  "9152": { fid: 9152, username: "warpcast", display_name: "Warpcast" },
  "309857": { fid: 309857, username: "baseapp.base.eth", display_name: "Base App" },
  "1234": { fid: 1234, username: "otherclient", display_name: "Other client" },
};

async function serve(page: Page, known: number, { indexed = false } = {}) {
  const origins = Object.fromEntries(pool.slice(0, known).map((c, i) => [c.hash, { signer: "0x01", app_fid: appOf(i) }]));
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const params = new URL(route.request().url()).searchParams;
    const index = indexed && params.get("window") === "30d" ? { index: { casts: 500, ranked_at: Date.now() } } : {};
    return route.fulfill({ json: { pool: params.get("offset") ? [] : pool, notes: { origins, apps }, meta, ...index } });
  });
}

test("the front page shows which apps today's trending casts were posted from", async ({ page }) => {
  await serve(page, 25);
  await page.goto("/");
  const share = page.getByRole("region", { name: "Apps posting the past 24 hours' trending casts" });
  await expect(share).toBeVisible();
  await expect(share.locator(".client-legend li")).toHaveText(["Farcaster 60%", "Base App 24%", "Other client 16%"]);
  await expect(share.getByRole("img")).toHaveAttribute("aria-label", "Farcaster 60%, Base App 24%, Other client 16%");
  await expect(share.locator(".share-bar .seg")).toHaveCount(3);
  // The Farcaster app is "Farcaster" everywhere, also in each cast's "via".
  await expect(page.locator(".cast-card").first().locator(".origin")).toHaveText("via Farcaster");
  await expect(page.getByText("Warpcast")).toHaveCount(0);
});

test("the bar names the time range it covers", async ({ page }) => {
  await serve(page, 25);
  await page.goto("/?t=hour");
  await expect(page.getByRole("region", { name: "Apps posting the past hour's trending casts" })).toBeVisible();
  // Before the index can rank the past month, the list is the past week's.
  await page.goto("/?sort=top&t=month");
  await expect(page.getByRole("region", { name: "Apps posting the past 7 days' trending casts" })).toBeVisible();
});

test("the bar names a range ranked from the index", async ({ page }) => {
  await serve(page, 25, { indexed: true });
  await page.goto("/?sort=top&t=month");
  await expect(page.getByRole("region", { name: "Apps posting the past 30 days' trending casts" })).toBeVisible();
});

test("with few casts' apps known, the bar shares them among all the casts, and counts the rest as not known", async ({ page }) => {
  await serve(page, 10);
  await page.goto("/");
  const share = page.getByRole("region", { name: "Apps posting the past 24 hours' trending casts" });
  // 10 from the Farcaster app of 25: 40%, not 100%.
  await expect(share.locator(".client-legend li")).toHaveText(["Farcaster 40%", "15 not known"]);
  await expect(share.locator(".share-bar .seg-unknown")).toHaveCount(1);
});

test("no share bar when no cast's app is known and none is coming", async ({ page }) => {
  await serve(page, 0);
  await page.goto("/");
  await expect(page.locator(".cast-card").first()).toBeVisible();
  await expect(page.locator(".client-share")).toHaveCount(0);
});

test("the Farcaster app is never called @warpcast, in links or tooltips", async ({ page }) => {
  await serve(page, 25);
  await page.goto("/");
  const via = page.locator(".cast-card").first().locator(".origin");
  await expect(via).toHaveText("via Farcaster");
  await expect(via.locator("a")).toHaveAttribute("href", /\/u\/fid(:|%3A)9152$/);
  await expect(via).toHaveAttribute("aria-description", /requested by Farcaster \(app fid 9152\)/);
  expect(await page.locator("[title*='warpcast' i], [aria-description*='warpcast' i], a[href*='/u/warpcast']").count()).toBe(0);
});
