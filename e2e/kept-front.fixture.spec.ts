// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { appCame } from "./helpers/app";

/*
 * The front page is kept while another page shows (kept.rs): Back shows it
 * at once, as it was (the same cards, where they were, in the order and
 * the sort it had), with no request and nothing drawn again, and nothing of
 * it is in the other page.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = Math.floor(Date.now() / 1000) - 600;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const author = (n: number) => ({ fid: 100 + (n % 7), username: `author${n % 7}`, display_name: `Author ${n % 7}` });
const feed = Array.from({ length: 30 }, (_, i) => ({
  hash: hex(i + 1), author: author(i), text: `Cast number ${i + 1}, long enough to take a line or two of the card.`,
  ts: ts - i * 300, likes: 30 - i, replies: 1,
}));

/** A frame, as the test samples each: the page shown, its height, the card's top and the scroll. */
type Frame = { path: string; height: number; top: number | null; y: number };

async function serve(page: Page) {
  const asked: string[] = [];
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    asked.push(later ? "home later" : "home");
    return route.fulfill({ json: { pool: later ? [] : feed, notes: {}, meta } });
  });
  await page.route("**/api/v1/latest**", (route) => {
    asked.push("latest");
    return route.fulfill({ json: { pool: [...feed].reverse(), notes: {}, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash");
    const cast = feed.find((c) => c.hash === hash) ?? feed[0];
    return route.fulfill({ json: { thread: { cast, children: [] }, notes: {}, meta } });
  });
  return asked;
}

for (const size of [
  { name: "at 1280", use: { viewport: { width: 1280, height: 900 } } },
  { name: "at 390", use: { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true } },
]) {
  test.describe(size.name, () => {
    test.use(size.use);

    test("Back shows the front page as it was: the same cards, where they were, with no request", async ({ page }) => {
      const asked = await serve(page);
      await page.goto("/");
      await appCame(page);
      const cards = page.locator("#app .cast-card[data-hash]");
      await expect(cards.first()).toBeVisible();
      // Down the feed, to a card under the header, once the cards have
      // risen into place (drawn as the app came, the last of them rise for
      // up to 450 ms).
      await page.evaluate(() => window.scrollTo(0, 1400));
      await page.waitForTimeout(400);
      await expect.poll(() => page.evaluate(() => [...document.querySelectorAll("#app .cast-card")].some((c) => c.getAnimations().length > 0))).toBe(false);
      const before = await page.evaluate(() => {
        // The first card wholly under the header.
        const header = document.querySelector("#app .site-header")!.getBoundingClientRect().bottom;
        const card = [...document.querySelectorAll("#app .cast-card[data-hash]")]
          .find((c) => c.getBoundingClientRect().top > header + 8) as HTMLElement & { kept?: boolean };
        card.kept = true;
        return { hash: card.dataset.hash!, top: card.getBoundingClientRect().top, count: document.querySelectorAll("#app .cast-card").length };
      });
      const requests = asked.length;
      // Each frame from here on: the page shown, how tall it is, where the
      // card is, and how far the page is scrolled.
      await page.evaluate((hash) => {
        const w = window as Window & { __frames?: Frame[] };
        w.__frames = [];
        const frame = () => {
          const card = document.querySelector(`#app .cast-card[data-hash="${hash}"]`);
          w.__frames!.push({ path: location.pathname, height: document.documentElement.scrollHeight, top: card?.getBoundingClientRect().top ?? null, y: scrollY });
          requestAnimationFrame(frame);
        };
        requestAnimationFrame(frame);
      }, before.hash);
      await page.locator(`#app .cast-card[data-hash="${before.hash}"] .cast-text`).click();
      await expect(page.locator("#app .post")).toBeVisible();
      // None of the front page is in the post's page.
      await expect(page.locator("#app .cast-list .cast-card")).toHaveCount(0);
      await page.goBack();
      await expect(page).toHaveURL(/\/$/);
      const after = () => page.evaluate((hash) => {
        const card = document.querySelector(`#app .cast-card[data-hash="${hash}"]`) as HTMLElement & { kept?: boolean };
        return { same: card.kept === true, top: card.getBoundingClientRect().top, count: document.querySelectorAll("#app .cast-card").length };
      }, before.hash);
      const frames = () => page.evaluate(() => (window as Window & { __frames?: Frame[] }).__frames!);
      // Where the reader left the card: its last frame before the post. Not
      // where it was before the click, which can scroll the page first: a
      // card not yet still as Playwright checks it is scrolled into view
      // again, to the bottom of the screen (on CI, 646 px up).
      const sampled = await frames();
      const left = sampled.findIndex((f) => f.path !== "/");
      expect(left, "a frame of the post").toBeGreaterThan(0);
      const there = sampled[left - 1];
      // Back where it was, within the frames the place takes to come back.
      try {
        await expect.poll(async () => Math.abs((await after()).top - there.top!), { timeout: 3000 }).toBeLessThanOrEqual(2);
      } catch (error) {
        const all = await frames();
        const back = all.findIndex((f, i) => i > left && f.path === "/");
        throw new Error(`${(error as Error).message}\nthe frames leaving the front page, and from Back: ${JSON.stringify([...all.slice(Math.max(0, left - 2), left + 1), ...all.slice(back, back + 30)])}`);
      }
      // As tall as it was, from its first frame back: each card the height
      // it had, so nothing moved under the reader as it was put back.
      const shown = await frames();
      const returned = shown.findIndex((f, i) => i > left && f.path === "/" && f.top !== null);
      expect(returned, "a frame of the front page again").toBeGreaterThan(left);
      expect(shown[returned].height, `its height in its first frames back: ${JSON.stringify(shown.slice(returned, returned + 6))}`).toBe(there.height);
      const back = await after();
      expect(back.same, "the card is the one that was on the page").toBe(true);
      expect(back.count).toBe(before.count);
      expect(asked.length, "nothing asked for again").toBe(requests);
      await expect(page).toHaveTitle("Allthing");
    });
  });
}

test("the front page kept keeps its sort, and another page's address doesn't redraw it", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  const asked = await serve(page);
  await page.goto("/?sort=new");
  await appCame(page);
  const first = page.locator("#app .cast-list .cast-card[data-hash]").first();
  await expect(first).toHaveAttribute("data-hash", feed[0].hash);
  await first.evaluate((card) => ((card as HTMLElement & { kept?: boolean }).kept = true));
  const requests = asked.length;
  await first.locator(".cast-text").click();
  await expect(page.locator("#app .post")).toBeVisible();
  await page.goBack();
  await expect(page).toHaveURL(/\/\?sort=new$/);
  await expect(page.locator("#app .sort-tabs a.selected")).toHaveText("new");
  expect(await first.evaluate((card) => (card as HTMLElement & { kept?: boolean }).kept)).toBe(true);
  await expect(page).toHaveTitle("new · Allthing");
  expect(asked.length).toBe(requests);
});
