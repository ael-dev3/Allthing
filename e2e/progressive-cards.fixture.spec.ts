// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const now = Math.floor(Date.now() / 1000);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const author = (i: number) => ({ fid: 100 + i, username: `author${i}`, display_name: `Author ${i}` });
// Hot and top order these differently: likes grow down the list, age too.
const pool = Array.from({ length: 40 }, (_, i) => ({
  hash: hex(i + 1),
  author: author(i),
  text: `Cast number ${i + 1}`,
  ts: now - 600 * (i + 1),
  likes: (i * 7) % 40,
  recasts: i % 3,
  replies: i % 5,
}));
const latest = pool.slice(0, 30).map((c, i) => ({ ...c, hash: hex(1000 + i), ts: now - i }));

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : pool, notes: {}, meta } });
  });
  await page.route("**/api/v1/latest", (route) => route.fulfill({ json: { pool: latest, notes: {}, cursors: [], meta } }));
}

/** The chosen tab and the first card in the frame after the next click, before it paints. */
async function firstFrame(page: Page, click: () => Promise<void>) {
  await page.evaluate(() => {
    (window as any).__firstFrame = null;
    document.addEventListener(
      "click",
      () =>
        requestAnimationFrame(() => ((window as any).__firstFrame = {
          tab: document.querySelector(".sort-tabs a.selected")?.textContent?.trim() ?? "",
          first: document.querySelector(".cast-card:not(.skeleton)")?.getAttribute("data-hash") ?? "",
        })),
      { capture: true, once: true },
    );
  });
  await click();
  await expect.poll(() => page.evaluate(() => (window as any).__firstFrame)).not.toBeNull();
  return page.evaluate(() => (window as any).__firstFrame as { tab: string; first: string });
}

/** How many cards the list holds when a card other than `from` first leads it. */
function drawnAtFirst(page: Page, from: string) {
  return page.evaluate(
    (from) =>
      new Promise<number>((resolve) => {
        new MutationObserver((_, observer) => {
          const shown = document.querySelectorAll(".cast-card:not(.skeleton)");
          if (shown.length && shown[0].getAttribute("data-hash") !== from) {
            observer.disconnect();
            resolve(shown.length);
          }
        }).observe(document.body, { childList: true, subtree: true });
      }),
    from,
  );
}

const cards = (page: Page) => page.locator(".cast-card:not(.skeleton)");
const tab = (page: Page, name: string) => page.locator(".sort-tabs a", { hasText: name });

test("a sort tab shows as chosen in the frame after the tap; the cards follow, a screenful first", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  await expect(cards(page)).toHaveCount(25);
  const hot = (await cards(page).first().getAttribute("data-hash"))!;

  // Re-sorting the same list: the tab at once, the old order for that
  // frame, then the new order, a screenful drawn first, then all 25.
  let drawn = drawnAtFirst(page, hot);
  expect(await firstFrame(page, () => tab(page, "top").click())).toEqual({ tab: "top", first: hot });
  expect(await drawn).toBeLessThanOrEqual(6);
  await expect(cards(page)).toHaveCount(25);
  expect(await cards(page).first().getAttribute("data-hash")).not.toBe(hot);

  // Another list, its data already in the client cache: the same.
  const top = (await cards(page).first().getAttribute("data-hash"))!;
  await tab(page, "new").click();
  await expect(cards(page).first()).not.toHaveAttribute("data-hash", top);
  await expect(cards(page)).toHaveCount(25);
  const newest = (await cards(page).first().getAttribute("data-hash"))!;
  drawn = drawnAtFirst(page, newest);
  expect(await firstFrame(page, () => tab(page, "hot").click())).toEqual({ tab: "hot", first: newest });
  expect(await drawn).toBeLessThanOrEqual(6);
  await expect(cards(page)).toHaveCount(25);
  expect(await cards(page).first().getAttribute("data-hash")).toBe(hot);
});

test("scrolling adds the next cards and keeps going to the end of the list", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  await expect(cards(page)).toHaveCount(25);
  await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
  await expect(cards(page)).toHaveCount(40);
  await expect(page.locator(".cast-card").last()).toContainText("Cast number");
});
