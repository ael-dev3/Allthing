// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";

/*
 * The composer's emoji picker: Allthing's Pepe pack, searched, picked into
 * the draft at the caret, the reader's recently used emoji first, and each
 * still until pointed at. KLIPY is a stand-in that finds nothing.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const cast = { hash: `0x${"1".padStart(40, "0")}`, author: alice, text: "Say it with a Pepe", ts: 1792235681, likes: 3, replies: 0 };
/** The pack's groups, in order (`allthing_core::emoji::Group`). */
const GROUPS = ["Classics", "Reactions", "Happy", "Love", "Sad and scared", "Music and party", "Hi and bye", "Food and drink", "Doing things", "Dressed up", "Signs"];

async function serve(target: Page) {
  await target.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await target.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [cast], notes: {}, meta } });
  });
  await target.route("**/api/v1/gifs/key", (route) => route.fulfill({ json: { key: "fixture-key" } }));
  await target.route("https://api.klipy.com/**", (route) =>
    route.fulfill({ json: { result: true, data: { data: [], has_next: false } }, headers: { "access-control-allow-origin": "*" } }),
  );
  await target.addInitScript((user) => {
    document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax`;
  }, me);
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true,
  });
  return { context, page: await context.newPage() };
}

/** The composer, opened for a new cast. */
async function compose(page: Page, tap = false) {
  await page.goto("/");
  const write = page.getByRole("button", { name: "Write a cast" });
  await (tap ? write.tap() : write.click());
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await expect(dialog).toBeVisible();
  return { dialog, text: dialog.getByRole("textbox", { name: "What's happening?" }) };
}

test("a search, Enter, a click, Shift for another, and the recently used first", async ({ page }) => {
  await serve(page);
  const { dialog, text } = await compose(page);
  await expect(text).toBeFocused();
  await page.keyboard.type("gm");
  // The emoji button, before the GIF button, which comes once its key does.
  const button = dialog.getByRole("button", { name: "Add an emoji" });
  const gifs = dialog.getByRole("button", { name: "Add a GIF" });
  await expect(gifs).toBeVisible();
  const [emojiBox, gifBox] = await Promise.all([button.boundingBox(), gifs.boundingBox()]);
  expect(emojiBox!.x + emojiBox!.width).toBeLessThan(gifBox!.x);
  expect(Math.abs(emojiBox!.y + emojiBox!.height / 2 - (gifBox!.y + gifBox!.height / 2))).toBeLessThan(1.5);
  await expect(button).toHaveAttribute("aria-expanded", "false");
  await button.click();
  await expect(button).toHaveAttribute("aria-expanded", "true");
  const picker = dialog.getByRole("region", { name: "Pepe emoji" });
  const search = picker.getByRole("searchbox", { name: "Search emoji" });
  await expect(search).toBeFocused();
  // The pack's grid; the recently used have one of their own.
  const tiles = picker.locator(".emoji-results > .emoji-grid .emoji-tile");
  await expect(tiles).toHaveCount(219);
  await expect(picker.getByRole("heading", { name: "Recently used" })).toHaveCount(0);
  // Under its groups' headings, with a bar of the groups, the first marked.
  await expect(picker.getByRole("heading", { level: 4 })).toHaveText(GROUPS);
  const bar = picker.getByRole("group", { name: "Groups" });
  expect(await bar.getByRole("button").evaluateAll((buttons) => buttons.map((b) => b.getAttribute("aria-label")))).toEqual(GROUPS);
  await expect(bar.getByRole("button", { name: "Classics" })).toHaveAttribute("aria-current", "true");
  // The five first, still; an animated one moves while pointed at, and the foot names it.
  await expect(tiles.first()).toHaveAccessibleName("Pepe holds up a NO sign");
  await expect(tiles.first().locator("img")).toHaveAttribute("src", /\/assets\/emoji\/v1\/pepe_no\.webp$/);
  const foot = picker.locator(".emoji-foot");
  // The dialog grew under the pointer: away from the emoji, the foot says what other apps show.
  await page.mouse.move(5, 5);
  await expect(foot).toContainText("Other apps show the :shortcode:");
  await expect(foot.getByRole("link", { name: "Credits" })).toHaveAttribute("href", /\/web\/emoji\/CREDITS\.md$/);
  const nopers = picker.getByRole("button", { name: "Pepe shakes his head no", exact: true });
  await expect(nopers.locator("img")).toHaveAttribute("src", /pepe_nopers\.still\.webp$/);
  await nopers.hover();
  await expect(nopers.locator("img")).toHaveAttribute("src", /pepe_nopers\.webp$/);
  await expect(foot).toContainText(":pepe_nopers:");
  await expect(foot.locator(".emoji-preview img")).toHaveAttribute("src", /pepe_nopers\.webp$/);
  // A search, whose count is read out; Enter picks the first match and closes.
  await search.fill("heart");
  await expect(picker.getByRole("heading", { name: "3 emoji for “heart”" })).toBeVisible();
  // The one pointed at is gone from the results, and from the foot.
  await expect(foot).toContainText("Other apps show the :shortcode:");
  await expect(tiles).toHaveCount(3);
  await expect(tiles.first()).toHaveAccessibleName("Pepe hugs a big heart");
  await search.fill("zzz");
  await expect(picker.getByRole("heading", { name: "No emoji for “zzz”" })).toBeVisible();
  await expect(tiles).toHaveCount(0);
  await search.fill(":pepe_heart");
  await search.press("Enter");
  await expect(picker).toHaveCount(0);
  await expect(button).toHaveAttribute("aria-expanded", "false");
  // In the text, spaced, with the caret after it.
  await expect(text).toHaveValue("gm :pepe_heart_hug: ");
  await expect(text).toBeFocused();
  await page.keyboard.type("frens");
  await expect(text).toHaveValue("gm :pepe_heart_hug: frens");
  // The recently used come first; Shift+click keeps the picker open.
  await button.click();
  await expect(picker.getByRole("heading", { name: "Recently used" })).toBeVisible();
  const recent = picker.locator(".emoji-recent .emoji-tile");
  await expect(recent).toHaveCount(1);
  await expect(recent.first()).toHaveAccessibleName("Pepe hugs a big heart");
  await picker.getByRole("button", { name: "Pepe dances on the spot", exact: true }).first().click({ modifiers: ["Shift"] });
  await expect(picker).toBeVisible();
  await expect(text).toHaveValue("gm :pepe_heart_hug: frens :pepe_pls: ");
  // A search hides the recently used.
  await search.fill("no");
  await expect(picker.locator(".emoji-recent")).toBeHidden();
  await expect(tiles).toHaveCount(5);
  // A plain click picks, closes, and puts the caret after it.
  await tiles.first().click();
  await expect(picker).toHaveCount(0);
  await expect(text).toBeFocused();
  await expect(text).toHaveValue("gm :pepe_heart_hug: frens :pepe_pls: :pepe_no: ");
  // In place of a selection, in the middle of the text.
  await text.fill("say no to that");
  await text.evaluate((area: HTMLTextAreaElement) => area.setSelectionRange(4, 6));
  await button.click();
  await expect(picker.locator(".emoji-recent .emoji-tile")).toHaveCount(3);
  await expect(picker.locator(".emoji-recent .emoji-tile").first()).toHaveAccessibleName("Pepe holds up a NO sign");
  await picker.locator(".emoji-recent .emoji-tile").first().click();
  await expect(text).toHaveValue("say :pepe_no: to that");
  await expect.poll(() => text.evaluate((area: HTMLTextAreaElement) => [area.selectionStart, area.selectionEnd])).toEqual([13, 13]);
  // Kept between visits.
  expect(await page.evaluate(() => localStorage.getItem("allthing.emoji"))).toBe("pepe_no pepe_pls pepe_heart_hug");
});

test("one picker at a time, and Escape closes the picker before the dialog", async ({ page }) => {
  await serve(page);
  const { dialog } = await compose(page);
  const emoji = dialog.getByRole("button", { name: "Add an emoji" });
  const gifs = dialog.getByRole("button", { name: "Add a GIF" });
  const emojiPicker = dialog.getByRole("region", { name: "Pepe emoji" });
  const gifPicker = dialog.getByRole("region", { name: "GIFs from KLIPY" });
  await emoji.click();
  await expect(emojiPicker).toBeVisible();
  await gifs.click();
  await expect(gifPicker).toBeVisible();
  await expect(emojiPicker).toHaveCount(0);
  await expect(emoji).toHaveAttribute("aria-expanded", "false");
  await emoji.click();
  await expect(emojiPicker).toBeVisible();
  await expect(gifPicker).toHaveCount(0);
  // The button closes it, too.
  await emoji.click();
  await expect(emojiPicker).toHaveCount(0);
  await emoji.click();
  await expect(emojiPicker.getByRole("searchbox")).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(emojiPicker).toHaveCount(0);
  await expect(emoji).toBeFocused();
  await expect(dialog).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
});

/** How far below the top of the grid a group's heading is, in pixels. */
function headingTop(picker: Locator, name: string) {
  return picker.locator(".emoji-results").evaluate((grid, name) => {
    const heading = [...grid.querySelectorAll("h4")].find((h) => h.textContent === name)!;
    // `+ 0`: a hair above rounds to -0, which isn't 0 to `toBe`.
    return Math.round(heading.getBoundingClientRect().top - grid.getBoundingClientRect().top) + 0;
  }, name);
}

test("the groups' bar brings a group to the top, and marks the one in view as the grid scrolls", async ({ page }) => {
  await serve(page);
  const { dialog } = await compose(page);
  await dialog.getByRole("button", { name: "Add an emoji" }).click();
  const picker = dialog.getByRole("region", { name: "Pepe emoji" });
  const bar = picker.getByRole("group", { name: "Groups" });
  const grid = picker.locator(".emoji-results");
  const marked = (name: string) => expect(bar.getByRole("button", { name, exact: true })).toHaveAttribute("aria-current", "true");
  // Each button's face is its group's first emoji, still.
  await expect(bar.getByRole("button", { name: "Food and drink" }).locator("img")).toHaveAttribute("src", /pepe_tea\.still\.webp$/);
  await bar.getByRole("button", { name: "Food and drink" }).click();
  await expect.poll(() => headingTop(picker, "Food and drink")).toBe(0);
  await marked("Food and drink");
  await expect(bar.locator("[aria-current]")).toHaveCount(1);
  // Scrolled by hand, the mark follows the heading that has reached the top.
  await grid.evaluate((g) => g.scrollTo(0, 0));
  await marked("Classics");
  await grid.evaluate((g) => {
    const love = [...g.querySelectorAll("h4")].find((h) => h.textContent === "Love") as HTMLElement;
    g.scrollTo(0, love.offsetTop + 20);
  });
  await marked("Love");
  // The last group can't reach the top; chosen, it is still the one marked.
  await bar.getByRole("button", { name: "Signs" }).click();
  await marked("Signs");
  await expect(picker.getByRole("heading", { level: 4, name: "Signs" })).toBeInViewport();
  // From the keyboard too.
  await bar.getByRole("button", { name: "Happy" }).focus();
  await page.keyboard.press("Enter");
  await expect.poll(() => headingTop(picker, "Happy")).toBe(0);
  await marked("Happy");
  // A search shows what it found, without groups; cleared, they're back.
  await picker.getByRole("searchbox").fill("wizard");
  await expect(bar).toBeHidden();
  await expect(picker.getByRole("heading", { level: 4 })).toHaveCount(0);
  await expect(picker.getByRole("heading", { name: "1 emoji for “wizard”" })).toBeVisible();
  await picker.getByRole("searchbox").fill("");
  await expect(bar).toBeVisible();
  await expect(picker.getByRole("heading", { level: 4 })).toHaveCount(GROUPS.length);
  await marked("Classics");
});

test("with reduced motion, the emoji stay still", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await serve(page);
  const { dialog } = await compose(page);
  await dialog.getByRole("button", { name: "Add an emoji" }).click();
  const picker = dialog.getByRole("region", { name: "Pepe emoji" });
  const jam = picker.getByRole("button", { name: "Pepe jams with headphones", exact: true });
  await jam.hover();
  await expect(picker.locator(".emoji-foot")).toContainText(":pepe_jam:");
  await expect(jam.locator("img")).toHaveAttribute("src", /pepe_jam\.still\.webp$/);
  await expect(picker.locator(".emoji-preview img")).toHaveAttribute("src", /pepe_jam\.still\.webp$/);
});

test("at 390, the picker fits, and a tap leaves it open for another", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await serve(page);
  const { dialog, text } = await compose(page, true);
  await dialog.getByRole("button", { name: "Add an emoji" }).tap();
  const picker = dialog.getByRole("region", { name: "Pepe emoji" });
  await expect(picker).toBeVisible();
  // The phone's keyboard stays down, so the emoji can be seen.
  await expect(picker.getByRole("searchbox")).not.toBeFocused();
  const tiles = picker.locator(".emoji-tile");
  await tiles.nth(0).tap();
  await expect(picker).toBeVisible();
  await tiles.nth(1).tap();
  await expect(text).toHaveValue(":pepe_no: :pepe_smug: ");
  await expect(picker).toBeVisible();
  // Seven columns at least, inside the screen, and no sideways scroll.
  const box = await picker.boundingBox();
  expect(box!.x).toBeGreaterThanOrEqual(0);
  expect(box!.x + box!.width).toBeLessThanOrEqual(390);
  const columns = await picker.locator(".emoji-grid").last().evaluate((grid) => getComputedStyle(grid).gridTemplateColumns.split(" ").length);
  expect(columns).toBeGreaterThanOrEqual(7);
  // The groups' bar fits on one row, each button wide enough to tap.
  const bar = picker.getByRole("group", { name: "Groups" });
  const barBox = (await bar.boundingBox())!;
  expect(barBox.x + barBox.width).toBeLessThanOrEqual(box!.x + box!.width);
  for (const button of await bar.getByRole("button").all()) {
    const b = (await button.boundingBox())!;
    expect(b.width).toBeGreaterThanOrEqual(24);
    expect(b.height).toBeGreaterThanOrEqual(24);
    expect(Math.abs(b.y - barBox.y)).toBeLessThan(1);
  }
  await bar.getByRole("button", { name: "Hi and bye" }).tap();
  await expect.poll(() => headingTop(picker, "Hi and bye")).toBe(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  // The draft's post button stays in view, below what scrolls.
  await expect(dialog.getByRole("button", { name: "Post in Farcaster" }).or(dialog.getByRole("link", { name: /Post in Farcaster/ }))).toBeInViewport();
  await context.close();
});
