// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const cast = (id: number, likes: number, time: number) => ({
  hash: `0x${id.toString(16).padStart(40, "0")}`, author: { fid: 1, username: "fixture" },
  text: `Feed fixture ${id}`, ts: 1792235681 + time, likes,
});
const first = [cast(1, 20, 100), cast(2, 5, 300)];
const next = [cast(3, 10, 400), cast(4, 30, 200)];
const meta = { backend: "hypersnap", fetched_at: Date.now() };

test.beforeEach(async ({ page }) => {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home", (route) => route.fulfill({ json: { pool: first, meta } }));
  await page.route("**/api/v1/channels", (route) => route.fulfill({ json: { channels: [], meta } }));
  await page.route("**/api/v1/search?*", (route) => route.fulfill({ json: { query: "fixture", users: [], channels: [], casts: [], meta } }));
});

test("sorting reorders every loaded trending page without rebuilding cards or fetching again", async ({ page }) => {
  const requests: string[] = [];
  page.on("request", (request) => { if (request.url().includes("/api/v1/home")) requests.push(request.url()); });
  await page.route("**/api/v1/home?offset=100", (route) => route.fulfill({ json: { pool: next, meta } }));
  const end = page.waitForResponse((response) => response.url().endsWith("/api/v1/home?offset=200"));
  await page.goto("/");
  await expect(page.locator(".cast-card[data-hash]")).toHaveCount(4);
  await end;
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  const count = requests.length;
  const firstCard = await page.locator('.cast-card[data-hash="' + first[0].hash + '"]').elementHandle();
  const sort = page.getByRole("navigation", { name: "Sort", exact: true });
  await sort.getByRole("link", { name: "new", exact: true }).click();
  await expect.poll(() => page.locator(".cast-card[data-hash]").evaluateAll((cards) => cards.map((c) => Number(c.getAttribute("data-ts")))), { timeout: 1500 })
    .toEqual([300, 100, 400, 200].map((time) => 1792235681 + time));
  await sort.getByRole("link", { name: "top", exact: true }).click();
  await expect.poll(() => page.locator(".cast-card[data-hash]").evaluateAll((cards) => cards.map((c) => Number(c.getAttribute("data-points")))), { timeout: 1500 })
    .toEqual([20, 5, 30, 10]);
  expect(await firstCard!.evaluate((card) => card.isConnected)).toBe(true);
  expect(requests).toHaveLength(count);
});

test("appending an overlapping page preserves existing cards and renders each new hash once", async ({ page }) => {
  let release!: () => void;
  const pending = new Promise<void>((resolve) => { release = resolve; });
  let arrived!: () => void;
  const requested = new Promise<void>((resolve) => { arrived = resolve; });
  await page.route("**/api/v1/home?offset=100", async (route) => {
    arrived();
    await pending;
    await route.fulfill({ json: { pool: [...first, ...next, next[0]], meta } });
  });
  await page.goto("/");
  await requested;
  const cards = page.locator(".cast-card[data-hash]");
  const original = await cards.first().elementHandle();
  const order = await cards.evaluateAll((items) => items.map((item) => item.getAttribute("data-hash")));
  release();
  await expect(cards).toHaveCount(4);
  expect(await cards.evaluateAll((items) => items.slice(0, 2).map((item) => item.getAttribute("data-hash")))).toEqual(order);
  expect(await original!.evaluate((item) => item.isConnected)).toBe(true);
});

