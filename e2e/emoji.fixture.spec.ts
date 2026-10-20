// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";

/*
 * Allthing's emoji: ":pepe_no:" and the rest of the pack, shown inline in
 * cast text from the site's own files, large when a cast is only emoji,
 * and as stills for readers who ask for less motion.
 */

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const casts = [
  { hash: hex(1), author: alice, text: "no thanks :pepe_no: (gm :pepe_gm:)", ts, likes: 1, replies: 0 },
  { hash: hex(2), author: alice, text: ":pepe_heart_hug: :pepe_blush:", ts: ts - 60, likes: 1, replies: 0 },
  { hash: hex(3), author: alice, text: "at 10:30:45 :smile: :pepe_nope:", ts: ts - 120, likes: 1, replies: 0 },
];
const card = (page: Page, i: number) => page.locator(`.cast-card[data-hash="${casts[i].hash}"]`);

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : casts, notes: {}, meta } });
  });
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  return { context, page: await context.newPage() };
}

/** Whether the image has loaded, and how tall it shows. */
const loaded = (img: ReturnType<Page["locator"]>) =>
  img.evaluate(async (el: HTMLImageElement) => {
    if (!el.complete) await new Promise((done) => el.addEventListener("load", done, { once: true }));
    return { natural: el.naturalWidth, height: el.getBoundingClientRect().height, src: el.currentSrc };
  });

test("shortcodes of the pack show as the emoji, inline; anything else stays text", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  const no = card(page, 0).getByRole("img", { name: ":pepe_no:" });
  await expect(no).toHaveAttribute("title", "Pepe holds up a NO sign");
  const shown = await loaded(no);
  expect(shown.natural).toBeGreaterThan(0);
  expect(shown.src).toMatch(/\/assets\/emoji\/v1\/pepe_no\.webp$/);
  // About the height of the text's line, not more.
  expect(shown.height).toBeGreaterThan(20);
  expect(shown.height).toBeLessThan(28);
  await expect(card(page, 0).locator(".cast-text")).toContainText("no thanks");
  // An animated one plays.
  expect((await loaded(card(page, 0).getByRole("img", { name: ":pepe_gm:" }))).src).toMatch(/pepe_gm\.webp$/);
  // A cast of emoji alone shows them large.
  const hug = card(page, 1).getByRole("img", { name: ":pepe_heart_hug:" });
  expect((await loaded(hug)).height).toBe(48);
  // Times, other apps' shortcodes and names the pack hasn't stay as typed.
  await expect(card(page, 2).locator(".cast-text")).toHaveText("at 10:30:45 :smile: :pepe_nope:");
  await expect(card(page, 2).locator(".emoji")).toHaveCount(0);
});

test("readers who ask for less motion get stills", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await serve(page);
  await page.goto("/");
  const gm = await loaded(card(page, 0).getByRole("img", { name: ":pepe_gm:" }));
  expect(gm.src).toMatch(/\/assets\/emoji\/v1\/pepe_gm\.still\.webp$/);
  expect(gm.natural).toBeGreaterThan(0);
  // A still one has no other file.
  expect((await loaded(card(page, 0).getByRole("img", { name: ":pepe_no:" }))).src).toMatch(/pepe_no\.webp$/);
});

test("at 390, emoji fit their line and the page", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await serve(page);
  await page.goto("/");
  const no = await loaded(card(page, 0).getByRole("img", { name: ":pepe_no:" }));
  expect(no.natural).toBeGreaterThan(0);
  expect(no.height).toBeLessThan(28);
  expect((await loaded(card(page, 1).getByRole("img", { name: ":pepe_blush:" }))).height).toBe(48);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await context.close();
});

test("the pack is served from the site itself, cached for a year", async ({ request }) => {
  test.skip(Boolean(process.env.E2E_PAGES), "GitHub Pages sets its own caching");
  const file = await request.get("/assets/emoji/v1/pepe_no.webp");
  expect(file.status()).toBe(200);
  expect(file.headers()["content-type"]).toBe("image/webp");
  expect(file.headers()["cache-control"]).toContain("immutable");
});
