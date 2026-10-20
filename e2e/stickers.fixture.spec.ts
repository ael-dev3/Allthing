// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";

/*
 * Allthing's stickers: larger Pepes, picked in the composer's Stickers tab
 * and sent as an image embed of their GIF at allthing.social, which every
 * Farcaster app shows; Allthing shows them as stickers. Posting hands the
 * draft to farcaster.xyz's compose link, which is stubbed.
 */

const STICKERS = "https://allthing.social/assets/stickers/v1";
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const casts = [
  { hash: hex(1), author: alice, text: "", ts, likes: 2, replies: 0, embeds: [{ kind: "image", url: `${STICKERS}/pepe_jam.gif` }] },
  {
    hash: hex(2), author: alice, text: "and a photo", ts: ts - 60, likes: 1, replies: 0,
    embeds: [{ kind: "image", url: `${STICKERS}/pepe_party.gif` }, { kind: "image", url: "https://media.example/photo.svg" }],
  },
];
const card = (page: Page, i: number) => page.locator(`.cast-card[data-hash="${casts[i].hash}"]`);
const photo = `<svg xmlns="http://www.w3.org/2000/svg" width="600" height="400"><rect width="600" height="400" fill="#8ab"/></svg>`;

async function serve(page: Page, { signedIn = true } = {}) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : casts, notes: {}, meta } });
  });
  await page.route("https://media.example/**", (route) => route.fulfill({ body: photo, contentType: "image/svg+xml" }));
  await page.route("https://farcaster.xyz/**", (route) => route.fulfill({ body: "<title>Farcaster</title>", contentType: "text/html" }));
  if (signedIn) {
    await page.addInitScript((user) => {
      document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax`;
    }, me);
  }
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  return { context, page: await context.newPage() };
}

/** Whether the image has loaded, and how it shows. */
const loaded = (img: ReturnType<Page["locator"]>) =>
  img.evaluate(async (el: HTMLImageElement) => {
    if (!el.complete) await new Promise((done) => el.addEventListener("load", done, { once: true }));
    return { natural: el.naturalWidth, height: el.getBoundingClientRect().height, src: el.currentSrc };
  });

test.describe("in the composer", () => {
  test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

  test("a sticker from the Stickers tab goes with the cast as its GIF at allthing.social", async ({ page }) => {
    await serve(page);
    await page.goto("/");
    await page.getByRole("button", { name: "Write a cast" }).click();
    const dialog = page.getByRole("dialog", { name: "New cast" });
    await expect(dialog.getByRole("textbox")).toBeFocused();
    await page.keyboard.type("gm");
    // The emoji button opens the Emoji tab; the arrow keys move to Stickers,
    // and the sticker button says so.
    const emojiButton = dialog.getByRole("button", { name: "Add an emoji" });
    const stickerButton = dialog.getByRole("button", { name: "Add a sticker" });
    await emojiButton.click();
    const picker = dialog.getByRole("region", { name: "Pepe emoji and stickers" });
    const emojiTab = picker.getByRole("tab", { name: "Emoji" });
    const stickerTab = picker.getByRole("tab", { name: "Stickers" });
    await expect(emojiTab).toHaveAttribute("aria-selected", "true");
    await emojiTab.focus();
    await page.keyboard.press("ArrowRight");
    await expect(stickerTab).toBeFocused();
    await expect(stickerTab).toHaveAttribute("aria-selected", "true");
    await expect(picker.getByRole("tabpanel", { name: "Stickers" })).toBeVisible();
    await expect(stickerButton).toHaveAttribute("aria-expanded", "true");
    await expect(emojiButton).toHaveAttribute("aria-expanded", "false");
    const tiles = picker.locator(".emoji-results > .sticker-grid .sticker-tile");
    await expect(tiles).toHaveCount(32);
    await expect(tiles.first()).toHaveAccessibleName("Sticker: Pepe jams with headphones");
    // Moving while in view, as on Telegram: each its own animated tile.
    await expect(tiles.first().locator("img")).toHaveAttribute("src", /\/assets\/stickers\/v1\/pepe_jam\.tile\.webp$/);
    const foot = picker.locator(".emoji-foot");
    await page.mouse.move(5, 5);
    await expect(foot).toContainText("Stickers show in every Farcaster app");
    await expect(foot.getByRole("link", { name: "Credits" })).toHaveAttribute("href", /\/web\/stickers\/CREDITS\.md$/);
    // Pointed at, the foot says what it is; it keeps moving after.
    await tiles.nth(1).hover();
    await expect(foot).toContainText("Pepe dances");
    await page.mouse.move(5, 5);
    await expect(tiles.nth(1).locator("img")).toHaveAttribute("src", /pepe_pls\.tile\.webp$/);
    // A search, read out; Enter picks the first and closes the picker.
    const search = picker.getByRole("searchbox", { name: "Search stickers" });
    await search.fill("dance");
    await expect(picker.getByRole("heading", { name: "2 stickers for “dance”" })).toBeVisible();
    await expect(tiles).toHaveCount(2);
    await search.press("Enter");
    await expect(picker).toHaveCount(0);
    await expect(dialog.getByRole("textbox")).toBeFocused();
    await expect(dialog.getByRole("textbox")).toHaveValue("gm");
    // Attached as the GIF would be, without its frame.
    const attached = dialog.locator(".compose-gif.compose-sticker img");
    await expect(attached).toHaveAttribute("alt", "Sticker: Pepe dances sadly");
    await expect(attached).toHaveAttribute("src", /\/assets\/stickers\/v1\/pepe_sad_dance\.gif$/);
    await expect(dialog.getByRole("button", { name: "Remove the sticker" })).toBeVisible();
    // The hand-off embeds the GIF at allthing.social, for every app to show.
    const opened = page.context().waitForEvent("page");
    await dialog.getByRole("button", { name: "Post in Farcaster" }).click();
    await page.getByRole("dialog", { name: "Post in Farcaster" }).getByRole("link", { name: /Open on farcaster\.xyz/ }).click();
    const tab = await opened;
    await tab.waitForLoadState("domcontentloaded");
    const url = new URL(tab.url());
    await tab.close();
    expect(url.searchParams.get("text")).toBe("gm");
    expect(url.searchParams.getAll("embeds[]")).toEqual([`${STICKERS}/pepe_sad_dance.gif`]);
  });

  test("the sticker button opens the Stickers tab, with the recently used first", async ({ page }) => {
    await serve(page);
    await page.goto("/");
    await page.getByRole("button", { name: "Write a cast" }).click();
    const dialog = page.getByRole("dialog", { name: "New cast" });
    const emojiButton = dialog.getByRole("button", { name: "Add an emoji" });
    const stickerButton = dialog.getByRole("button", { name: "Add a sticker" });
    // Before the GIF button, after the emoji button.
    const [emojiBox, stickerBox] = await Promise.all([emojiButton.boundingBox(), stickerButton.boundingBox()]);
    expect(emojiBox!.x + emojiBox!.width).toBeLessThan(stickerBox!.x);
    await stickerButton.click();
    const picker = dialog.getByRole("region", { name: "Pepe emoji and stickers" });
    await expect(picker.getByRole("tab", { name: "Stickers" })).toHaveAttribute("aria-selected", "true");
    await expect(stickerButton).toHaveAttribute("aria-expanded", "true");
    await expect(picker.getByRole("heading", { name: "Recently used" })).toHaveCount(0);
    // Picking two, one after the other.
    const grid = picker.locator(".emoji-results > .sticker-grid .sticker-tile");
    await grid.nth(17).click();
    await expect(picker).toHaveCount(0);
    await expect(dialog.locator(".compose-sticker img")).toHaveAttribute("alt", "Sticker: Pepe says good morning");
    await stickerButton.click();
    await picker.locator(".emoji-results > .sticker-grid .sticker-tile").nth(0).click();
    await expect(dialog.locator(".compose-sticker img")).toHaveAttribute("alt", "Sticker: Pepe jams with headphones");
    // The newest first, kept apart from the emoji.
    await stickerButton.click();
    await expect(picker.getByRole("heading", { name: "Recently used" })).toBeVisible();
    const recent = picker.locator(".emoji-recent .sticker-tile");
    await expect(recent).toHaveCount(2);
    await expect(recent.first()).toHaveAccessibleName("Sticker: Pepe jams with headphones");
    await expect(recent.nth(1)).toHaveAccessibleName("Sticker: Pepe says good morning");
    expect(await page.evaluate(() => [localStorage.getItem("allthing.stickers"), localStorage.getItem("allthing.emoji")])).toEqual(["pepe_jam pepe_gm", null]);
    // The emoji button switches the open picker to Emoji; a second press closes it.
    await emojiButton.click();
    await expect(picker.getByRole("tab", { name: "Emoji" })).toHaveAttribute("aria-selected", "true");
    await expect(picker.getByRole("heading", { name: "Recently used" })).toHaveCount(0);
    await expect(emojiButton).toHaveAttribute("aria-expanded", "true");
    await expect(stickerButton).toHaveAttribute("aria-expanded", "false");
    await emojiButton.click();
    await expect(picker).toHaveCount(0);
    // Escape from the Stickers tab returns to the sticker button.
    await stickerButton.click();
    await expect(picker.getByRole("searchbox", { name: "Search stickers" })).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(picker).toHaveCount(0);
    await expect(stickerButton).toBeFocused();
  });

  test("at 390, the Stickers tab fits", async ({ browser }, testInfo) => {
    const { context, page } = await phone(browser, testInfo);
    await serve(page);
    await page.goto("/");
    await page.getByRole("button", { name: "Write a cast" }).tap();
    const dialog = page.getByRole("dialog", { name: "New cast" });
    await dialog.getByRole("button", { name: "Add a sticker" }).tap();
    const picker = dialog.getByRole("region", { name: "Pepe emoji and stickers" });
    await expect(picker.getByRole("tab", { name: "Stickers" })).toHaveAttribute("aria-selected", "true");
    // The bar's buttons and Post fit on one line.
    const bar = await dialog.locator(".compose-bar").boundingBox();
    expect(bar!.height).toBeLessThan(48);
    const columns = await picker.locator(".sticker-grid").evaluate((grid) => getComputedStyle(grid).gridTemplateColumns.split(" ").length);
    expect(columns).toBeGreaterThanOrEqual(4);
    const box = await picker.boundingBox();
    expect(box!.x + box!.width).toBeLessThanOrEqual(390);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    // Four rows in view, as on Telegram, with the dialog still on screen.
    const whole = await picker.locator(".emoji-results").evaluate((results) => {
      const frame = results.getBoundingClientRect();
      return [...results.querySelectorAll(".sticker-tile")].filter((tile) => {
        const r = tile.getBoundingClientRect();
        return r.top >= frame.top - 1 && r.bottom <= frame.bottom + 1;
      }).length;
    });
    expect(whole).toBeGreaterThanOrEqual(12);
    await expect(dialog).toBeInViewport({ ratio: 1 });
    // Only those in view load and move; the rest as they scroll in.
    const results = picker.locator(".emoji-results");
    await expect.poll(() => picker.locator(".sticker-tile img[src*='.tile.webp']").count()).toBeGreaterThanOrEqual(16);
    expect(await picker.locator(".sticker-tile img[data-src]").count()).toBeGreaterThan(0);
    await results.evaluate((r) => r.scrollTo(0, r.scrollHeight));
    await expect(picker.locator(".sticker-tile img[data-src]")).toHaveCount(0);
    await results.evaluate((r) => r.scrollTo(0, 0));
    await page.screenshot({ path: testInfo.outputPath("stickers-390.png") });
    // Held down, one shows large; let go, nothing is picked.
    const held = picker.locator(".emoji-results > .sticker-grid .sticker-tile").nth(2);
    await held.dispatchEvent("pointerdown", { pointerType: "touch", isPrimary: true, button: 0 });
    const preview = picker.locator(".sticker-preview");
    await expect(preview).toBeVisible();
    await expect(preview.locator("img")).toHaveAttribute("src", /pepe_sad_dance\.tile\.webp$/);
    await expect(preview).toContainText("Pepe dances sadly");
    const large = (await preview.locator("img").boundingBox())!;
    expect(large.height).toBeGreaterThan(120);
    await page.screenshot({ path: testInfo.outputPath("stickers-390-held.png") });
    await held.dispatchEvent("pointerup", { pointerType: "touch", isPrimary: true, button: 0 });
    await held.dispatchEvent("click", { detail: 1 });
    await expect(preview).toHaveCount(0);
    await expect(picker).toBeVisible();
    await expect(dialog.locator(".compose-sticker")).toHaveCount(0);
    // A tap picks one, and closes the picker: a cast takes one.
    await picker.locator(".sticker-tile").nth(4).tap();
    await expect(picker).toHaveCount(0);
    await expect(dialog.locator(".compose-sticker img")).toHaveAttribute("alt", "Sticker: Pepe claps");
    await context.close();
  });
});

test.describe("held, and still", () => {
  test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

  test("a sticker held down with the mouse shows large; a click picks it", async ({ page }, testInfo) => {
    await serve(page);
    await page.goto("/");
    await page.getByRole("button", { name: "Write a cast" }).click();
    const dialog = page.getByRole("dialog", { name: "New cast" });
    await dialog.getByRole("button", { name: "Add a sticker" }).click();
    const picker = dialog.getByRole("region", { name: "Pepe emoji and stickers" });
    const tile = picker.locator(".emoji-results > .sticker-grid .sticker-tile").nth(4);
    await tile.hover();
    await page.mouse.down();
    const preview = picker.locator(".sticker-preview");
    await expect(preview).toBeVisible();
    await expect(preview).toContainText("Pepe claps");
    await page.screenshot({ path: testInfo.outputPath("stickers-1280-held.png") });
    await page.mouse.up();
    await expect(preview).toHaveCount(0);
    await expect(dialog.locator(".compose-sticker")).toHaveCount(0);
    await tile.click();
    await expect(dialog.locator(".compose-sticker img")).toHaveAttribute("alt", "Sticker: Pepe claps");
  });

  for (const [why, setup] of [
    ["with reduced motion", async (page: Page) => page.emulateMedia({ reducedMotion: "reduce" })],
    ["saving data", async (page: Page) => page.addInitScript(() => Object.defineProperty(navigator, "connection", { value: { saveData: true } }))],
  ] as const) {
    test(`${why}, the Stickers tab shows stills`, async ({ page }) => {
      await setup(page);
      await serve(page);
      await page.goto("/");
      await page.getByRole("button", { name: "Write a cast" }).click();
      const dialog = page.getByRole("dialog", { name: "New cast" });
      await dialog.getByRole("button", { name: "Add a sticker" }).click();
      const tiles = dialog.locator(".emoji-results > .sticker-grid .sticker-tile");
      await expect(tiles.first().locator("img")).toHaveAttribute("src", /pepe_jam\.still\.webp$/);
      await expect(tiles.first()).not.toHaveAttribute("data-moving");
    });
  }
});

test("a cast's sticker shows as a sticker, apart from its photos", async ({ page }) => {
  test.skip(Boolean(process.env.E2E_PAGES), "the fixture feed needs the Worker's API");
  await serve(page, { signedIn: false });
  await page.goto("/");
  const jam = card(page, 0).locator(".sticker img");
  await expect(jam).toHaveAttribute("alt", "Sticker: Pepe jams with headphones");
  const shown = await loaded(jam);
  expect(shown.natural).toBe(132);
  expect(shown.src).toMatch(/\/assets\/stickers\/v1\/pepe_jam\.gif$/);
  expect(shown.height).toBeCloseTo(128, 1);
  // No grid tile and no viewer: a sticker is not a photo.
  await expect(card(page, 0).locator(".media-grid")).toHaveCount(0);
  // Beside a photo, the sticker first, then the photo's grid of one.
  await expect(card(page, 1).locator(".sticker img")).toHaveAttribute("alt", "Sticker: Pepe parties");
  await expect(card(page, 1).locator(".media-grid.n1 img")).toHaveCount(1);
});

test("with reduced motion, a cast's sticker stands still", async ({ page }) => {
  test.skip(Boolean(process.env.E2E_PAGES), "the fixture feed needs the Worker's API");
  await page.emulateMedia({ reducedMotion: "reduce" });
  await serve(page, { signedIn: false });
  await page.goto("/");
  const shown = await loaded(card(page, 0).locator(".sticker img"));
  expect(shown.src).toMatch(/\/assets\/stickers\/v1\/pepe_jam\.still\.webp$/);
  expect(shown.natural).toBe(132);
});

test("at 390, a cast's sticker fits", async ({ browser }, testInfo) => {
  test.skip(Boolean(process.env.E2E_PAGES), "the fixture feed needs the Worker's API");
  const { context, page } = await phone(browser, testInfo);
  await serve(page, { signedIn: false });
  await page.goto("/");
  expect((await loaded(card(page, 0).locator(".sticker img"))).height).toBeCloseTo(128, 1);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await context.close();
});

test("stickers are served from the site itself, as GIFs cached for a year", async ({ request }) => {
  test.skip(Boolean(process.env.E2E_PAGES), "GitHub Pages sets its own caching");
  const file = await request.get("/assets/stickers/v1/pepe_jam.gif");
  expect(file.status()).toBe(200);
  expect(file.headers()["content-type"]).toBe("image/gif");
  expect(file.headers()["cache-control"]).toContain("immutable");
});
