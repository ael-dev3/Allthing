// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { signIn } from "./helpers/farcaster";

/*
 * Show less: on a profile, the reader sees less of the account, in this
 * browser. Its casts sit a little lower in the reader's hot and top feeds,
 * on the front page and in channels (half their points), with no mark;
 * nothing is hidden, and new stays in time order. An account is a favorite
 * or one the reader sees less of, not both. Nothing is sent anywhere.
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
// Ann's busy cast, bob's middling one, and cat's newest, quietest one.
const busy = { hash: hex(1), author: ann, text: "Busy", ts, likes: 30, replies: 0 };
const middling = { hash: hex(2), author: bob, text: "Middling", ts, likes: 20, replies: 0 };
const newest = { hash: hex(3), author: cat, text: "Newest", ts: ts + 3600, likes: 1, replies: 0 };
const pool = [busy, middling, newest];

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

const kept = (page: Page) =>
  page.evaluate(() => ({
    favorites: JSON.parse(localStorage.getItem("allthing.favorites") ?? "[]"),
    less: JSON.parse(localStorage.getItem("allthing.show-less") ?? "[]"),
  }));

test("show less lowers an account's casts a little in hot and top, with no mark and nothing hidden; new stays in time order", async ({ page }) => {
  await serve(page);
  await feed(page, "/?sort=top", ["Busy", "Middling", "Newest"]);

  await page.goto("/u/ann");
  const less = page.getByRole("button", { name: "Show less" });
  await expect(less).toHaveAttribute("aria-pressed", "false");
  await expect(less).toHaveAccessibleDescription("Their casts sit a little lower in your hot and top feeds. Nothing is hidden. Kept in this browser only.");
  await less.click();
  await expect(less).toHaveAttribute("aria-pressed", "true");
  expect(await kept(page)).toEqual({ favorites: [], less: [{ fid: 1, username: "ann" }] });

  // Ann's 30 points weigh as 15: under bob's 20, still over cat's 1.
  await feed(page, "/?sort=top", ["Middling", "Busy", "Newest"]);
  await expect(page.locator(".cast-card", { hasText: "Busy" })).toBeVisible();
  await expect(page.locator(".cast-card", { hasText: "Busy" }).locator(".favorite-mark")).toHaveCount(0);
  await feed(page, "/?sort=hot", ["Middling", "Busy", "Newest"]);
  await feed(page, "/c/books?sort=top", ["Middling", "Busy", "Newest"]);
  await feed(page, "/c/books?sort=new", ["Newest", "Busy", "Middling"]);

  // Undone, everything is as it was.
  await page.goto("/u/ann");
  await page.getByRole("button", { name: "Show less" }).click();
  await expect(page.getByRole("button", { name: "Show less" })).toHaveAttribute("aria-pressed", "false");
  await feed(page, "/?sort=top", ["Busy", "Middling", "Newest"]);
});

test("an account is a favorite or one you see less of, not both, and another tab follows", async ({ page, context }) => {
  await serve(page);
  await feed(page, "/?sort=top", ["Busy", "Middling", "Newest"]);
  const other = await context.newPage();
  await serve(other);
  await other.goto("/u/ann");
  const favorite = other.getByRole("button", { name: "Favorite" });
  const less = other.getByRole("button", { name: "Show less" });
  await favorite.click();
  await expect(favorite).toHaveAttribute("aria-pressed", "true");
  await less.click();
  await expect(less).toHaveAttribute("aria-pressed", "true");
  await expect(favorite).toHaveAttribute("aria-pressed", "false");
  expect(await kept(other)).toEqual({ favorites: [], less: [{ fid: 1, username: "ann" }] });
  // The first tab's feed re-sorts in place.
  await expect.poll(() => order(page)).toEqual(["Middling", "Busy", "Newest"]);
  await favorite.click();
  await expect(less).toHaveAttribute("aria-pressed", "false");
  expect(await kept(other)).toEqual({ favorites: [{ fid: 1, username: "ann" }], less: [] });
  await expect.poll(() => order(page)).toEqual(["Busy", "Middling", "Newest"]);
  await other.reload();
  await expect(other.getByRole("button", { name: "Favorite" })).toHaveAttribute("aria-pressed", "true");
  await expect(other.getByRole("button", { name: "Show less" })).toHaveAttribute("aria-pressed", "false");
});

test("there's no Show less on the reader's own profile", async ({ page }) => {
  await signIn(page, me);
  await serve(page);
  await page.goto("/u/reader");
  await expect(page.getByRole("heading", { level: 1, name: "Reader" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Show less" })).toHaveCount(0);
  await page.goto("/u/bob");
  await expect(page.getByRole("button", { name: "Show less" })).toBeVisible();
});

for (const width of [390, 360]) {
  test(`at ${width}, Favorite and Show less share a row under the picture and name, inside the screen`, async ({ browser }, testInfo) => {
    const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width, height: 844 }, isMobile: true, hasTouch: true });
    const page = await context.newPage();
    await serve(page);
    await page.goto("/u/ann");
    const favorite = page.getByRole("button", { name: "Favorite" });
    const less = page.getByRole("button", { name: "Show less" });
    await less.tap();
    await expect(less).toHaveAttribute("aria-pressed", "true");
    const [a, b] = [(await favorite.boundingBox())!, (await less.boundingBox())!];
    const picture = (await page.locator(".profile .avatar").boundingBox())!;
    expect(Math.abs(a.y - b.y)).toBeLessThan(2);
    expect(a.y).toBeGreaterThan(picture.y + picture.height);
    expect(a.x).toBeLessThanOrEqual(picture.x + 1);
    expect(b.x + b.width).toBeLessThanOrEqual(width);
    expect(b.height).toBeGreaterThanOrEqual(30);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    await context.close();
  });
}

test("at 1280, Favorite and Show less sit at the profile's top right, level with the name", async ({ page }) => {
  await serve(page);
  await page.goto("/u/ann");
  const heading = page.getByRole("heading", { level: 1, name: "Ann" });
  await expect(heading).toBeVisible();
  await expect(page.getByRole("button", { name: "Show less" })).toBeVisible();
  const name = await heading.boundingBox();
  const favorite = await page.getByRole("button", { name: "Favorite" }).boundingBox();
  const less = await page.getByRole("button", { name: "Show less" }).boundingBox();
  const card = await page.locator("section.profile").boundingBox();
  expect(Math.abs(favorite!.y - less!.y)).toBeLessThan(2);
  expect(favorite!.x).toBeGreaterThan(name!.x + name!.width);
  expect(card!.x + card!.width - (less!.x + less!.width)).toBeLessThan(40);
  expect(favorite!.y).toBeLessThan(name!.y + name!.height);
});

test("the profile card's content can't be moved sideways under its edge", async ({ page }) => {
  await serve(page);
  await page.goto("/u/ann");
  await expect(page.getByRole("button", { name: "Show less" })).toBeVisible();
  // The glow behind the name reaches past the card's sides. When the card
  // could be scrolled, anything that scrolled it moved its avatar and text
  // 40 px to the left, cut off by the card's edge.
  const card = page.locator("section.profile");
  expect(await card.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeGreaterThan(0);
  expect(await card.evaluate((el) => { el.scrollLeft = 40; return el.scrollLeft; })).toBe(0);
});
