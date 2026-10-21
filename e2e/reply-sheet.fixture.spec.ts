// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";
import { expectDraft } from "./helpers/composer";
import { signIn } from "./helpers/farcaster";

/*
 * On a phone the composer is a sheet that sits on the keyboard, as the
 * Farcaster app's composer does: Cancel on the left of its head and Reply
 * on the right, where the keyboard can't cover them, and its tools right
 * above the keyboard, following the visual viewport as the keyboard comes
 * and goes. On an iPhone, the tap that opens it gives focus at once to a
 * text field out of sight, which iOS's WebKit needs to bring up its
 * keyboard, and the text box takes focus from it as it's drawn; elsewhere
 * the box's own focus does. Chromium has no on-screen keyboard: these tests
 * stand in a visual viewport whose height they set.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no composer");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const now = Math.floor(Date.now() / 1000);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const post = { hash: hex(1), author: alice, text: "What should I read next?", ts: now - 3600, likes: 3, replies: 0 };
const KEYBOARD = 336;

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: { thread: { cast: post, children: [] }, notes: {}, meta } }));
  await page.route("https://haatz.quilibrium.com/**", (route) => route.fulfill({ headers: { "access-control-allow-origin": "*" }, json: { messages: [] } }));
}

/** Safari's on an iPhone. */
const IPHONE = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";

/** A phone, with a visual viewport the test sizes (`__keyboard(px)`), and focus noted as it moves. */
async function phone(browser: Browser, testInfo: TestInfo, userAgent?: string) {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL,
    viewport: { width: 390, height: 844 },
    isMobile: true,
    hasTouch: true,
    ...(userAgent ? { userAgent } : {}),
  });
  const page = await context.newPage();
  await page.addInitScript(() => {
    const viewport = new EventTarget();
    let height = innerHeight;
    let top = 0;
    Object.defineProperty(viewport, "height", { get: () => height });
    Object.defineProperty(viewport, "offsetTop", { get: () => top });
    Object.defineProperty(viewport, "width", { get: () => innerWidth });
    Object.defineProperty(window, "visualViewport", { configurable: true, get: () => viewport });
    const w = window as unknown as { __keyboard: (px: number, scrolled?: number) => void; __focus: string[] };
    w.__keyboard = (px, scrolled = 0) => {
      height = innerHeight - px - scrolled;
      top = scrolled;
      viewport.dispatchEvent(new Event("resize"));
    };
    w.__focus = [];
    addEventListener("focusin", (e) => {
      const el = e.target as HTMLElement;
      w.__focus.push(el.id || el.getAttribute("aria-label") || el.className);
    });
  });
  await signIn(page, me);
  await serve(page);
  await page.goto(`/p/${post.hash}`);
  await expect(page.locator(".post")).toBeVisible();
  return { context, page };
}

const boxOf = (page: Page, selector: string) => page.locator(selector).first().evaluate((el) => el.getBoundingClientRect().toJSON() as DOMRect);

test("at 390, Cancel on the left and Reply on the right of the sheet's head; the title names it, unseen", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await page.locator(".post").getByRole("button", { name: "Reply to @alice" }).tap();
  const dialog = page.getByRole("dialog", { name: "Reply" });
  // Reply, or, where Allthing doesn't post for this reader, Post in Farcaster.
  const reply = dialog.locator(".compose-post");
  await expect(reply).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Close", exact: true })).toBeHidden();
  const cancel = dialog.getByRole("button", { name: "Cancel" });
  await expect(cancel).toBeVisible();
  await expect(dialog.locator("#compose-title")).toHaveText("Reply");
  await dialog.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished.catch(() => {}))));
  const [head, cancelBox, replyBox, bar] = await Promise.all([
    boxOf(page, ".compose-head"),
    cancel.boundingBox(),
    reply.boundingBox(),
    boxOf(page, ".compose-bar"),
  ]);
  // Both in the head, Cancel at its left, Reply at its right, 36 px tall.
  for (const box of [cancelBox!, replyBox!]) {
    expect(box.y).toBeGreaterThanOrEqual(head.y);
    expect(box.y + box.height).toBeLessThanOrEqual(head.y + head.height + 0.5);
    expect(box.height).toBe(36);
  }
  expect(cancelBox!.x).toBeLessThan(40);
  expect(Math.round(390 - (replyBox!.x + replyBox!.width))).toBe(14);
  // The bar keeps the tools, Reply out of it.
  expect(replyBox!.y).toBeLessThan(bar.y);
  await expect(dialog.locator(".compose-bar .compose-tool").first()).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("sheet-390.png") });
  // Cancel closes it, keeping the draft, as × does.
  await page.keyboard.type("Hyperion");
  await cancel.tap();
  await expect(dialog).toHaveCount(0);
  await page.locator(".post").getByRole("button", { name: "Reply to @alice" }).tap();
  await expectDraft(dialog.getByRole("textbox", { name: "Cast your reply" }), "Hyperion");
  await context.close();
});

