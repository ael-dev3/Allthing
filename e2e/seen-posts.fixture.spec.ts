// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { NOW } from "./fixture-time";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const KEY = "allthing.seen.v1";
const hash = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const person = { fid: 900, username: "fixture", display_name: "Fixture" };
const pool = Array.from({ length: 32 }, (_, i) => ({
  hash: hash(i + 1), author: person, text: `Reading fixture ${i + 1}. ` + "This cast has a little room to read. ".repeat(5),
  ts: NOW - 60 - i * 60, likes: 32 - i, replies: 0, recasts: 0,
}));
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const card = (page: Page, n: number) => page.locator(`.cast-card[data-hash="${hash(n)}"]`);
const seeded = (scopes: Array<{ name: string; hashes: number[] }>) => ({
  version: 1, scopes: scopes.map(({ name, hashes }) => ({ name, touched: NOW, posts: hashes.map((n) => ({ hash: hash(n), at: NOW })) })),
});
async function serve(page: Page, casts = pool) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture" } }));
  await page.route("**/api/v1/home**", (route) => route.fulfill({ json: { pool: new URL(route.request().url()).searchParams.has("offset") ? [] : casts, notes: {}, meta } }));
  await page.route("**/api/v1/latest**", (route) => route.fulfill({ json: { pool: casts, notes: {}, meta } }));
  await page.route("**/api/v1/following**", (route) => route.fulfill({ json: { pool: casts, notes: {}, meta } }));
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: { user: person, pool: casts, notes: {}, meta } }));
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: { thread: { cast: casts[0], children: [] }, ancestors: [], quotes: [], notes: {}, meta } }));
}
async function hashes(page: Page, name = "anonymous"): Promise<string[]> {
  return page.evaluate(([key, name]) => {
    const value = JSON.parse(localStorage.getItem(key) ?? '{"scopes":[]}');
    return value.scopes.find((s: { name: string }) => s.name === name)?.posts.map((p: { hash: string }) => p.hash) ?? [];
  }, [KEY, name]);
}
async function reset(page: Page) {
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "Settings", exact: true });
  await settings.getByRole("button", { name: "Reset seen posts", exact: true }).click();
  await expect(settings.getByRole("status")).toContainText("Seen history reset");
}

