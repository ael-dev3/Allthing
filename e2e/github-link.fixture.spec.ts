// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Locator, type Page } from "@playwright/test";

/*
 * The source on GitHub: the footer's Source link, with GitHub's mark, on
 * every page and in the static copy; and, signed in, the avatar menu's.
 * Each opens the repository in a new tab, with no hold on Allthing's
 * (rel="noopener").
 */

const SOURCE = "https://github.com/ael-dev3/Allthing";
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const cast = { hash: `0x${"1".padStart(40, "0")}`, author: alice, text: "gm", ts: 1792235681, likes: 1, recasts: 0, replies: 0 };

/** One cast for the feed; GitHub itself is never reached, as a tab opened
 * on it gets a stand-in. */
async function serve(page: Page) {
  await page.context().route(`${SOURCE}**`, (route) => route.fulfill({ contentType: "text/html", body: "<title>Allthing on GitHub</title>" }));
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [cast], notes: {}, meta } });
  });
}

async function signIn(page: Page) {
  await page.addInitScript((user) => {
    document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax`;
  }, me);
}

/** Opens `link` (a tap on a touch screen) and checks that the repository
 * opened in a new tab that can't reach back into Allthing's, which stays
 * where it was. */
async function opensInNewTab(page: Page, link: Locator, { tap = false } = {}) {
  await expect(link).toHaveAttribute("target", "_blank");
  await expect(link).toHaveAttribute("rel", "noopener");
  const before = page.url();
  const [tab] = await Promise.all([page.waitForEvent("popup"), tap ? link.tap() : link.click()]);
  await expect(tab).toHaveURL(SOURCE);
  expect(await tab.evaluate(() => window.opener)).toBeNull();
  expect(page.url()).toBe(before);
  await tab.close();
}

test.beforeEach(async ({ page }) => serve(page));

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
    await opensInNewTab(page, source);
  });
}

test("signed in, the avatar menu links to Allthing on GitHub", async ({ page }, info) => {
  test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");
  await signIn(page);
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
  await opensInNewTab(page, link);
});

test("signed in at 390, a tap on the picture shows the menu's GitHub link, and a tap opens it", async ({ browser }, info) => {
  test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");
  const context = await browser.newContext({
    baseURL: info.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true,
  });
  const page = await context.newPage();
  await serve(page);
  await signIn(page);
  await page.goto("/");
  const picture = (await page.getByRole("link", { name: "Signed in as @reader: your profile" }).boundingBox())!;
  await page.touchscreen.tap(picture.x + picture.width / 2, picture.y + picture.height / 2);
  const menu = page.getByRole("dialog", { name: "Your account" });
  await expect(menu).toBeInViewport({ ratio: 1 });
  await menu.evaluate((el) => Promise.all(el.getAnimations().map((a) => a.finished)));
  const link = menu.getByRole("link", { name: "Allthing on GitHub" });
  await expect(link.locator("svg.github-mark")).toBeVisible();
  expect((await link.boundingBox())!.height).toBeGreaterThanOrEqual(44);
  await page.screenshot({ path: info.outputPath("github-menu-390.png") });
  await opensInNewTab(page, link, { tap: true });
  await context.close();
});
