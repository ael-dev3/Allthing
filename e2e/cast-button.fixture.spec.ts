// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";
import { appCame } from "./helpers/app";
import { signIn } from "./helpers/farcaster";

/*
 * Cast stays in reach as the page scrolls, and covers nothing. On a
 * computer it's the site header's, before the account's picture, on the
 * pages that have Cast (the front page's feed, a channel's, the reader's
 * own profile): the header is sticky, so it's always in view, and the page
 * has no other. On the other pages its place is kept, unseen, so the header
 * doesn't move. On a phone it's the round button at the bottom right, 48 px,
 * on the same pages; the page leaves room for it after its end, so, scrolled
 * to the end, the last card's like, recast and reply and the footer's links
 * are clear of it and take a tap. It steps out of the way of a dialog, a
 * popover and the keyboard.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no Cast");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = Math.floor(Date.now() / 1000) - 3_600;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const people = [
  { fid: 42, username: "alice", display_name: "Alice" },
  { fid: 7, username: "bob", display_name: "Bob" },
  { fid: 9, username: "carol", display_name: "Carol" },
];
const words = "Reading on a phone should never mean a button sits on the words you want, or on the heart you mean to press. ";
const casts = Array.from({ length: 12 }, (_, i) => ({
  hash: hex(i + 1),
  author: people[i % 3],
  text: `${i + 1}. ${words.repeat(1 + (i % 3))}`,
  ts: ts - i * 300,
  likes: 10 + i,
  recasts: 2 + i,
  replies: 1 + (i % 4),
}));
const mine = casts.map((cast, i) => ({ ...cast, hash: hex(100 + i), author: me }));
const books = { id: "books", name: "Books", description: "Reading, together.", follower_count: 1200, parent_url: "https://farcaster.xyz/~/channel/books" };
const cors = { "access-control-allow-origin": "*" };

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : casts, notes: {}, meta } });
  });
  await page.route("**/api/v1/channel?*", (route) => route.fulfill({ json: { channel: books, pool: casts, notes: {}, meta } }));
  await page.route("**/api/v1/user?*", (route) => {
    const name = new URL(route.request().url()).searchParams.get("name");
    const who = name === me.username ? me : people[0];
    const pool = who === me ? mine : casts.filter((c) => c.author === who);
    return route.fulfill({ json: { user: { ...who, bio: "Reads everything.", follower_count: 12, following_count: 3 }, pool, notes: {}, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: { thread: { cast: casts[0], children: [] }, notes: {}, meta } }));
  await page.route("**/api/v1/reactions?*", (route) => {
    const url = new URL(route.request().url());
    return route.fulfill({ json: { hash: url.searchParams.get("hash"), kind: url.searchParams.get("type"), users: people, more: false, meta } });
  });
  await page.route("**/api/v1/signer", (route) => route.fulfill({ status: 404, json: { error: "Liking and recasting from Allthing isn't set up on this server" } }));
  await page.route("https://haatz.quilibrium.com/v1/**", (route) =>
    new URL(route.request().url()).pathname.endsWith("/reactionsByFid")
      ? route.fulfill({ headers: cors, json: { messages: [] } })
      : route.fulfill({ status: 400, headers: cors, json: { error: "Failed", error_detail: 'status: NotFound, message: "not found"' } }),
  );
}

/** A page `width` wide (a phone's, with touch, at 390), signed in as the reader. */
async function open(browser: Browser, testInfo: TestInfo, width: number) {
  const phone = width < 641;
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL,
    viewport: { width, height: phone ? 844 : 900 },
    isMobile: phone,
    hasTouch: phone,
  });
  const page = await context.newPage();
  await signIn(page, me);
  await serve(page);
  return { context, page };
}

async function visit(page: Page, path: string) {
  await page.goto(path);
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  await appCame(page);
}

const cast = (page: Page) => page.getByRole("button", { name: "Write a cast" });
const box = async (target: Locator) => (await target.boundingBox())!;

/** Down the page, `by` px, as a reader scrolls. */
async function scroll(page: Page, by: number) {
  await page.mouse.wheel(0, by);
  await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThan(by / 2);
}

/** Scrolled to the very end, once the feed has stopped growing. */
async function toTheEnd(page: Page) {
  await expect(async () => {
    await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
    await page.waitForTimeout(300);
    expect(await page.locator(".cast-card.skeleton").count()).toBe(0);
    expect(await page.evaluate(() => document.documentElement.scrollHeight - window.innerHeight - window.scrollY)).toBeLessThan(1);
  }).toPass();
}

