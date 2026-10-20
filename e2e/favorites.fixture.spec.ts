// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { signIn } from "./helpers/farcaster";

/*
 * Favorites: the star on a profile makes the account one of the reader's
 * favorites, kept in this browser. Its casts rise in the reader's hot and
 * top feeds, on the front page and in channels, and carry a small star
 * there; new stays in time order. Nothing is sent anywhere.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const person = (fid: number, username: string) => ({ fid, username, display_name: username[0].toUpperCase() + username.slice(1), follower_count: 10, following_count: 5, bio: "" });
const ann = person(1, "ann");
const bob = person(2, "bob");
const cat = person(3, "cat");
const me = { fid: 1234, username: "reader", display_name: "Reader" };
// A stranger's busy cast, bob's quieter one, and a stranger's newest.
const busy = { hash: hex(1), author: ann, text: "Busy", ts, likes: 30, replies: 0 };
const quiet = { hash: hex(2), author: bob, text: "Quiet", ts, likes: 10, replies: 0 };
const newest = { hash: hex(3), author: cat, text: "Newest", ts: ts + 3600, likes: 1, replies: 0 };
const pool = [busy, quiet, newest];

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : pool, notes: {}, meta } });
  });
  await page.route("**/api/v1/channel?*", (route) =>
    route.fulfill({ json: { channel: { id: "books", name: "Books", parent_url: "https://farcaster.xyz/~/channel/books", follower_count: 10 }, pool, notes: {}, meta } }),
  );
  await page.route("**/api/v1/user?*", (route) => {
    const name = new URL(route.request().url()).searchParams.get("name");
    const user = [ann, bob, cat].find((u) => u.username === name) ?? { ...me, follower_count: 1, following_count: 1, bio: "" };
    return route.fulfill({ json: { user, pool: pool.filter((c) => c.author.fid === user.fid), notes: {}, meta } });
  });
}

const order = (page: Page) =>
  page.locator(".cast-list .cast-card:not(.skeleton)").evaluateAll((cards) => cards.map((card) => card.querySelector(".cast-text")?.textContent?.trim()));

async function feed(page: Page, path: string, expected: string[]) {
  await page.goto(path);
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  await expect.poll(() => order(page)).toEqual(expected);
}

test("a favorite's casts rise in hot and top, on the front page and in channels, with a star; new stays in time order", async ({ page }) => {
  await serve(page);
  await feed(page, "/?sort=top", ["Busy", "Quiet", "Newest"]);
  await expect(page.getByRole("img", { name: "One of your favorites" })).toHaveCount(0);

  // On bob's profile, the star makes bob a favorite.
  await page.goto("/u/bob");
  const star = page.getByRole("button", { name: "Favorite" });
  await expect(star).toHaveAttribute("aria-pressed", "false");
  await expect(star).toHaveAccessibleDescription("Their casts rise in your hot and top feeds. Kept in this browser only.");
  await star.click();
  await expect(star).toHaveAttribute("aria-pressed", "true");
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem("allthing.favorites") ?? "[]"))).toEqual([{ fid: 2, username: "bob" }]);
  // Bob's own casts here, under the star that's on, don't each carry one.
  await expect(page.locator(".cast-card", { hasText: "Quiet" })).toBeVisible();
  await expect(page.getByRole("img", { name: "One of your favorites" })).toHaveCount(0);

  // Bob's 10 points weigh as 40: over ann's 30, in top and in hot.
  await feed(page, "/?sort=top", ["Quiet", "Busy", "Newest"]);
  const marked = page.getByRole("img", { name: "One of your favorites" });
  await expect(marked).toHaveCount(1);
  await expect(page.locator(".cast-card", { hasText: "Quiet" }).getByRole("img", { name: "One of your favorites" })).toBeVisible();
  await feed(page, "/?sort=hot", ["Quiet", "Busy", "Newest"]);
  await feed(page, "/c/books?sort=new", ["Newest", "Busy", "Quiet"]);
  await feed(page, "/c/books?sort=top", ["Quiet", "Busy", "Newest"]);

  // Undone, everything is as it was.
  await page.goto("/u/bob");
  await page.getByRole("button", { name: "Favorite" }).click();
  await expect(page.getByRole("button", { name: "Favorite" })).toHaveAttribute("aria-pressed", "false");
  await feed(page, "/?sort=top", ["Busy", "Quiet", "Newest"]);
  await expect(page.getByRole("img", { name: "One of your favorites" })).toHaveCount(0);
});

test("favorites are kept in this browser, across reloads, and another tab follows", async ({ page, context }) => {
  await serve(page);
  await feed(page, "/?sort=top", ["Busy", "Quiet", "Newest"]);
  const other = await context.newPage();
  await serve(other);
  await other.goto("/u/bob");
  await other.getByRole("button", { name: "Favorite" }).click();
  // The first tab's feed re-sorts in place.
  await expect.poll(() => order(page)).toEqual(["Quiet", "Busy", "Newest"]);
  await other.reload();
  await expect(other.getByRole("button", { name: "Favorite" })).toHaveAttribute("aria-pressed", "true");
});

test("there's no star on the reader's own profile", async ({ page }) => {
  await signIn(page, me);
  await serve(page);
  await page.goto("/u/reader");
  await expect(page.getByRole("heading", { level: 1, name: "Reader" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Favorite" })).toHaveCount(0);
  await page.goto("/u/bob");
  await expect(page.getByRole("button", { name: "Favorite" })).toBeVisible();
});

test("at 390, the star sits under the profile's picture and name, inside the screen", async ({ browser }, testInfo) => {
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  const page = await context.newPage();
  await serve(page);
  await page.goto("/u/bob");
  const star = page.getByRole("button", { name: "Favorite" });
  await star.tap();
  await expect(star).toHaveAttribute("aria-pressed", "true");
  const box = (await star.boundingBox())!;
  expect(box.x + box.width).toBeLessThanOrEqual(390);
  expect(box.height).toBeGreaterThanOrEqual(30);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await context.close();
});
