// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";
import { signIn } from "./helpers/farcaster";
import { NOW } from "./fixture-time";

/*
 * A signed-in reader's Following tab ranks only the casts of whom they
 * follow (`/api/v1/following`, a stand-in here answering as the Worker
 * would), with their Show less and favorites, and no trending casts topping
 * it up; For you shows the trending pool. Signed out, the front page is the
 * trending pool as before, with no tabs, and never asks for the following
 * feed. At 1280 and at 390 on a touch screen. The endpoint's own refusal
 * without a session asks the real Worker. The tabs themselves:
 * feed-tabs.fixture.spec.ts.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = NOW - 600;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const person = (fid: number, username: string) =>
  ({ fid, username, display_name: username[0].toUpperCase() + username.slice(1), follower_count: 10, following_count: 5, bio: "" });
const ann = person(1, "ann");
const bob = person(2, "bob");
const stranger = person(9, "stranger");
const me = { fid: 1234, username: "reader", display_name: "Reader" };

// Two casts by accounts the reader follows, and the stranger's busier one.
const followedBusy = { hash: hex(1), author: ann, text: "Followed busy", ts, likes: 30, replies: 0 };
const followedQuiet = { hash: hex(2), author: bob, text: "Followed quiet", ts, likes: 10, replies: 0 };
const trending = { hash: hex(9), author: stranger, text: "Stranger trending", ts, likes: 500, replies: 0 };
const PRIVATE = { "cache-control": "private, no-store" };

type Asked = { following: number; home: number };

async function serve(page: Page, following: object = { pool: [followedQuiet, followedBusy], notes: {}, meta }) {
  const asked: Asked = { following: 0, home: 0 };
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    asked.home += 1;
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [trending, followedBusy], notes: {}, meta } });
  });
  await page.route("**/api/v1/following**", (route) => {
    asked.following += 1;
    return route.fulfill({ json: following, headers: PRIVATE });
  });
  await page.route("**/api/v1/notifications**", (route) => route.fulfill({ json: { groups: [], meta }, headers: PRIVATE }));
  await page.route("https://haatz.quilibrium.com/**", (route) =>
    route.fulfill({ headers: { "access-control-allow-origin": "*" }, json: { messages: [] } }),
  );
  return asked;
}

const order = (page: Page) =>
  page.locator(".cast-list .cast-card:not(.skeleton)").evaluateAll((cards) => cards.map((card) => card.querySelector(".cast-text")?.textContent?.trim()));

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true,
  });
  return { context, page: await context.newPage() };
}

async function signedInSeesOnlyFollowed(page: Page) {
  await signIn(page, me);
  const asked = await serve(page);
  await page.goto("/?sort=top&feed=following");
  await expect.poll(() => order(page)).toEqual(["Followed busy", "Followed quiet"]);
  await expect(page.getByText("Stranger trending")).toHaveCount(0);
  await expect(page.locator(".following-note")).toHaveText("Casts from people you follow.");
  const tabs = page.getByRole("navigation", { name: "Feed" });
  await expect(tabs.getByRole("link", { name: "Following" })).toHaveAttribute("aria-current", "page");
  // Following keeps the time range too.
  await expect(page.getByRole("combobox", { name: "Time range" })).toBeVisible();
  expect(asked.following).toBeGreaterThan(0);
  expect(asked.home).toBe(0);
  // Nothing beside the feed reaches past the page's width.
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);

  // For you: the trending pool, with the sort kept.
  await tabs.getByRole("link", { name: "For you" }).click();
  await expect(page).not.toHaveURL(/feed=/);
  await expect(page).toHaveURL(/[?&]sort=top/);
  await expect.poll(() => order(page)).toEqual(["Stranger trending", "Followed busy"]);
  // And back, the sort kept.
  await tabs.getByRole("link", { name: "Following" }).click();
  await expect(page).toHaveURL(/[?&]feed=following/);
  await expect(page).toHaveURL(/[?&]sort=top/);
  await expect.poll(() => order(page)).toEqual(["Followed busy", "Followed quiet"]);
}

test("a signed-in reader's Following tab ranks only whom they follow, at 1280", async ({ page }) => {
  await signedInSeesOnlyFollowed(page);
});

test("a signed-in reader's Following tab ranks only whom they follow, at 390", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await signedInSeesOnlyFollowed(page);
  await context.close();
});

test("Show less demotes a followed account, and a favorite rises", async ({ page }) => {
  await signIn(page, me);
  await page.addInitScript(() => localStorage.setItem("allthing.show-less", JSON.stringify([{ fid: 1, username: "ann" }])));
  await serve(page);
  await page.goto("/?sort=top&feed=following");
  await expect.poll(() => order(page)).toEqual(["Followed quiet", "Followed busy"]);
  await page.goto("/?feed=following");
  await expect.poll(() => order(page)).toEqual(["Followed quiet", "Followed busy"]);
});

test("a favorite among whom they follow rises in their feed", async ({ page }) => {
  await signIn(page, me);
  await page.addInitScript(() => localStorage.setItem("allthing.favorites", JSON.stringify([{ fid: 2, username: "bob" }])));
  await serve(page);
  await page.goto("/?sort=top&feed=following");
  await expect.poll(() => order(page)).toEqual(["Followed quiet", "Followed busy"]);
  await expect(page.getByRole("img", { name: "One of your favorites" })).toHaveCount(1);
});

test("following few who cast lately, Following shows only theirs: no trending casts top it up", async ({ page }) => {
  await signIn(page, me);
  // A payload from an older Worker that still tops up: its extras are not shown.
  await serve(page, { pool: [followedQuiet], topped_up: [trending], notes: {}, meta });
  await page.goto("/?sort=top&feed=following");
  await expect.poll(() => order(page)).toEqual(["Followed quiet"]);
  await expect(page.locator(".following-note")).toHaveText("Casts from people you follow.");
});

test("Following's new is theirs, newest first", async ({ page }) => {
  await signIn(page, me);
  const older = { ...followedBusy, ts: ts - 3600 };
  await serve(page, { pool: [older, followedQuiet], notes: {}, meta });
  await page.goto("/?sort=new&feed=following");
  await expect.poll(() => order(page)).toEqual(["Followed quiet", "Followed busy"]);
});

test("signed out, the front page is the trending pool, with no tabs, at 1280 and 390", async ({ page, browser }, testInfo) => {
  const small = await phone(browser, testInfo);
  for (const which of [page, small.page]) {
    const asked = await serve(which);
    // An old link to the Following tab is For you signed out.
    await which.goto("/?sort=top&feed=following");
    await expect.poll(() => order(which)).toEqual(["Stranger trending", "Followed busy"]);
    await expect(which.getByRole("navigation", { name: "Feed" })).toHaveCount(0);
    await expect(which.getByRole("combobox", { name: "Time range" })).toBeVisible();
    await expect(which.locator(".following-note")).toHaveCount(0);
    expect(asked.following).toBe(0);
  }
  await small.context.close();
});

test("the following endpoint answers only a session", async ({ request }) => {
  for (const path of ["/api/v1/following", "/api/v1/following?fid=3", "/api/v1/following?cursor=183148482"]) {
    const plain = await request.get(path);
    expect(plain.status(), path).toBe(401);
    expect(plain.headers()["cache-control"], path).toBe("no-store");
  }
  // The user cookie only labels the header; it signs no one in.
  const labelled = await request.get("/api/v1/following", {
    headers: { cookie: `__Host-allthing-user=${encodeURIComponent(JSON.stringify({ fid: 3, username: "dwr" }))}` },
  });
  expect(labelled.status()).toBe(401);
});