for (const status of [200, 503]) {
  test(`leaving the feed while a later page returns ${status} does not use disposed state`, async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    let release!: () => void;
    const pending = new Promise<void>((resolve) => { release = resolve; });
    let arrived!: () => void;
    const requested = new Promise<void>((resolve) => { arrived = resolve; });
    await page.route("**/api/v1/home?offset=100", async (route) => {
      arrived();
      await pending;
      await route.fulfill({ status, json: status === 200 ? { pool: next, meta } : { error: "Busy" } });
    });
    await page.goto("/");
    await requested;
    const search = page.getByRole("searchbox", { name: "Search casts, people and channels", exact: true });
    await search.fill("fixture");
    await search.press("Enter");
    await expect(page).toHaveURL(/\/search\?q=fixture$/);
    const response = page.waitForResponse((response) => response.url().includes("/home?offset=100"));
    release();
    await response;
    // Let the asynchronous fetch continuation and any error event run.
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    expect(errors).toEqual([]);
    await page.getByRole("link", { name: "Allthing, home", exact: true }).click();
    await expect(page.locator(".cast-card[data-hash]")).not.toHaveCount(0);
  });
}

test("later-page annotations survive navigation back to the feed", async ({ page }) => {
  const later = { ...next[0], author: { fid: 2, username: "later" } };
  await page.route("**/api/v1/home", (route) => route.fulfill({ json: { pool: first, notes: { scores: { "1": 500000 } }, meta } }));
  await page.route("**/api/v1/home?offset=100", (route) => route.fulfill({ json: {
    pool: [later], meta, notes: {
      scores: { "2": 910000 }, origins: { [later.hash]: { signer: "0xabc", app_fid: 99 } },
      apps: { "99": { fid: 99, username: "posting-app", display_name: "Fixture app" } },
    },
  } }));
  await page.goto("/");
  const card = page.locator(`.cast-card[data-hash="${later.hash}"]`);
  await expect(card).toBeVisible();
  await expect(card.locator(".score")).toHaveText("0.91", { timeout: 1500 });
  await expect(card.locator(".origin")).toContainText("via Fixture app");
  await expect(page.locator(`.cast-card[data-hash="${first[0].hash}"] .score`)).toHaveText("0.50");
  const search = page.getByRole("searchbox", { name: "Search casts, people and channels", exact: true });
  await search.fill("fixture");
  await search.press("Enter");
  await expect(page).toHaveURL(/\/search\?q=fixture$/);
  await page.goBack();
  await expect(card.locator(".score")).toHaveText("0.91");
  await expect(card.locator(".origin")).toContainText("via Fixture app");
});

test("an empty filtered page advances to the next page without another scroll", async ({ page }) => {
  await page.route("**/api/v1/home?offset=100", (route) => route.fulfill({ json: { pool: [], meta } }));
  await page.route("**/api/v1/home?offset=200", (route) => route.fulfill({ json: { pool: [next[0]], meta } }));
  await page.goto("/");
  await expect(page.locator(`.cast-card[data-hash="${next[0].hash}"]`)).toBeVisible({ timeout: 5000 });
});

test("empty later pages stop at the trending pagination limit", async ({ page }) => {
  const offsets: number[] = [];
  await page.route("**/api/v1/home?offset=*", (route) => {
    offsets.push(Number(new URL(route.request().url()).searchParams.get("offset")));
    return route.fulfill({ json: { pool: [], meta } });
  });
  await page.goto("/");
  await expect.poll(() => offsets, { timeout: 5000 }).toEqual([100, 200, 300, 400, 500, 600, 700, 800, 900]);
  await expect(page.locator(".cast-card[data-hash]")).toHaveCount(2);
  await page.waitForTimeout(200);
  expect(offsets).toHaveLength(9);
});

test("a failed later page observes its retry delay while the end stays visible", async ({ page }) => {
  const attempts: number[] = [];
  await page.route("**/api/v1/home?offset=100", (route) => {
    attempts.push(Date.now());
    return route.fulfill({ status: attempts.length === 1 ? 503 : 200,
      json: attempts.length === 1 ? { error: "Busy" } : { pool: next, meta } });
  });
  await page.goto("/");
  await expect.poll(() => attempts.length).toBe(1);
  await page.waitForTimeout(300);
  expect(attempts).toHaveLength(1);
  await expect(page.locator(`.cast-card[data-hash="${next[0].hash}"]`)).toBeVisible();
  expect(attempts).toHaveLength(2);
  expect(attempts[1] - attempts[0]).toBeGreaterThanOrEqual(1900);
});
