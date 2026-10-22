// SPDX-License-Identifier: AGPL-3.0-or-later
import axe from "axe-core";
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";

/*
 * An account's followers and whom it follows (`/u/<name>/followers`,
 * `/u/<name>/following`). The profile's counts link to them, and tabs
 * switch between them. The people come the newest follow first, a page at
 * a time as the reader scrolls (`/api/v1/follows`), into a list as tall as
 * all of them, of which only the rows in view and a few either side are in
 * the page: a thousand people are a few dozen rows. Nothing moves as they
 * come, Back from a person's profile puts the list back where it was, and
 * Back from the list goes to the profile.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const now = Math.floor(Date.now() / 1000);
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const bob = { fid: 43, username: "bob", display_name: "Bob" };
const PAGE = 100;
type Person = { fid: number; username: string; display_name: string; bio?: string; followers?: number; following?: number; followed_at: number };
/** The `n`th of a list, newest follow first. */
const person = (list: "fan" | "pal", n: number): Person => ({
  fid: (list === "fan" ? 10_000 : 20_000) + n,
  username: `${list}${n}`,
  display_name: `${list === "fan" ? "Fan" : "Pal"} ${n}`,
  bio: n % 3 === 0 ? "" : `Bio of ${list} ${n}: reads, writes, and follows along for the long conversations.`,
  followers: (n * 977) % 25_000,
  following: 12,
  followed_at: now - n * 600,
});
const followers = Array.from({ length: 1000 }, (_, n) => person("fan", n));
const following = Array.from({ length: 250 }, (_, n) => person("pal", n));
const owner = { ...alice, followers: 1234, following: 567 };

type Served = { follows: string[] };

/** The Worker's API, standing in: a page of 100 people a request; `trouble` answers a later page's first asks with `status`. */
async function serve(page: Page, trouble?: { status: number; times: number }) {
  const served: Served = { follows: [] };
  let troubles = trouble?.times ?? 0;
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/user?*", (route) => {
    const name = new URL(route.request().url()).searchParams.get("name");
    const user = name === "bob" ? { ...bob, follower_count: 0, following_count: 0 } : { ...alice, follower_count: owner.followers, following_count: owner.following, bio: "Reads." };
    return route.fulfill({ json: { user, pool: [], notes: {}, meta } });
  });
  await page.route("**/api/v1/vote?*", (route) =>
    route.fulfill({ json: { fid: Number(new URL(route.request().url()).searchParams.get("fid")), onchain: "stay", app: "opted_in", meta } }));
  await page.route("**/api/v1/wallets?*", (route) =>
    route.fulfill({ json: { fid: Number(new URL(route.request().url()).searchParams.get("fid")), addresses: [], chains: [], total: 0, sources: [], fetched_at: Date.now() } }));
  await page.route("**/api/v1/follows?*", (route) => {
    const url = new URL(route.request().url());
    served.follows.push(url.search);
    const name = url.searchParams.get("name");
    const dir = url.searchParams.get("dir");
    const cursor = url.searchParams.get("cursor");
    const at = cursor ? Number(cursor.slice(1)) : 0;
    if (cursor && troubles > 0) {
      troubles -= 1;
      return route.fulfill({ status: trouble!.status, json: { error: "Farcaster data source is busy; backing off", retry_after: 1 } });
    }
    const empty = name === "bob" || name === "fid:43";
    const list = empty ? [] : dir === "following" ? following : followers;
    const people = list.slice(at, at + PAGE);
    const next = at + PAGE < list.length ? `p${at + PAGE}` : undefined;
    const first = cursor ? {} : { owner: empty ? { ...bob, followers: 0, following: 0 } : owner };
    return route.fulfill({ json: { dir, ...first, people, ...(next ? { next } : {}), meta } });
  });
  return served;
}

