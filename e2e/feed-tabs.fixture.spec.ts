// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { signIn } from "./helpers/farcaster";

/*
 * The front page's two tabs for a signed-in reader, as X's: For you, the
 * trending pool with the casts of whom they follow lifted, and Following,
 * only theirs (`?feed=following`). Links with `aria-current`, reached by the
 * keyboard, 44 px tall, keeping the time range and choosing each feed's default sort;
 * an old `?feed=everyone` link opens For you. Whom they follow is kept from
 * their Following page, renewed once in the background when it's missing
 * (followed.rs), and the list on screen isn't re-sorted when it lands. At
 * 1280 and at 390 on a touch screen. Signed out, there are no tabs
 * (following-feed.fixture.spec.ts).
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
// Recent: the front page leaves out casts older than its time range.
const ts = Math.floor(Date.now() / 1000) - 600;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const person = (fid: number, username: string) =>
  ({ fid, username, display_name: username[0].toUpperCase() + username.slice(1), follower_count: 10, following_count: 5, bio: "" });
const ann = person(1, "ann");
const stranger = person(9, "stranger");
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const PRIVATE = { "cache-control": "private, no-store" };

// The stranger's cast has 15 times the points of ann's, as fresh: first in
// everyone's hot, second once ann's is lifted 20 times.
const followed = { hash: hex(1), author: ann, text: "Followed cast", ts, likes: 10, recasts: 0, replies: 0 };
const trending = { hash: hex(9), author: stranger, text: "Stranger trending", ts, likes: 150, recasts: 0, replies: 0 };

async function serve(page: Page) {
  const asked = { following: 0, home: 0 };
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    asked.home += 1;
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [trending, followed], notes: {}, meta } });
  });
  await page.route("**/api/v1/following**", (route) => {
    asked.following += 1;
    return route.fulfill({ json: { pool: [followed], notes: {}, meta }, headers: PRIVATE });
  });
  await page.route("**/api/v1/notifications**", (route) => route.fulfill({ json: { groups: [], meta }, headers: PRIVATE }));
  await page.route("https://haatz.quilibrium.com/**", (route) =>
    route.fulfill({ headers: { "access-control-allow-origin": "*" }, json: { messages: [] } }),
  );
  return asked;
}

const order = (page: Page) =>
  page.locator("#app .cast-list .cast-card:not(.skeleton)").evaluateAll((cards) => cards.map((card) => card.querySelector(".cast-text")?.textContent?.trim()));

// Reading the first screen (including its screenshot) intentionally marks
// casts seen. On the next generation, use the reader's actual control to
// restore those rows before checking their personalized ranking.
async function expectRanking(page: Page, expected: string[], checked?: () => void) {
  await expect(page.locator("#boot")).toHaveCount(0);
  const show = page.getByRole("button", { name: "Show seen posts", exact: true });
  await expect.poll(async () => {
    // Navigation may keep the previous rows until its payload arrives. Restore
    // read posts when the new generation's notice appears, not just once before it.
    if (await show.isVisible()) await show.click();
    const ranked = await order(page);
    checked?.();
    return ranked;
  }).toEqual(expected);
}