for (const width of [1280, 390]) {
  test(`only foreground viewport dwell counts, and the current feed stays put at ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    await serve(page);
    await page.goto("/");
    await expect(card(page, 1)).toBeVisible();
    // At least a second in the actual viewport, then the debounced local write.
    await expect.poll(() => hashes(page)).toContain(hash(1));
    expect(await hashes(page)).not.toContain(hash(32));
    await expect(card(page, 1)).toBeVisible();
    await page.reload();
    await expect(card(page, 1)).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Show seen posts", exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Show seen posts", exact: true }).click();
    await expect(card(page, 1)).toBeVisible();
    await page.goto(`/p/${hash(1)}`);
    await expect(page.locator("article.post")).toContainText("Reading fixture 1.");
    await page.goto("/u/fixture");
    await expect(card(page, 1)).toBeVisible();
  });
}

test("a sort paints its selected tab before suppressing the previous generation's read cards", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  await expect(page.locator("#app .cast-card[data-hash]")).toHaveCount(25);
  await expect.poll(() => hashes(page)).toContain(hash(1));
  const firstFrame = await page.evaluate(() => new Promise<{ retained: boolean; selected: boolean; count: number }>((resolve) => {
    const old = document.querySelector("#app .cast-card[data-hash]")!;
    const tab = document.querySelector<HTMLAnchorElement>('#app .sort-tabs a[href*="sort=top"]')!;
    // Queue this before the router handles the click. The first animation
    // frame observes the tap's answer; the old list's cleanup comes later.
    tab.addEventListener("click", () => requestAnimationFrame(() => resolve({
      retained: old.isConnected,
      selected: document.querySelector("#app .sort-tabs a.selected")?.textContent === "top",
      count: document.querySelectorAll("#app .cast-card[data-hash]").length,
    })), { once: true, capture: true });
    tab.click();
  }));
  expect(firstFrame).toEqual({ retained: true, selected: true, count: 25 });
  // Deferring cleanup still applies reading history to the next generation.
  await expect(card(page, 1)).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Show seen posts", exact: true })).toBeVisible();
});

test("a card taller than two screens counts after a full viewport dwell, not from a sliver", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  // Enough preceding rows to place the card below 800 document pixels:
  // otherwise scrollTo(top - 800) clamps to zero and leaves most of it visible.
  const tall = { ...pool[9], text: "A long cast and its discussion.\n\n".repeat(110) };
  await serve(page, [...pool.slice(0, 9), tall, ...pool.slice(10, 13)]);
  await page.goto("/");
  const reading = card(page, 10);
  await reading.scrollIntoViewIfNeeded();
  await expect.poll(async () => (await reading.boundingBox())!.height).toBeGreaterThan(2 * 844);
  const top = await reading.evaluate((element) => element.getBoundingClientRect().top + scrollY);
  expect(top).toBeGreaterThan(800);
  const exposure = () => reading.evaluate((element) => {
    const r = element.getBoundingClientRect();
    const visible = Math.max(0, Math.min(r.bottom, innerHeight) - Math.max(r.top, 0));
    return visible / Math.min(r.height, innerHeight);
  });
  await page.evaluate((top) => scrollTo(0, top - 800), top);
  await expect.poll(exposure).toBeLessThan(0.1);
  await page.waitForTimeout(1600);
  expect(await hashes(page)).not.toContain(hash(10));
  await reading.evaluate((element) => scrollTo(0, element.getBoundingClientRect().top + scrollY));
  await expect.poll(exposure).toBeGreaterThanOrEqual(0.5);
  await expect.poll(() => hashes(page)).toContain(hash(10));
  await expect(reading).toBeVisible();
});

test("a brief visit and detached kept Home do not mark loaded posts", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  await expect(card(page, 1)).toBeVisible();
  await page.goto(`/p/${hash(1)}`);
  await expect(page.locator("article.post")).toBeVisible();
  await page.waitForTimeout(1700);
  expect(await hashes(page)).toEqual([]);
});

test("new keeps chronological order after seen suppression; caught up offers show and reset", async ({ page }) => {
  await serve(page, pool.slice(0, 4));
  await page.addInitScript(({ key, history }) => localStorage.setItem(key, JSON.stringify(history)), { key: KEY, history: seeded([{ name: "anonymous", hashes: [1, 3] }]) });
  await page.goto("/?sort=new");
  await expect(card(page, 2)).toBeVisible();
  await expect(card(page, 1)).toHaveCount(0);
  expect(await page.locator(".cast-card[data-hash]").evaluateAll((items) => items.map((item) => item.getAttribute("data-hash")))).toEqual([hash(2), hash(4)]);
  await reset(page);
  await expect(card(page, 1)).toBeVisible();
  expect(await page.locator(".cast-card[data-hash]").evaluateAll((items) => items.map((item) => item.getAttribute("data-hash")))).toEqual([1, 2, 3, 4].map(hash));
});

test("all caught up stays recoverable and records nothing from its empty feed", async ({ page }) => {
  await serve(page, pool.slice(0, 2));
  await page.addInitScript(({ key, history }) => localStorage.setItem(key, JSON.stringify(history)), { key: KEY, history: seeded([{ name: "anonymous", hashes: [1, 2] }]) });
  await page.goto("/");
  await expect(page.locator(".seen-notice")).toContainText("You're caught up");
  await expect(page.locator(".cast-card[data-hash]")).toHaveCount(0);
  await page.getByRole("button", { name: "Show seen posts", exact: true }).click();
  await expect(card(page, 1)).toBeVisible();
});

test("account changes isolate history and cancel the previous account's dwell timer", async ({ page }) => {
  await serve(page, pool.slice(0, 4));
  await page.addInitScript(({ key, history }) => localStorage.setItem(key, JSON.stringify(history)), { key: KEY, history: seeded([{ name: "anonymous", hashes: [1] }, { name: "fid:11", hashes: [2] }, { name: "fid:22", hashes: [3] }]) });
  await page.goto("/");
  await expect(card(page, 1)).toHaveCount(0);
  await expect(card(page, 2)).toBeVisible();
  for (const [fid, hidden, shown] of [[11, 2, 1], [22, 3, 2]]) {
    await page.evaluate((fid) => {
      document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify({ fid, username: `reader${fid}` }))}; Path=/; Secure; SameSite=Lax`;
      const channel = new BroadcastChannel("allthing-session");
      channel.postMessage("changed");
      channel.close();
    }, fid);
    await expect(page.getByRole("link", { name: `Signed in as @reader${fid}: your profile`, exact: true })).toBeVisible();
    await expect(card(page, hidden)).toHaveCount(0);
    await expect(card(page, shown)).toBeVisible();
  }
  await expect.poll(() => hashes(page, "fid:22")).toContain(hash(2));
  // The first account's seeded history is not overwritten by the second.
  expect(await hashes(page, "fid:11")).toEqual([hash(2)]);
});