async function at(browser: Browser, testInfo: TestInfo, phone: boolean) {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL,
    viewport: phone ? { width: 390, height: 844 } : { width: 1280, height: 900 },
    isMobile: phone,
    hasTouch: phone,
  });
  const page = await context.newPage();
  // Layout shifts not caused by the reader's input, from the first frame.
  await page.addInitScript(() => {
    (window as unknown as { __shift: number }).__shift = 0;
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries() as (PerformanceEntry & { value: number; hadRecentInput: boolean })[]) {
        // The page's foot, which a test that jumps to the bottom has in view
        // as the next page comes, moves down as in any list that reads on.
        const sources = (entry as unknown as { sources?: { node?: Node | null }[] }).sources ?? [];
        if (sources.length && sources.every((s) => (s.node as Element | null)?.closest?.(".site-footer"))) continue;
        if (!entry.hadRecentInput) (window as unknown as { __shift: number }).__shift += entry.value;
      }
    }).observe({ type: "layout-shift", buffered: true });
  });
  return { context, page };
}

const rows = (page: Page) => page.locator(".people-list:not(.waiting) > .person-row");
/** Once the page has come in: nothing moving but what loops (the placeholders' sheen). */
const settled = (page: Page) =>
  page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== "running" || a.effect?.getComputedTiming().endTime === Infinity));
const shift = (page: Page) => page.evaluate(() => (window as unknown as { __shift: number }).__shift);
/** How tall the list is, in rows. */
const listRows = (page: Page) =>
  page.locator(".people-list:not(.waiting)").evaluate((list) => {
    const row = list.querySelector(".person-row")!.getBoundingClientRect().height;
    return Math.round(list.getBoundingClientRect().height / row);
  });

