// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";

/*
 * The source on GitHub: the footer's Source link, with GitHub's mark, on
 * every page and in the static copy; and, signed in, the avatar menu's.
 */

const SOURCE = "https://github.com/ael-dev3/Allthing";
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const cast = { hash: `0x${"1".padStart(40, "0")}`, author: alice, text: "gm", ts: 1792235681, likes: 1, recasts: 0, replies: 0 };

test.beforeEach(async ({ page }) => {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [cast], notes: {}, meta } });
  });
});

for (const width of [1280, 390]) {
  test(`the footer links to the source on GitHub, with its mark, at ${width}`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 844 });
    await page.goto(".");
    const source = page.locator(".site-footer").getByRole("link", { name: "Source on GitHub" });
    await expect(source).toHaveAttribute("href", SOURCE);
    await expect(source).toHaveText("Source");
    const mark = source.locator("svg.github-mark");
    await expect(mark).toBeVisible();
    // In the footer's colour, and the size of its text.
    const [markColor, linkColor, box] = await mark.evaluate((svg) => [
      getComputedStyle(svg).fill, getComputedStyle(svg.parentElement!).color, svg.getBoundingClientRect().height,
    ] as const);
    expect(markColor).toBe(linkColor);
    expect(box).toBe(14);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    await source.scrollIntoViewIfNeeded();
    await page.screenshot({ path: info.outputPath(`github-footer-${width}.png`) });
  });
}

test("signed in, the avatar menu links to Allthing on GitHub", async ({ page }, info) => {
  test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");
  await page.addInitScript((user) => {
    document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax`;
  }, me);
  await page.goto("/");
  await page.getByRole("link", { name: "Signed in as @reader: your profile" }).hover();
  const menu = page.getByRole("dialog", { name: "Your account" });
  const link = menu.getByRole("link", { name: "Allthing on GitHub" });
  await expect(link).toHaveAttribute("href", SOURCE);
  await expect(link.locator("svg.github-mark")).toBeVisible();
  // Above Sign out, below the reader's own links.
  const items = await menu.locator(".account-link").allTextContents();
  expect(items.indexOf("Allthing on GitHub")).toBe(items.length - 2);
  await page.screenshot({ path: info.outputPath("github-menu-1280.png") });
});