test("a reset in another tab discards this tab's pre-reset pending write", async ({ page, context }) => {
  const other = await context.newPage();
  await serve(other, []);
  await other.goto("/channels");
  await other.getByRole("button", { name: "Settings", exact: true }).click();
  // Stretch only debounce timers so the browser's real one-second dwell
  // has completed, while its local write is still pending at the reset.
  await page.addInitScript(() => {
    const later = window.setTimeout.bind(window);
    window.setTimeout = ((callback: TimerHandler, delay?: number, ...args: unknown[]) => later(callback, delay === 500 ? 5000 : delay, ...args)) as typeof window.setTimeout;
  });
  await serve(page, pool.slice(0, 2));
  await page.bringToFront();
  await page.goto("/");
  await expect(card(page, 1)).toBeVisible();
  await page.waitForTimeout(1300);
  await other.getByRole("dialog", { name: "Settings", exact: true }).getByRole("button", { name: "Reset seen posts", exact: true }).click();
  await expect(other.getByRole("dialog", { name: "Settings", exact: true }).getByRole("status")).toContainText("Seen history reset");
  await page.waitForTimeout(5100);
  expect(await hashes(page)).toEqual([]);
  await other.close();
});

for (const failure of ["malformed", "denied"]) {
  test(`${failure} local storage does not break the feed or leak history to a request`, async ({ page }) => {
    const errors: string[] = [], leaked: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    page.on("request", (request) => { if (request.url().includes(KEY) || request.postData()?.includes(KEY)) leaked.push(request.url()); });
    await serve(page, pool.slice(0, 2));
    await page.addInitScript(({ key, failure }) => {
      if (failure === "malformed") localStorage.setItem(key, "broken JSON");
      else {
        const get = Storage.prototype.getItem, set = Storage.prototype.setItem;
        Storage.prototype.getItem = function(name) { if (name === key) throw new DOMException("Denied", "SecurityError"); return get.call(this, name); };
        Storage.prototype.setItem = function(name, value) { if (name === key) throw new DOMException("Denied", "QuotaExceededError"); return set.call(this, name, value); };
      }
    }, { key: KEY, failure });
    await page.goto("/");
    await expect(card(page, 1)).toBeVisible();
    await page.waitForTimeout(1700);
    await reset(page);
    await expect(card(page, 1)).toBeVisible();
    expect(errors).toEqual([]);
    expect(leaked).toEqual([]);
  });
}
