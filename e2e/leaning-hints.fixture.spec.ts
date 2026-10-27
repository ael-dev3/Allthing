// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";

/*
 * What Favorite and Show less do, said under each in Allthing's style rather
 * than in a native tooltip: on hover (after a moment) or keyboard focus, and
 * for a few seconds after a press, which is how a phone, which can't hover,
 * sees it. The hint is each button's description too.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const bob = { fid: 2, username: "bob", display_name: "Bob", follower_count: 10, following_count: 5, bio: "Writes about books." };

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: { user: bob, pool: [], notes: {}, meta } }));
  await page.goto("/u/bob");
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  return { context, page: await context.newPage() };
}

const FAVORITE = "Considerably boosts their casts in your hot and top feeds. Kept in this browser only.";
const LESS = "Omits about 90% of their posts from Home, Following and channel feeds. The rest rank much lower in hot and top; new stays chronological. Profiles stay available. Toggle again to undo. Kept in this browser only.";

/** The hint is on screen, inside the profile's card, under its button. */
async function under(page: Page, hint: Locator, button: Locator, width: number) {
  await expect(hint).toBeVisible();
  const [h, b, card] = [(await hint.boundingBox())!, (await button.boundingBox())!, (await page.locator("section.profile").boundingBox())!];
  expect(h.y).toBeGreaterThan(b.y + b.height);
  expect(h.x).toBeGreaterThanOrEqual(Math.max(0, card.x));
  expect(h.x + h.width).toBeLessThanOrEqual(Math.min(width, card.x + card.width));
}

test("at 1280, a hint says what each does on hover or keyboard focus, with no native tooltip", async ({ page }) => {
  await serve(page);
  const favorite = page.getByRole("button", { name: "Favorite" });
  const less = page.getByRole("button", { name: "Show less" });
  await expect(favorite).toBeVisible();
  await expect(page.locator(".leaning-buttons [title]")).toHaveCount(0);
  await expect(favorite).toHaveAccessibleDescription(FAVORITE);
  await expect(less).toHaveAccessibleDescription(LESS);
  const favoriteHint = page.locator("#favorite-what");
  const lessHint = page.locator("#show-less-what");
  await expect(favoriteHint).toBeHidden();
  // Pointing at one shows its hint after a moment, and only its.
  await favorite.hover();
  await under(page, favoriteHint, favorite, 1280);
  await expect(favoriteHint).toHaveText(FAVORITE);
  await expect(lessHint).toBeHidden();
  await less.hover();
  await under(page, lessHint, less, 1280);
  await expect(lessHint).toHaveText(LESS);
  await expect(favoriteHint).toBeHidden();
  await page.mouse.move(5, 600);
  await expect(lessHint).toBeHidden();
  // Keyboard focus shows it too.
  await favorite.focus();
  await page.keyboard.press("Shift+Tab");
  await page.keyboard.press("Tab");
  await expect(favorite).toBeFocused();
  await under(page, favoriteHint, favorite, 1280);
});

test("at 390, a tap says what it did for a few seconds, on screen, and leaves no hover look behind", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await serve(page);
  const favorite = page.getByRole("button", { name: "Favorite" });
  const less = page.getByRole("button", { name: "Show less" });
  await expect(favorite).toBeVisible();
  const look = (button: Locator) => button.evaluate((el) => [getComputedStyle(el).borderColor, getComputedStyle(el).color]);
  const resting = await look(favorite);
  const hint = page.locator("#favorite-what");
  await expect(hint).toBeHidden();
  await favorite.tap();
  await expect(favorite).toHaveAttribute("aria-pressed", "true");
  await under(page, hint, favorite, 390);
  await expect(hint).toHaveText(FAVORITE);
  // Gone a few seconds later.
  await expect(hint).toBeHidden({ timeout: 6000 });
  // Tapped again, it's off, and looks it once the tap's :hover lands.
  await favorite.tap();
  await expect(favorite).toHaveAttribute("aria-pressed", "false");
  await expect.poll(() => favorite.evaluate((el) => el.matches(":hover"))).toBe(true);
  // Once its colours have eased back (0.2 s): with the hover look, the
  // border would stay gold.
  await expect.poll(() => look(favorite)).toEqual(resting);
  // Show less's hint, from the second button, is on screen too, longer as
  // it is.
  await less.tap();
  await under(page, page.locator("#show-less-what"), less, 390);
  await expect(page.locator("#show-less-what")).toHaveText(LESS);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await context.close();
});