for (const phone of [false, true]) {
  const width = phone ? 390 : 1280;

  test(`at ${width}, a profile's counts open its lists, the tabs switch between them, and Back goes back`, async ({ browser }, testInfo) => {
    const { context, page } = await at(browser, testInfo, phone);
    await serve(page);
    await page.goto("/u/alice");
    const counts = page.locator(".profile .counts");
    await expect(counts.getByRole("link", { name: "1,234 followers" })).toHaveAttribute("href", "/u/alice/followers");
    await expect(counts.getByRole("link", { name: "567 following" })).toHaveAttribute("href", "/u/alice/following");
    await counts.getByRole("link", { name: "1,234 followers" }).click();
    await expect(page).toHaveURL(/\/u\/alice\/followers$/);
    await expect(page).toHaveTitle("People following Alice (@alice) · Allthing");
    const tabs = page.getByRole("navigation", { name: "Followers and following" });
    await expect(tabs.getByRole("link", { name: "Followers 1,234" })).toHaveAttribute("aria-current", "page");
    await expect(rows(page).first()).toContainText("Fan 0");
    await tabs.getByRole("link", { name: "Following 567" }).click();
    await expect(page).toHaveURL(/\/u\/alice\/following$/);
    await expect(tabs.getByRole("link", { name: "Following 567" })).toHaveAttribute("aria-current", "page");
    await expect(rows(page).first()).toContainText("Pal 0");
    // The way back to the profile, and Back.
    await expect(page.getByRole("link", { name: /^Back to Alice/ })).toHaveAttribute("href", "/u/alice");
    await page.goBack();
    await expect(page).toHaveURL(/\/u\/alice\/followers$/);
    await expect(rows(page).first()).toContainText("Fan 0");
    await page.goBack();
    await expect(page).toHaveURL(/\/u\/alice$/);
    await expect(page.locator(".profile h1")).toHaveText("Alice");
    await context.close();
  });

  test(`at ${width}, the people come newest follow first, a window of rows at a time, and scrolling reads on to the end`, async ({ browser }, testInfo) => {
    const { context, page } = await at(browser, testInfo, phone);
    const served = await serve(page);
    await page.goto("/u/alice/followers");
    await expect(rows(page).first()).toContainText("Fan 0");
    // Each row: picture, name and handle, the start of the bio, followers.
    const second = rows(page).nth(1);
    await expect(second.locator(".person-row-name")).toHaveText("Fan 1");
    await expect(second.locator(".person-row-handle")).toHaveText("@fan1");
    await expect(second.locator(".person-row-bio")).toHaveText(followers[1].bio!);
    await expect(second.locator(".person-row-stat")).toHaveText("977 followers");
    await expect(rows(page).nth(3).locator(".person-row-stat")).toHaveText("2,931 followers");
    expect(await rows(page).evaluateAll((all) => all.slice(0, 4).map((row) => [row.getAttribute("aria-posinset"), row.getAttribute("aria-setsize")]))).toEqual([
      ["1", "1234"], ["2", "1234"], ["3", "1234"], ["4", "1234"],
    ]);
    // Every row as tall as the others, and the list as tall as all of them.
    const height = await rows(page).first().evaluate((row) => row.getBoundingClientRect().height);
    expect(height).toBeCloseTo(phone ? 72 : 64, 1);
    expect(await listRows(page)).toBe(100);
    // Scrolling reads on, a page at a time, until all 1,000 are in; no
    // more than 60 rows are ever in the page.
    let most = 0;
    for (let i = 0; i < 40 && (await listRows(page)) < 1000; i++) {
      await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
      await page.waitForTimeout(150);
      most = Math.max(most, await rows(page).count());
    }
    expect(await listRows(page)).toBe(1000);
    expect(served.follows.filter((q) => q.includes("cursor="))).toHaveLength(9);
    // Later pages name the account by fid, from the cursor.
    expect(served.follows[1]).toBe("?name=fid%3A42&dir=followers&cursor=p100");
    // The last page came in below the view: on down to its end.
    await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
    await expect(rows(page).last()).toContainText("Fan 999");
    most = Math.max(most, await rows(page).count());
    expect(most).toBeLessThanOrEqual(60);
    // All in: the list holds as many as it has.
    await expect(rows(page).last()).toHaveAttribute("aria-setsize", "1000");
    await expect(rows(page).last()).toHaveAttribute("aria-posinset", "1000");
    // Nothing moved as they came.
    expect(await shift(page)).toBeLessThan(0.01);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    await page.screenshot({ path: testInfo.outputPath(`follows-${width}-end.png`) });
    await context.close();
  });

  test(`at ${width}, Back from a person's profile puts the list back where it was`, async ({ browser }, testInfo) => {
    const { context, page } = await at(browser, testInfo, phone);
    await serve(page);
    await page.goto("/u/alice/followers");
    await expect(rows(page).first()).toContainText("Fan 0");
    // Three pages in, a little way down the third.
    for (let i = 0; i < 20 && (await listRows(page)) < 300; i++) {
      await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
      await page.waitForTimeout(150);
    }
    const row = phone ? 72 : 64;
    await page.evaluate((y) => window.scrollTo(0, y), 230 * row);
    await page.waitForTimeout(300);
    const target = page.locator('.person-row[aria-posinset="236"]');
    await expect(target).toBeInViewport();
    const before = await target.evaluate((el) => el.getBoundingClientRect().top);
    const scrolled = await page.evaluate(() => window.scrollY);
    // A click on the row, away from its links, opens the profile.
    await target.locator(".person-row-stat").click();
    await expect(page).toHaveURL(/\/u\/fan235$/);
    // Back before the profile's code has come: the list stays, as it was,
    // when it comes.
    await page.goBack();
    await expect(page).toHaveURL(/\/u\/alice\/followers$/);
    await expect(page.locator('.person-row[aria-posinset="236"]')).toBeInViewport();
    await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThan(scrolled - 3);
    await settled(page);
    await page.waitForTimeout(1000);
    expect(Math.abs((await page.evaluate(() => window.scrollY)) - scrolled)).toBeLessThanOrEqual(2);
    expect(Math.abs((await page.locator('.person-row[aria-posinset="236"]').evaluate((el) => el.getBoundingClientRect().top)) - before)).toBeLessThanOrEqual(2);
    await expect(page.locator(".profile")).toHaveCount(0);
    await context.close();
  });

  test(`at ${width}, the list has its place before its people come, and axe finds nothing`, async ({ browser }, testInfo) => {
    const { context, page } = await at(browser, testInfo, phone);
    await serve(page);
    // The first page held back a moment: the header, toolbar and rows waiting.
    let release!: () => void;
    const held = new Promise<void>((resolve) => (release = resolve));
    await page.route("**/api/v1/follows?*", async (route) => {
      await held;
      return route.fallback();
    });
    await page.goto("/u/alice/followers");
    // Rows waiting down to the screen's foot: what comes after the list stays out of view.
    await expect(page.locator(".people-list.waiting .person-row").first()).toBeVisible();
    await settled(page);
    expect(await page.locator(".people-list.waiting").evaluate((list) => list.getBoundingClientRect().bottom)).toBeGreaterThanOrEqual(phone ? 844 : 900);
    const toolbar = await page.locator(".follows-toolbar").boundingBox();
    release();
    await expect(rows(page).first()).toContainText("Fan 0");
    await settled(page);
    expect((await page.locator(".follows-toolbar").boundingBox())!.y).toBe(toolbar!.y);
    expect(await shift(page)).toBeLessThan(0.01);
    await page.screenshot({ path: testInfo.outputPath(`follows-${width}.png`) });
    await page.evaluate(axe.source);
    const violations = await page.evaluate(async () => {
      // @ts-expect-error axe is the injected script's global
      const result = await window.axe.run(".follows-page", { resultTypes: ["violations"] });
      return result.violations.map((v: { id: string; nodes: { target: string[] }[] }) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`);
    });
    expect(violations).toEqual([]);
    await context.close();
  });
}

test("a person's name shows their profile card, from what the row knows", async ({ page }) => {
  await serve(page);
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/u/alice/followers");
  await rows(page).nth(1).locator(".person-row-id").hover();
  const card = page.getByRole("dialog", { name: "Profile: Fan 1 (@fan1)", exact: true });
  await expect(card).toBeVisible();
  await expect(card).toContainText(followers[1].bio!);
  await expect(card).toContainText("977 followers");
  await expect(card).toContainText("12 following");
});

test("a page that fails is tried again, and then the reader can try again, keeping the people shown", async ({ page }) => {
  // Busy once: tried again by itself.
  const served = await serve(page, { status: 503, times: 1 });
  await page.goto("/u/alice/followers");
  await expect(rows(page).first()).toContainText("Fan 0");
  await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
  await expect.poll(() => listRows(page), { timeout: 10_000 }).toBe(200);
  expect(served.follows.filter((q) => q.includes("cursor=p100"))).toHaveLength(2);
});

test("a page that keeps failing offers Try again, and the people stay", async ({ page }) => {
  await serve(page, { status: 404, times: 1 });
  await page.goto("/u/alice/followers");
  await expect(rows(page).first()).toContainText("Fan 0");
  await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
  const again = page.getByRole("button", { name: "Try again" });
  await expect(again).toBeVisible();
  await expect(page.locator(".follows-more")).toContainText("Couldn't load more people.");
  expect(await listRows(page)).toBe(100);
  await again.click();
  await expect.poll(() => listRows(page)).toBe(200);
  await expect(again).toHaveCount(0);
});

test("an account with no one says so", async ({ page }) => {
  await serve(page);
  await page.goto("/u/bob/followers");
  await expect(page.getByText("No followers yet", { exact: true })).toBeVisible();
  await page.goto("/u/bob/following");
  await expect(page.getByText("Not following anyone yet", { exact: true })).toBeVisible();
});
