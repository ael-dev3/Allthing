// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { NOW } from "./fixture-time";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const hash = (i: number) => `0x${(i + 1).toString(16).padStart(40, "0")}`;
const pool = Array.from({ length: 25 }, (_, i) => ({
  hash: hash(i), author: { fid: 1, username: "fixture" }, text: `Client filter fixture ${i}`, ts: (NOW - 19) - i, likes: 25 - i,
}));
// Four named apps lead; two smaller ones make up "Other apps".
const appOf = (i: number) => (i < 10 ? 9152 : i < 16 ? 309857 : i < 19 ? 1234 : i < 21 ? 5555 : i < 23 ? 6666 : 7777);
const apps = {
  "9152": { fid: 9152, username: "warpcast", display_name: "Warpcast" },
  "309857": { fid: 309857, username: "baseapp.base.eth", display_name: "Base App" },
  "1234": { fid: 1234, username: "otherclient", display_name: "Other client" },
  "5555": { fid: 5555, username: "fourth", display_name: "Fourth app" },
  "6666": { fid: 6666, username: "small", display_name: "Small app" },
  "7777": { fid: 7777, username: "tiny", display_name: "Tiny app" },
};

async function serve(page: Page) {
  const origins = Object.fromEntries(pool.map((c, i) => [c.hash, { signer: "0x01", app_fid: appOf(i) }]));
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const params = new URL(route.request().url()).searchParams;
    return route.fulfill({ json: { pool: params.get("offset") ? [] : pool, notes: { origins, apps }, meta } });
  });
}

test("a client in the usage bar filters the feed; the filter is a removable chip kept in the URL", async ({ page }) => {
  await serve(page);
  await page.goto("/?sort=top");
  const cards = page.locator(".cast-card:not(.skeleton)");
  await expect(cards).toHaveCount(25);
  const legend = page.getByRole("region", { name: "Apps posting the past 24 hours' trending casts" }).locator(".client-legend");
  await legend.getByRole("link", { name: /Base App/ }).click();
  await expect(page).toHaveURL(/[?&]client=309857/);
  await expect(page).toHaveURL(/sort=top/);
  await expect(cards).toHaveCount(6);
  await expect(page.locator(".client-chip")).toContainText("Base App");
  await expect(page.getByRole("link", { name: "hot" })).toHaveAttribute("href", /client=309857/);
  await page.locator(".client-chip").click();
  await expect(page).not.toHaveURL(/client=/);
  await expect(page).toHaveURL(/sort=top/);
  await expect(cards).toHaveCount(25);
  await expect(page.locator(".client-chip")).toHaveCount(0);
});

test("Other apps lists the smaller clients, and each one filters", async ({ page }, info) => {
  await serve(page);
  await page.goto("/");
  const other = page.getByRole("button", { name: "Other apps" });
  if (info.project.use.hasTouch) await other.tap(); else await other.hover();
  const list = page.getByRole("dialog", { name: "Other apps" });
  await expect(list.getByRole("link")).toHaveCount(2);
  await list.getByRole("link", { name: /Tiny app/ }).click();
  await expect(page).toHaveURL(/client=7777/);
  await expect(page.locator(".cast-card:not(.skeleton)")).toHaveCount(2);
  await expect(page.locator(".client-chip")).toContainText("Tiny app");
  // The list closes after a pick, so it doesn't cover the chip.
  await expect(list).toBeHidden();
});
