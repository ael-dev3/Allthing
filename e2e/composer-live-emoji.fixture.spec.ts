// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";
import { draftOf, expectDraft, selectIn, selectionIn } from "./helpers/composer";
import { approvedKey, cast as castOf, signIn, type Envelope } from "./helpers/farcaster";
import { standIn } from "./helpers/node";
import { NOW } from "./fixture-time";

/*
 * In the composer, a Pepe's shortcode shows as the Pepe as soon as it is
 * typed (":pepe_smug:" becomes the smug Pepe), while the draft, and the
 * cast that posts, keep the shortcode. The caret, selections, Backspace,
 * paste, copy and cut, Enter, and undo work on it as in a text box. The
 * node is a stand-in, and nothing is posted.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no composer");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const post = { hash: hex(1), author: alice, text: "What should I read next?", ts: (NOW - 19), likes: 3, replies: 0 };
const NODE = "https://haatz.quilibrium.com/v1";
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" };

async function serve(page: Page): Promise<Envelope[]> {
  let key = "";
  // The node: it checks each message, and answers its read-backs.
  const { sent } = await standIn(page, { fid: me.fid, key: () => key });
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => route.fulfill({ json: { pool: new URL(route.request().url()).searchParams.has("offset") ? [] : [post], notes: {}, meta } }));
  await page.route("**/api/v1/signer", (route) => route.fulfill({ json: { app_fid: 3356318 } }));
  await page.route("**/api/v1/refresh", (route) => route.fulfill({ json: { expired: 0 } }));
  await page.route(`${NODE}/signersByFid?*`, (route) => route.fulfill({ headers: cors, json: { signers: [{ key }] } }));
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  return sent;
}

/** Signed in, with a key, and the New cast composer open. */
async function compose(page: Page) {
  await signIn(page, me);
  const sent = await serve(page);
  await page.goto("/");
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  await approvedKey(page, me.fid, 3_600_000);
  await page.reload();
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  await page.getByRole("button", { name: "Write a cast" }).first().click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  const text = dialog.getByRole("textbox", { name: "What's happening?" });
  await expect(text).toBeFocused();
  return { sent, dialog, text };
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  return { context, page: await context.newPage() };
}

const emoji = (code: string) => `img.emoji[data-text=":${code}:"]`;

test("a shortcode shows as its Pepe as it is typed, and the cast keeps the shortcode", async ({ page }, testInfo) => {
  const { sent, dialog, text } = await compose(page);
  await page.keyboard.type("gm :pepe_smug");
  // Not yet a shortcode: text.
  await expect(text.locator("img.emoji")).toHaveCount(0);
  await page.keyboard.type(": frens");
  const smug = text.locator(emoji("pepe_smug"));
  await expect(smug).toBeVisible();
  await expect(smug).toHaveAttribute("alt", ":pepe_smug:");
  // Shown as the Pepe, not as its letters; typed on after it.
  await expect(text).not.toContainText("pepe_smug");
  expect(await text.evaluate((root) => Array.from(root.childNodes, (n) => (n instanceof HTMLImageElement ? "[img]" : n.nodeValue)))).toEqual(["gm ", "[img]", " frens"]);
  await expectDraft(text, "gm :pepe_smug: frens");
  // The Pepe is as tall as the text, give or take.
  const [image, line] = await Promise.all([smug.boundingBox(), text.evaluate((root) => parseFloat(getComputedStyle(root).fontSize))]);
  expect(image!.height).toBeGreaterThan(line);
  expect(image!.height).toBeLessThan(line * 1.6);
  await page.screenshot({ path: testInfo.outputPath("live-emoji-1280.png") });
  // It posts as written.
  await dialog.getByRole("button", { name: "Cast", exact: true }).click();
  await expect(dialog).toBeHidden();
  expect(castOf(sent[0]).text).toBe("gm :pepe_smug: frens");
});

test("Backspace takes the Pepe away whole, and a selection with one in it is replaced as one", async ({ page }) => {
  const { text } = await compose(page);
  await page.keyboard.type("a :pepe_no:");
  await expect(text.locator(emoji("pepe_no"))).toBeVisible();
  await page.keyboard.press("Backspace");
  await expectDraft(text, "a ");
  await expect(text.locator("img.emoji")).toHaveCount(0);
  await page.keyboard.type("b");
  await expectDraft(text, "a b");

  await text.fill("x :pepe_no: y");
  await expect(text.locator(emoji("pepe_no"))).toBeVisible();
  // From before the Pepe to after it, as a text box counts.
  await selectIn(text, 2, 11);
  expect(await selectionIn(text)).toEqual([2, 11]);
  await page.keyboard.type("z");
  await expectDraft(text, "x z y");
  expect(await selectionIn(text)).toEqual([3, 3]);
  // Arrow keys step over a Pepe in one press.
  await text.fill("1:pepe_gm:2");
  await selectIn(text, 1, 1);
  await page.keyboard.press("ArrowRight");
  expect(await selectionIn(text)).toEqual([10, 10]);
});