/** Whether a tap at the middle of `target` lands on it. */
const lands = (target: Locator) =>
  target.evaluate((el) => {
    const b = el.getBoundingClientRect();
    const hit = document.elementFromPoint(b.x + b.width / 2, b.y + b.height / 2);
    return hit === el || el.contains(hit);
  });

/** Whether the round Cast is out of the way: unseen, and not what a tap at its middle lands on. */
const away = (fab: Locator) =>
  fab.evaluate((el) => {
    const b = el.getBoundingClientRect();
    const hit = document.elementFromPoint(b.x + b.width / 2, b.y + b.height / 2);
    return getComputedStyle(el).opacity === "0" && hit !== el && !el.contains(hit);
  });

test("at 1280, Cast is the sticky header's on the feeds and the reader's own profile, in view all the way down, and the page's only one", async ({ browser }, testInfo) => {
  const { context, page } = await open(browser, testInfo, 1280);
  const header = page.locator(".site-header");
  for (const [name, path] of [["home", "/"], ["channel", "/c/books"], ["own profile", "/u/reader"]]) {
    await visit(page, path);
    await expect(cast(page), name).toHaveCount(1);
    const pill = header.getByRole("button", { name: "Write a cast" });
    await expect(pill, name).toBeVisible();
    await expect(pill).toHaveText("Cast");
    // Before the account's picture, with only the notifications' bell
    // between them (e2e/notifications.fixture.spec.ts), level with both.
    const [p, b, a] = await Promise.all([box(pill), box(header.locator(".bell")), box(header.locator(".account-button"))]);
    expect(p.x + p.width, name).toBeLessThanOrEqual(b.x);
    expect(b.x - (p.x + p.width), name).toBeLessThan(24);
    expect(b.x + b.width, name).toBeLessThanOrEqual(a.x);
    expect(a.x - (b.x + b.width), name).toBeLessThan(24);
    expect(Math.abs(p.y + p.height / 2 - (a.y + a.height / 2)), name).toBeLessThan(2);
    expect(Math.abs(b.y + b.height / 2 - (a.y + a.height / 2)), name).toBeLessThan(2);
    // Well down the page, it's still in view, where it was.
    await scroll(page, 1_800);
    await expect(pill, name).toBeInViewport({ ratio: 1 });
    const later = await box(pill);
    expect(Math.abs(later.y - p.y), name).toBeLessThan(1);
    expect(Math.abs(later.x - p.x), name).toBeLessThan(1);
    if (name === "home") await page.screenshot({ path: testInfo.outputPath("cast-button-1280-scrolled.png") });
  }
  // A channel's starts a cast in the channel.
  await visit(page, "/c/books");
  await scroll(page, 1_200);
  await header.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText("/books");
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(header.getByRole("button", { name: "Write a cast" })).toBeFocused();
  await context.close();
});

test("at 1280, a page without Cast keeps its place unseen: the header doesn't move or grow from page to page", async ({ browser }, testInfo) => {
  const { context, page } = await open(browser, testInfo, 1280);
  const header = page.locator(".site-header");
  const layout = async () => {
    const [h, search, account] = await Promise.all([box(header), box(header.locator(".search-box")), box(header.locator(".account-button"))]);
    return { height: h.height, search: [search.x, search.width], account: account.x };
  };
  await visit(page, "/");
  const home = await layout();
  for (const path of [`/p/${casts[0].hash}`, "/u/alice"]) {
    await page.goto(path);
    await expect(page.locator(".post, section.profile").first()).toBeVisible();
    await expect(cast(page), path).toHaveCount(0);
    await expect(header.locator(".cast-pill")).toBeHidden();
    expect(await layout(), path).toEqual(home);
  }
  // As tall as the search box: the header is as tall with it as without.
  const [pill, search] = await Promise.all([box(page.locator(".header-cast")), box(header.locator(".search-box"))]);
  expect(pill.height).toBeLessThanOrEqual(search.height + 0.5);
  await context.close();
});

test("at 1280, nothing moves as the front page loads and the app takes over", async ({ browser }, testInfo) => {
  const { context, page } = await open(browser, testInfo, 1280);
  await page.addInitScript(() => {
    (window as unknown as { shifts: number }).shifts = 0;
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries() as (PerformanceEntry & { value: number; hadRecentInput: boolean })[]) {
        if (!entry.hadRecentInput) (window as unknown as { shifts: number }).shifts += entry.value;
      }
    }).observe({ type: "layout-shift", buffered: true });
  });
  await visit(page, "/");
  await expect(cast(page)).toBeVisible();
  await page.waitForTimeout(500);
  expect(await page.evaluate(() => (window as unknown as { shifts: number }).shifts)).toBeLessThan(0.01);
  await context.close();
});