test("at 390, the sheet sits on the keyboard: its tools right above it, its head in view, as it comes and goes", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await page.locator(".post").getByRole("button", { name: "Reply to @alice" }).tap();
  const dialog = page.getByRole("dialog", { name: "Reply" });
  await expect(dialog.locator(".compose-bar .compose-tool").first()).toBeVisible();
  await dialog.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished.catch(() => {}))));
  expect(Math.round((await boxOf(page, ".compose-modal")).bottom)).toBe(844);
  // Set on the sheet alone, and only with a keyboard up: on the page's root,
  // every element on the page would be styled again as the composer opens.
  const set = () =>
    page.evaluate(() => [document.documentElement, document.querySelector(".compose-modal")].map((el) => (el as HTMLElement).style.getPropertyValue("--keyboard")));
  expect(await set()).toEqual(["", ""]);
  // The keyboard comes up: the sheet's foot sits on it, and its head is in view.
  await page.evaluate((px) => (window as unknown as { __keyboard: (px: number) => void }).__keyboard(px), KEYBOARD);
  await expect.poll(async () => Math.round((await boxOf(page, ".compose-modal")).bottom)).toBe(844 - KEYBOARD);
  expect(await set()).toEqual(["", `${KEYBOARD}px`]);
  const sheet = await boxOf(page, ".compose-modal");
  expect(sheet.top).toBeGreaterThanOrEqual(0);
  expect(Math.round((await boxOf(page, ".compose-foot")).bottom)).toBe(844 - KEYBOARD);
  const reply = (await dialog.locator(".compose-post").boundingBox())!;
  expect(reply.y).toBeGreaterThanOrEqual(0);
  await page.screenshot({ path: testInfo.outputPath("sheet-keyboard-390.png") });
  // The page scrolled 100 px under it by the browser, as iOS does to show
  // a field: still on the keyboard, and whole within what's in view.
  await page.evaluate((px) => (window as unknown as { __keyboard: (px: number, s: number) => void }).__keyboard(px, 100), KEYBOARD);
  await expect.poll(async () => Math.round((await boxOf(page, ".compose-modal")).height)).toBeLessThanOrEqual(844 - KEYBOARD - 100 - 24);
  const scrolled = await boxOf(page, ".compose-modal");
  expect(Math.round(scrolled.bottom)).toBe(844 - KEYBOARD);
  expect(scrolled.top).toBeGreaterThanOrEqual(100);
  // The keyboard goes: the sheet goes back down.
  await page.evaluate(() => (window as unknown as { __keyboard: (px: number) => void }).__keyboard(0));
  await expect.poll(async () => Math.round((await boxOf(page, ".compose-modal")).bottom)).toBe(844);
  // Closed, nothing is left set on the page.
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  expect(await page.evaluate(() => document.documentElement.style.getPropertyValue("--keyboard"))).toBe("");
  await context.close();
});

test("on an iPhone, the tap gives focus at once to a field out of sight, for iOS's keyboard, and the box takes it from there", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo, IPHONE);
  await page.locator(".post").getByRole("button", { name: "Reply to @alice" }).tap();
  const box = page.getByRole("dialog", { name: "Reply" }).getByRole("textbox", { name: "Cast your reply" });
  await expect(box).toBeFocused();
  const moves = await page.evaluate(() => (window as unknown as { __focus: string[] }).__focus);
  // The button, the field the tap focused, then the box.
  expect(moves.slice(-2)).toEqual(["keyboard-proxy", "Cast your reply"]);
  // The field is out of sight, out of the tab order, and hidden from screen readers.
  const field = page.locator("#keyboard-proxy");
  await expect(field).toHaveAttribute("tabindex", "-1");
  await expect(field).toHaveAttribute("aria-hidden", "true");
  expect(await field.evaluate((el) => getComputedStyle(el).opacity)).toBe("0");
  await context.close();
});

test("on another phone, the box takes focus itself: no field out of sight, and no other focus in the tap", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await page.locator(".post").getByRole("button", { name: "Reply to @alice" }).tap();
  const box = page.getByRole("dialog", { name: "Reply" }).getByRole("textbox", { name: "Cast your reply" });
  await expect(box).toBeFocused();
  const moves = await page.evaluate(() => (window as unknown as { __focus: string[] }).__focus);
  expect(moves).not.toContain("keyboard-proxy");
  expect(moves.at(-1)).toBe("Cast your reply");
  await expect(page.locator("#keyboard-proxy")).toHaveCount(0);
  await context.close();
});

test("at 1280, nothing changes: × in the head, Reply in the bar, no field taking focus first", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await signIn(page, me);
  await serve(page);
  await page.goto(`/p/${post.hash}`);
  await page.locator(".post").getByRole("button", { name: "Reply to @alice" }).click();
  const dialog = page.getByRole("dialog", { name: "Reply" });
  await expect(dialog.getByRole("button", { name: "Close", exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Cancel" })).toBeHidden();
  await expect(dialog.locator("#compose-title")).toBeVisible();
  const button = (await dialog.locator(".compose-post").boundingBox())!;
  const bar = (await dialog.locator(".compose-bar").boundingBox())!;
  expect(button.y).toBeGreaterThanOrEqual(bar.y - 0.5);
  await expect(page.locator("#keyboard-proxy")).toHaveCount(0);
});