test("a paste is plain text with its shortcodes as Pepes; copy and cut give the shortcodes", async ({ page }) => {
  const { text } = await compose(page);
  await page.keyboard.type("hi ");
  const paste = (data: string, html = "") =>
    text.evaluate(
      (root, [data, html]) => {
        const clipboard = new DataTransfer();
        clipboard.setData("text/plain", data);
        if (html) clipboard.setData("text/html", html);
        root.dispatchEvent(new ClipboardEvent("paste", { clipboardData: clipboard, bubbles: true, cancelable: true }));
      },
      [data, html],
    );
  // Markup that comes with it is left behind.
  await paste(":pepe_gm: there\r\nfren", "<b>:pepe_gm:</b> there<br>fren");
  await expectDraft(text, "hi :pepe_gm: there\nfren");
  await expect(text.locator(emoji("pepe_gm"))).toBeVisible();
  await expect(text.locator("b")).toHaveCount(0);
  // The caret is after what was pasted.
  expect(await selectionIn(text)).toEqual([23, 23]);
  await page.keyboard.type("!");
  await expectDraft(text, "hi :pepe_gm: there\nfren!");

  const copy = (kind: "copy" | "cut") =>
    text.evaluate((root, kind) => {
      const clipboard = new DataTransfer();
      root.dispatchEvent(new ClipboardEvent(kind, { clipboardData: clipboard, bubbles: true, cancelable: true }));
      return clipboard.getData("text/plain");
    }, kind);
  await selectIn(text, 3, 18);
  expect(await copy("copy")).toBe(":pepe_gm: there");
  await expectDraft(text, "hi :pepe_gm: there\nfren!");
  expect(await copy("cut")).toBe(":pepe_gm: there");
  await expectDraft(text, "hi \nfren!");
  await expect(text.locator("img.emoji")).toHaveCount(0);
});

test("Enter is a new line; undo and redo go back and forth", async ({ page }) => {
  const { text } = await compose(page);
  await page.keyboard.type("one :pepe_gm:");
  await page.keyboard.press("Enter");
  await page.keyboard.type("two");
  await expectDraft(text, "one :pepe_gm:\ntwo");
  // Two lines, the Pepe on the first.
  const [first, second] = await text.evaluate((root) => {
    const range = document.createRange();
    range.selectNodeContents(root);
    return Array.from(range.getClientRects(), (r) => Math.round(r.top));
  }).then((tops) => [Math.min(...tops), Math.max(...tops)]);
  expect(second).toBeGreaterThan(first);
  await page.keyboard.press("Control+z");
  await expectDraft(text, "one :pepe_gm:\n");
  await page.keyboard.press("Control+z");
  await expectDraft(text, "one :pepe_gm:");
  await expect(text.locator(emoji("pepe_gm"))).toBeVisible();
  await page.keyboard.press("Control+Shift+z");
  await expectDraft(text, "one :pepe_gm:\n");
  // An empty last line takes the caret.
  await page.keyboard.type("3");
  await expectDraft(text, "one :pepe_gm:\n3");
});

test("only the pack's shortcodes become Pepes, and not inside a link", async ({ page }) => {
  const { text } = await compose(page);
  await page.keyboard.type(":nope: https://x.com/:pepe_no: :PEPE_NO:");
  await expectDraft(text, ":nope: https://x.com/:pepe_no: :PEPE_NO:");
  // As written, in any case: shown, and kept as typed.
  await expect(text.locator("img.emoji")).toHaveCount(1);
  await expect(text.locator(emoji("PEPE_NO"))).toBeVisible();
});

test("a draft taken up again shows its Pepes; the picker's pick goes in at the caret", async ({ page }) => {
  const { dialog, text } = await compose(page);
  await page.keyboard.type("say :pepe_no: to that");
  await dialog.getByRole("button", { name: "Close" }).click();
  await expect(dialog).toBeHidden();
  await page.getByRole("button", { name: "Write a cast" }).first().click();
  await expect(text.locator(emoji("pepe_no"))).toBeVisible();
  await expectDraft(text, "say :pepe_no: to that");
  // The caret is at the end.
  expect(await selectionIn(text)).toEqual([21, 21]);
  await selectIn(text, 3, 3);
  await dialog.getByRole("button", { name: /emoji/i }).first().click();
  const picker = dialog.getByRole("region", { name: "Pepe emoji" });
  await picker.locator(".emoji-tile").first().click();
  await expect(picker).toHaveCount(0);
  const draft = await draftOf(text);
  expect(draft.startsWith("say :")).toBe(true);
  expect(draft.endsWith(": :pepe_no: to that")).toBe(true);
  await expect(text.locator("img.emoji")).toHaveCount(2);
});

test("at 390, typed shortcodes show as Pepes too", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await signIn(page, me);
  const sent = await serve(page);
  await page.goto("/");
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  await approvedKey(page, me.fid, 3_600_000);
  await page.reload();
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  await page.getByRole("button", { name: "Write a cast" }).last().tap();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  const text = dialog.getByRole("textbox");
  await text.tap();
  await page.keyboard.type("gm :pepe_smug: :pepe_gm:");
  await expect(text.locator("img.emoji")).toHaveCount(2);
  await expectDraft(text, "gm :pepe_smug: :pepe_gm:");
  const box = await text.boundingBox();
  expect(box!.x).toBeGreaterThanOrEqual(0);
  expect(box!.x + box!.width).toBeLessThanOrEqual(390);
  await page.screenshot({ path: testInfo.outputPath("live-emoji-390.png") });
  await dialog.getByRole("button", { name: "Cast", exact: true }).tap();
  await expect(dialog).toBeHidden();
  expect(castOf(sent[0]).text).toBe("gm :pepe_smug: :pepe_gm:");
  await context.close();
});
