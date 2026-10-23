// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, type Page } from "@playwright/test";

/**
 * Once the app has the page. The front page's first cards can be drawn
 * before it comes (web/paint.js), inert and hidden from screen readers,
 * until its own take their places; a test that reads, scrolls to or
 * focuses a card it found before then may have found one of those.
 */
export async function appCame(page: Page) {
  await expect(page.locator("#boot")).toHaveCount(0);
}

/** Recover a revisited feed through its visible control before testing another feature. */
export async function showSeenPosts(page: Page) {
  await appCame(page);
  await expect(page.locator("#app .cast-list .cast-card:not(.skeleton), #app .seen-notice").first()).toBeVisible();
  const show = page.getByRole("button", { name: "Show seen posts", exact: true });
  if (await show.isVisible()) await show.click();
}