test("at 390, the round Cast is 48 px at the bottom right, and the page leaves room for it: at the end, the last card's like, recast and reply and the footer's links are clear of it", async ({ browser }, testInfo) => {
  const { context, page } = await open(browser, testInfo, 390);
  for (const path of ["/", "/c/books", "/u/reader"]) {
    await visit(page, path);
    // The page's only Cast: the header has none on a phone, nor the profile.
    await expect(cast(page), path).toHaveCount(1);
    const fab = page.locator(".cast-fab");
    await expect(fab, path).toBeVisible();
    const f = await box(fab);
    expect([Math.round(f.width), Math.round(f.height)]).toEqual([48, 48]);
    expect(f.x + f.width).toBeLessThanOrEqual(390 - 8);
    expect(f.y + f.height).toBeLessThanOrEqual(844 - 8);
    expect(await fab.evaluate((el) => getComputedStyle(el).position)).toBe("fixed");

    // Scrolled to the end, as far as it goes.
    await toTheEnd(page);
    const last = page.locator(".cast-list > .cast-card:not(.skeleton)").last();
    const controls = {
      like: last.locator(".stat-likes").getByRole("button", { name: "Like", exact: true }),
      recast: last.locator(".stat-recasts").getByRole("button", { name: "Recast", exact: true }),
      reply: last.locator(".stat-replies"),
    };
    for (const [name, control] of Object.entries(controls)) {
      await expect(control, `${path} ${name}`).toBeInViewport({ ratio: 1 });
      expect(await lands(control), `${path}: the last card's ${name}`).toBe(true);
    }
    // The footer's links, and its whole line, end above the round Cast.
    await toTheEnd(page);
    const footer = page.locator(".site-footer");
    for (const link of await footer.locator("a, .version").all()) {
      await expect(link).toBeInViewport({ ratio: 1 });
      expect(await lands(link), `${path}: ${await link.textContent()}`).toBe(true);
      const l = await box(link);
      expect(l.y + l.height, `${path}: ${await link.textContent()}`).toBeLessThanOrEqual(f.y);
    }
    const line = await footer.evaluate((el) => {
      const b = el.querySelector("a")!.getBoundingClientRect();
      return document.elementFromPoint(380, b.y + b.height / 2)?.closest(".cast-fab") === null;
    });
    expect(line, `${path}: the footer's line at the right edge`).toBe(true);
    if (path === "/") await page.screenshot({ path: testInfo.outputPath("cast-button-390-end.png") });
  }
  await context.close();
});

test("at 390, the round Cast steps out of the way of a dialog, a popover and the keyboard", async ({ browser }, testInfo) => {
  const { context, page } = await open(browser, testInfo, 390);
  await visit(page, "/");
  const fab = page.locator(".cast-fab");
  await expect.poll(() => away(fab)).toBe(false);
  // The composer it opens.
  await fab.tap();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await expect(dialog).toBeVisible();
  await expect.poll(() => away(fab)).toBe(true);
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(fab).toBeFocused();
  await expect.poll(() => away(fab)).toBe(false);
  // A popover: who liked a card.
  const likes = page.locator(".cast-card:not(.skeleton)").first().locator(".stat-likes .popover-trigger");
  await likes.tap();
  await expect(page.locator(".popover-panel")).toBeVisible();
  await expect.poll(() => away(fab)).toBe(true);
  await page.keyboard.press("Escape");
  await expect(page.locator(".popover-panel")).toHaveCount(0);
  await expect.poll(() => away(fab)).toBe(false);
  // The keyboard, up for the search box.
  const search = page.locator(".site-header .search-box input");
  await search.tap();
  await expect(search).toBeFocused();
  await expect.poll(() => away(fab)).toBe(true);
  await search.blur();
  await expect.poll(() => away(fab)).toBe(false);
  await context.close();
});

test("at 390, a page without Cast has no round button and no room kept for one", async ({ browser }, testInfo) => {
  const { context, page } = await open(browser, testInfo, 390);
  const room = () => page.locator(".site-footer").evaluate((el) => parseFloat(getComputedStyle(el).paddingBottom));
  await visit(page, "/");
  const withCast = await room();
  for (const path of [`/p/${casts[0].hash}`, "/u/alice"]) {
    await page.goto(path);
    await expect(page.locator(".post, section.profile").first()).toBeVisible();
    await expect(cast(page), path).toHaveCount(0);
    await expect(page.locator(".cast-fab")).toHaveCount(0);
    expect(await room(), path).toBeLessThan(withCast - 40);
  }
  await context.close();
});
