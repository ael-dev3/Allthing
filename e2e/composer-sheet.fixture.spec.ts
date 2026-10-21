// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";
import { signIn } from "./helpers/farcaster";

/*
 * Where the composer sits (style/allthing.css, `.compose-modal`). On a
 * computer it opens near the top of the window and grows down as a picker
 * opens, so its top and the draft stay where they are. On a phone it is a
 * sheet from the bottom of the screen, as wide as it, that grows up; with
 * the stickers open on a short phone, the panel scrolls under the draft,
 * which stays in view below the header. KLIPY is a stand-in, and the
 * Worker endorses no keys, so nothing is posted.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no composer");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const post = { hash: hex(1), author: alice, text: "Where does the composer go?", ts, likes: 3, replies: 0 };
const KLIPY = "https://api.klipy.com/api/v1/fixture-key/gifs";
const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64");

/** A KLIPY result, in its documented shape. */
const gif = (slug: string) => {
  const base = `https://static.klipy.com/ii/fixture/${slug}`;
  const format = (size: string, w: number) => ({
    gif: { url: `${base}/${size}.gif`, width: w, height: w, size: 1000 },
    webp: { url: `${base}/${size}.webp`, width: w, height: w, size: 500 },
  });
  return { id: slug.length, slug, title: slug.replace("-", " "), type: "gif", tags: [], file: { hd: format("hd", 498), md: format("md", 320), sm: format("sm", 200), xs: format("xs", 90) } };
};

/** Signed in, where the Worker endorses no keys, with the feed, the GIF key and KLIPY's trending GIFs. */
async function ready(page: Page) {
  await signIn(page, me);
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [post], notes: {}, meta } });
  });
  await page.route("**/api/v1/gifs/key", (route) => route.fulfill({ json: { key: "fixture-key" } }));
  await page.route(`${KLIPY}/**`, (route) =>
    route.fulfill({
      headers: { "access-control-allow-origin": "*" },
      json: { result: true, data: { data: Array.from({ length: 24 }, (_, i) => gif(`trending-${i + 1}`)), current_page: 1, per_page: 24, has_next: false } },
    }),
  );
  await page.route("https://static.klipy.com/**", (route) => route.fulfill({ body: png, contentType: "image/png" }));
  await page.goto("/");
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
}

async function phone(browser: Browser, testInfo: TestInfo, height: number) {
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height }, isMobile: true, hasTouch: true });
  return { context, page: await context.newPage() };
}

/** `el`'s box once its animations (the composer's coming in) are over. */
async function settled(el: Locator) {
  await el.evaluate((node) => Promise.all(node.getAnimations({ subtree: true }).map((a) => a.finished.catch(() => {}))));
  return (await el.boundingBox())!;
}

/** The draft's text box, whole, between the composer's header and its bar. */
async function draftInView(dialog: Locator) {
  const text = await settled(dialog.getByRole("textbox", { name: "What's happening?" }));
  const head = (await dialog.locator(".modal-head").boundingBox())!;
  const foot = (await dialog.locator(".compose-foot").boundingBox())!;
  expect(text.y, "the draft's top, under the header").toBeGreaterThanOrEqual(head.y + head.height - 1);
  expect(text.y + text.height, "the draft's bottom, above the bar").toBeLessThanOrEqual(foot.y + 1);
}

/** Writes two lines in the draft. */
async function twoLines(page: Page, dialog: Locator) {
  await dialog.getByRole("textbox", { name: "What's happening?" }).click();
  await page.keyboard.type("A first line");
  await page.keyboard.press("Enter");
  await page.keyboard.type("and a second");
}