for (const width of [1280, 390]) {
  test(`For you and Following are tabs, For you lifting whom the reader follows, at width ${width}`, async ({ browser }, testInfo) => {
    const phone = width === 390;
    const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width, height: 844 }, isMobile: phone, hasTouch: phone });
    const page = await context.newPage();
    await signIn(page, me);
    const asked = await serve(page);
    await page.goto("/");
    const tabs = page.getByRole("navigation", { name: "Feed" });
    const forYou = tabs.getByRole("link", { name: "For you" });
    const following = tabs.getByRole("link", { name: "Following" });
    await expect(forYou).toHaveAttribute("aria-current", "page");
    await expect(following).not.toHaveAttribute("aria-current", /.*/);
    // Nothing kept yet of whom they follow: everyone's order, and one read
    // of their Following page in the background to keep it.
    await expect.poll(() => order(page)).toEqual(["Stranger trending", "Followed cast"]);
    await expect.poll(() => asked.following).toBe(1);
    await expect
      .poll(() => page.evaluate(() => JSON.parse(localStorage.getItem("allthing.followed") || "null")))
      .toMatchObject({ reader: 1234, fids: [1] });
    // The list on screen stays as it is.
    expect(await order(page)).toEqual(["Stranger trending", "Followed cast"]);
    // A fingertip's height, and nothing past the page's width.
    for (const tab of [forYou, following]) expect((await tab.boundingBox())!.height).toBeGreaterThanOrEqual(44);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`feed-tabs-${width}.png`) });

    // Next time, ann's cast is lifted to the top of hot, with no new read.
    await page.reload();
    await expect(forYou).toHaveAttribute("aria-current", "page");
    await expectRanking(page, ["Followed cast", "Stranger trending"]);
    expect(asked.following).toBe(1);

    // The keyboard reaches Following in chronological order, keeping its range.
    await page.goto("/?sort=top&t=week");
    await expectRanking(page, ["Stranger trending", "Followed cast"]);
    await following.focus();
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/[?&]feed=following/);
    expect(new URL(page.url()).searchParams.get("sort")).toBeNull();
    await expect(page.getByRole("navigation", { name: "Sort" }).getByRole("link", { name: "new", exact: true })).toHaveAttribute("aria-current", "page");
    await expect(page).toHaveURL(/[?&]t=week/);
    await expect(following).toHaveAttribute("aria-current", "page");
    await expectRanking(page, ["Followed cast"]);
    // The sort tabs and the time range keep the tab.
    await expect(page.locator(".sort-tabs a", { hasText: "hot" })).toHaveAttribute("href", /feed=following/);
    await expect(page.locator(".sort-tabs a", { hasText: "hot" })).toHaveAttribute("href", /[?&]sort=hot/);
    await expect(page.getByRole("combobox", { name: "Time range" })).toBeVisible();
    // And back to For you, by the keyboard too.
    await forYou.focus();
    await page.keyboard.press("Enter");
    await expect(page).not.toHaveURL(/feed=/);
    await expect(forYou).toHaveAttribute("aria-current", "page");
    await expect(page.getByRole("navigation", { name: "Sort" }).getByRole("link", { name: "hot", exact: true })).toHaveAttribute("aria-current", "page");
    await expect(page).toHaveURL(/[?&]t=week/);
    await expectRanking(page, ["Followed cast", "Stranger trending"]);
    await context.close();
  });
}

test("ranking recovery waits for a delayed Following generation's seen notice", async ({ page }) => {
  await signIn(page, me);
  // A fresh follow set prevents a background read from filling the API cache
  // before the navigation whose first response this test deliberately holds.
  await page.addInitScript(() => localStorage.setItem("allthing.followed", JSON.stringify({ reader: 1234, at: Date.now(), fids: [1] })));
  await serve(page);
  await page.goto("/?sort=top&t=week");
  await expect.poll(() => order(page)).toEqual(["Stranger trending", "Followed cast"]);
  // Let real viewport dwell record the followed cast; do not seed or clear history.
  await expect.poll(() => page.evaluate(() => {
    const history = JSON.parse(localStorage.getItem("allthing.seen.v1") ?? '{"scopes":[]}');
    return history.scopes.find((scope: { name: string }) => scope.name === "fid:1234")?.posts.map((cast: { hash: string }) => cast.hash) ?? [];
  })).toContain(followed.hash);
  let release!: () => void;
  let requested = false;
  const held = new Promise<void>((done) => { release = done; });
  await page.route("**/api/v1/following**", async (route) => {
    requested = true;
    await held;
    await route.fulfill({ json: { pool: [followed], notes: {}, meta }, headers: PRIVATE });
  });
  try {
    const navigation = page.getByRole("navigation", { name: "Feed" }).getByRole("link", { name: "Following" }).click();
    await expect.poll(() => requested, { message: "Following navigation requests its delayed payload" }).toBe(true);
    // There is no recovery control until the destination's payload arrives.
    await expect(page.getByRole("button", { name: "Show seen posts", exact: true })).toHaveCount(0);
    let checked = false;
    let recovered = false;
    const recovery = expectRanking(page, ["Followed cast"], () => { checked = true; }).then(() => { recovered = true; });
    await expect.poll(() => checked).toBe(true);
    expect(recovered).toBe(false);
    release();
    await navigation;
    await recovery;
    await expect(page.getByRole("button", { name: "Show seen posts", exact: true })).toHaveCount(0);
  } finally {
    release();
  }
});

test("an old link to everyone's feed opens For you", async ({ page }) => {
  await signIn(page, me);
  await serve(page);
  await page.goto("/?feed=everyone");
  await expect(page.getByRole("navigation", { name: "Feed" }).getByRole("link", { name: "For you" })).toHaveAttribute("aria-current", "page");
  await expect.poll(() => order(page)).toEqual(["Stranger trending", "Followed cast"]);
});
