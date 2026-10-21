// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";

/*
 * On a touch screen, the bands of a card's and a reply's counts and buttons
 * (44 px tall) stay clear of what is above the row: a photo, a link card, a
 * quote, or the last line of the words. A tap on that thing's lower edge is
 * its own, where the like's or the recast's band took 3 px of it (4 px under
 * a reply). With a mouse, nothing changes.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = Math.floor(Date.now() / 1000) - 3 * 3600;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
/** A photo 1200 by 800, as an image host would serve one. */
const photo = `<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="800"><rect width="1200" height="800" fill="#4f43d4"/></svg>`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const image = { kind: "image", url: "https://media.example/photo.svg" };
const link = (url: string, title: string) => ({ kind: "link", url, title, image_url: null });
const cast = (n: number, text: string, embeds: object[] = []) =>
  ({ hash: hex(n), author: alice, text, ts: ts - n * 60, likes: 3, recasts: 1, replies: 2, embeds });
const quoted = cast(9, "The quoted cast.");
const pool = [
  cast(1, "", [image]),
  cast(2, "With a photo under the words.", [image]),
  cast(3, "With a link card.", [link("https://example.com/a-post", "A post on example.com")]),
  cast(4, "Quoting a cast.", [{ kind: "cast", hash: quoted.hash, fid: alice.fid }]),
  cast(5, "Ends with a link https://example.com/x"),
];
const reply = (n: number, text: string, embeds: object[] = []) =>
  ({ cast: { ...cast(n, text, embeds), parent_hash: pool[1].hash }, children: [] });

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : pool, notes: { quotes: { [quoted.hash]: quoted } }, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
    thread: { cast: pool[1], children: [reply(20, "", [image]), reply(21, "A reply with a link card.", [link("https://example.com/b", "B")])] },
    notes: {}, meta,
  } }));
  await page.route("https://media.example/**", (route) =>
    route.fulfill({ body: photo, contentType: "image/svg+xml", headers: { "access-control-allow-origin": "*" } }));
}

async function at(browser: Browser, testInfo: TestInfo, touch: boolean) {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL, viewport: { width: touch ? 390 : 1280, height: 900 }, isMobile: touch, hasTouch: touch,
  });
  return { context, page: await context.newPage() };
}

/** Each row of counts and buttons: what is above it, and how far the top of its bands is below that. */
async function clearances(page: Page, feet: string) {
  return page.locator(feet).evaluateAll((all) => all.map((foot) => {
    const above = foot.previousElementSibling!;
    const tops = [...foot.querySelectorAll("button, a[href]")].map((el) => {
      const box = el.getBoundingClientRect();
      const band = getComputedStyle(el, "::after");
      if (band.content === "none" || band.position !== "absolute") return box.top;
      const m = band.transform && band.transform !== "none" ? new DOMMatrixReadOnly(band.transform) : new DOMMatrixReadOnly();
      return Math.min(box.top, box.top + (el as HTMLElement).clientTop + parseFloat(band.top) + m.f);
    });
    return { above: above.className, clearance: Math.min(...tops) - above.getBoundingClientRect().bottom };
  }));
}

test("on a touch screen, a card's and a reply's bands stay clear of the photo, link card, quote or words above them", async ({ browser }, testInfo) => {
  const { context, page } = await at(browser, testInfo, true);
  await serve(page);
  await page.goto("/");
  await expect(page.locator("#app .cast-card:not(.skeleton)")).toHaveCount(pool.length);
  await expect(page.locator("#app .cast-card .tile-image img").first()).toBeVisible();
  const cards = await clearances(page, "#app .cast-card > .cast-foot");
  expect(cards.map((c) => c.above.split(" ")[0])).toEqual(["media-grid", "media-grid", "link-card", "quote", "cast-text"]);
  // 2 px clear: a font a little taller elsewhere can't close it.
  for (const { above, clearance } of cards) expect(clearance, `a card's bands under its ${above}`).toBeGreaterThanOrEqual(1.5);
  // A tap on a photo's lower edge, over the counts and their buttons (clear
  // of its rounded corner), is the photo's.
  const edge = await page.locator(`#app .cast-card[data-hash="${pool[0].hash}"]`).evaluate((card) => {
    const tile = card.querySelector(".tile-image")!.getBoundingClientRect();
    const row = card.querySelector(".engagement")!.getBoundingClientRect();
    const missed: string[] = [];
    for (let x = tile.left + 16; x < row.right; x += 8) {
      const hit = document.elementFromPoint(x, tile.bottom - 1);
      if (!hit?.closest(".tile-image")) missed.push(`${Math.round(x)}: ${hit?.tagName.toLowerCase()}.${hit?.className}`);
    }
    return missed;
  });
  expect(edge).toEqual([]);

  await page.goto(`/p/${pool[1].hash}`);
  await expect(page.locator(".comment")).toHaveCount(2);
  const replies = await clearances(page, ".comment-inner > .comment-foot");
  expect(replies.map((c) => c.above.split(" ")[0]).sort()).toEqual(["link-card", "media-grid"]);
  for (const { above, clearance } of replies) expect(clearance, `a reply's bands under its ${above}`).toBeGreaterThanOrEqual(1.5);
  await context.close();
});

test("with a mouse, the rows of counts and buttons are where they were", async ({ browser }, testInfo) => {
  const { context, page } = await at(browser, testInfo, false);
  await serve(page);
  await page.goto("/");
  const foot = page.locator(`#app .cast-card[data-hash="${pool[0].hash}"] > .cast-foot`);
  await expect(foot).toBeVisible();
  expect(await foot.evaluate((el) => getComputedStyle(el).marginTop)).toBe("9px");
  await page.goto(`/p/${pool[1].hash}`);
  await expect(page.locator(".comment")).toHaveCount(2);
  expect(await page.locator(".comment-inner > .comment-foot").first().evaluate((el) => getComputedStyle(el).marginTop)).toBe("7px");
  await context.close();
});