test("at 1280, the composer opens near the top and stays put as its pickers open", async ({ page }, info) => {
  await ready(page);
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await expect(dialog.getByRole("button", { name: "Add a GIF" })).toBeVisible();
  await twoLines(page, dialog);
  const opened = await settled(dialog);
  // Near the top: the backdrop's 20 px, and 64 px more on a window this tall.
  expect(opened.y).toBeGreaterThanOrEqual(20);
  expect(opened.y).toBeLessThanOrEqual(85);
  expect(opened.x + opened.width / 2).toBeCloseTo(640, 0);
  const pickers: [string, Locator][] = [
    ["Add an emoji", dialog.locator(".emoji-picker")],
    ["Add a sticker", dialog.locator(".emoji-picker.stickers")],
    ["Add a GIF", dialog.locator(".gif-picker")],
  ];
  for (const [name, picker] of pickers) {
    await dialog.getByRole("button", { name }).click();
    await expect(picker).toBeVisible();
    const now = await settled(dialog);
    expect(Math.abs(now.y - opened.y), `${name}: the composer's top`).toBeLessThanOrEqual(1);
    expect(now.y + now.height, `${name}: the composer's bottom`).toBeLessThanOrEqual(900);
    await draftInView(dialog);
  }
  await page.screenshot({ path: info.outputPath("composer-1280-gifs.png") });
  // Closed and opened again, it comes back where it was.
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await page.getByRole("button", { name: "Write a cast" }).click();
  expect(Math.abs((await settled(dialog)).y - opened.y)).toBeLessThanOrEqual(1);
});

test("at 390, the composer is a sheet from the bottom, and the stickers leave the draft in view", async ({ browser }, info) => {
  const { context, page } = await phone(browser, info, 844);
  await ready(page);
  await page.getByRole("button", { name: "Write a cast" }).tap();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await expect(dialog.getByRole("button", { name: "Add a sticker" })).toBeVisible();
  const sheet = await settled(dialog);
  expect(sheet.x).toBeCloseTo(0, 0);
  expect(sheet.width).toBeCloseTo(390, 0);
  expect(sheet.y + sheet.height).toBeCloseTo(844, 0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await page.screenshot({ path: info.outputPath("composer-390-sheet.png") });
  await twoLines(page, dialog);
  await dialog.getByRole("button", { name: "Add a sticker" }).tap();
  const picker = dialog.getByRole("region", { name: "Pepe emoji and stickers" });
  await expect(picker.getByRole("tab", { name: "Stickers" })).toHaveAttribute("aria-selected", "true");
  // It grows up from where it was, whole on the screen, with the draft in view.
  const grown = await settled(dialog);
  expect(grown.y + grown.height).toBeCloseTo(844, 0);
  expect(grown.height).toBeGreaterThan(sheet.height);
  expect(grown.y).toBeGreaterThanOrEqual(0);
  await draftInView(dialog);
  // Four rows of stickers, as on Telegram.
  const whole = await picker.locator(".emoji-results").evaluate((results) => {
    const frame = results.getBoundingClientRect();
    return [...results.querySelectorAll(".sticker-tile")].filter((tile) => {
      const r = tile.getBoundingClientRect();
      return r.top >= frame.top - 1 && r.bottom <= frame.bottom + 1;
    }).length;
  });
  expect(whole).toBeGreaterThanOrEqual(12);
  await page.screenshot({ path: info.outputPath("composer-390-stickers.png") });
  await context.close();
});

test("on a short phone, the stickers' panel scrolls under the draft, which stays below the header", async ({ browser }, info) => {
  const { context, page } = await phone(browser, info, 667);
  await ready(page);
  await page.getByRole("button", { name: "Write a cast" }).tap();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await expect(dialog.getByRole("button", { name: "Add a sticker" })).toBeVisible();
  await twoLines(page, dialog);
  await dialog.getByRole("button", { name: "Add a sticker" }).tap();
  await expect(dialog.getByRole("region", { name: "Pepe emoji and stickers" })).toBeVisible();
  const sheet = await settled(dialog);
  expect(sheet.y).toBeGreaterThanOrEqual(0);
  expect(sheet.y + sheet.height).toBeCloseTo(667, 0);
  // Too short for both: what's under the draft scrolls, and the draft stays.
  const body = dialog.locator(".compose-body");
  expect(await body.evaluate((el) => el.scrollHeight > el.clientHeight)).toBe(true);
  await body.evaluate((el) => el.scrollTo(0, el.scrollHeight));
  expect(await body.evaluate((el) => el.scrollTop)).toBeGreaterThan(0);
  await draftInView(dialog);
  await page.screenshot({ path: info.outputPath("composer-390-short-stickers.png") });
  await context.close();
});
