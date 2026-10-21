// SPDX-License-Identifier: AGPL-3.0-or-later
import axe from "axe-core";
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";
import { appCame } from "./helpers/app";
import { approvedKey, kept, signIn } from "./helpers/farcaster";

/*
 * The account menu's Revoke permissions: a first click arms it, a second
 * within 5 s forgets this browser's key, so writing from here stops until
 * the reader approves Allthing again. What it does is said in Allthing's
 * own tooltip, and under it once armed, phones included. WebCrypto and
 * IndexedDB are real; the Worker and the node are stand-ins.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = Math.floor(Date.now() / 1000) - 600;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const post = { hash: hex(1), author: alice, text: "Like this one", ts, likes: 3, recasts: 0, replies: 0 };
const NODE = "https://haatz.quilibrium.com/v1";
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" };
const WHAT =
  "Removes this browser's permission to write as you: liking, recasting, casting, replying, quoting, following and deleting from here stop until you approve Allthing again. You stay signed in, and casts you scheduled still go out, as they're signed already. Allthing's key stays among your Farcaster app's connected apps until you remove it there.";

/** The page at a width: 1280 with a mouse, or a 390 px phone, by touch. */
type At = { page: Page; width: 1280 | 390; press: (control: Locator) => Promise<void>; close: () => Promise<void> };

async function at(width: 1280 | 390, desktop: Page, browser: Browser, testInfo: TestInfo): Promise<At> {
  if (width === 1280) return { page: desktop, width, press: (control) => control.click(), close: async () => {} };
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  return { page: await context.newPage(), width, press: (control) => control.tap(), close: () => context.close() };
}

/** Signed in, with a key approved an hour ago, on the front page. */
async function ready(page: Page) {
  await signIn(page, me);
  let key = "";
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [post], notes: {}, meta } });
  });
  await page.route("**/api/v1/signer", (route) => route.fulfill({ json: { app_fid: 3356318 } }));
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  await page.route(`${NODE}/signersByFid?*`, (route) => route.fulfill({ headers: cors, json: { signers: [{ key }] } }));
  await page.goto("/");
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  key = await approvedKey(page, me.fid, 3_600_000);
  const keyRead = page.waitForRequest((request) => request.url().includes("/reactionsByFid?"));
  await page.reload();
  await appCame(page);
  await keyRead;
}

/** The account menu, open: by hovering the picture, or by a tap on it. */
async function menu(it: At) {
  const picture = it.page.getByRole("link", { name: `Signed in as @${me.username}: your profile` });
  if (it.width === 1280) {
    await it.page.mouse.move(5, 300);
    await picture.hover();
  } else {
    await picture.tap();
  }
  const panel = it.page.getByRole("dialog", { name: "Your account" });
  await expect(panel).toBeVisible();
  await panel.evaluate((el) => Promise.all(el.getAnimations().map((a) => a.finished)));
  return panel;
}

const item = (panel: Locator) => panel.locator(".revoke-button");

/** Away from the menu, which closes: the pointer leaving it, or a tap outside it. */
async function away(it: At) {
  if (it.width === 1280) await it.page.mouse.move(5, 600);
  else await it.page.touchscreen.tap(10, 700);
}

/** Accessibility problems axe finds in `scope`, once its animations are done. */
async function axeViolations(page: Page, scope: string) {
  await page.evaluate(() => Promise.all(document.getAnimations().filter((a) => a.effect?.getComputedTiming().endTime !== Infinity).map((a) => a.finished.catch(() => undefined))));
  await page.evaluate(axe.source);
  return page.evaluate(async (scope) => {
    // @ts-expect-error axe is the injected script's global
    const result = await window.axe.run(scope, { resultTypes: ["violations"] });
    return result.violations.map((v: { id: string; nodes: { target: string[] }[] }) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`);
  }, scope);
}

for (const width of [1280, 390] as const) {
  test(`at ${width}, a first press only arms Revoke permissions; a second revokes, and a like then asks for approval`, async ({ page: desktop, browser }, testInfo) => {
    const it = await at(width, desktop, browser, testInfo);
    const { page } = it;
    await ready(page);
    const panel = await menu(it);
    const revoke = item(panel);
    await expect(revoke).toHaveAccessibleName("Revoke permissions");
    await expect(revoke).toHaveAccessibleDescription(WHAT);
    await page.screenshot({ path: testInfo.outputPath(`revoke-${width}-unarmed.png`) });

    // One press: armed, said under it, and nothing forgotten.
    await it.press(revoke);
    const again = width === 1280 ? "Click again to revoke" : "Tap again to revoke";
    await expect(revoke).toHaveAccessibleName(again);
    await expect(revoke).toHaveAccessibleDescription(WHAT);
    await expect(panel.locator(".revoke.armed .revoke-what")).toBeVisible();
    await expect(panel.locator(".revoke-what")).toHaveText(WHAT);
    expect(await kept(page, me.fid)).not.toBeNull();
    expect(await axeViolations(page, ".account .popover-panel")).toEqual([]);
    await page.screenshot({ path: testInfo.outputPath(`revoke-${width}-armed.png`) });

    // A second: revoked, and said so; the reader stays signed in.
    await it.press(revoke);
    await expect(page.locator(".toast.shown")).toHaveText("Write permission revoked in this browser.");
    await expect.poll(() => kept(page, me.fid)).toBeNull();
    await expect(panel.locator(".revoke-button")).toHaveCount(0);
    await expect(page.getByRole("link", { name: `Signed in as @${me.username}: your profile` })).toBeVisible();
    // Writing asks for approval again.
    await away(it);
    await expect(panel).toBeHidden();
    await it.press(page.locator(`.cast-card[data-hash="${post.hash}"]`).getByRole("button", { name: "Like", exact: true }));
    await expect(page.getByRole("dialog", { name: "Approve Allthing" })).toBeVisible();
    await it.close();
  });

  test(`at ${width}, Revoke permissions disarms after 5 s, on Escape, and as the menu closes`, async ({ page: desktop, browser }, testInfo) => {
    const it = await at(width, desktop, browser, testInfo);
    const { page } = it;
    await ready(page);
    let panel = await menu(it);
    // Left alone for 5 s.
    await it.press(item(panel));
    await expect(item(panel)).toHaveClass(/revoke-button/);
    await expect(panel.locator(".revoke.armed")).toHaveCount(1);
    await expect(panel.locator(".revoke.armed")).toHaveCount(0, { timeout: 7_000 });
    await expect(item(panel)).toHaveAccessibleName("Revoke permissions");
    // Escape disarms it, and the menu stays open.
    await it.press(item(panel));
    await expect(panel.locator(".revoke.armed")).toHaveCount(1);
    await item(panel).focus();
    await page.keyboard.press("Escape");
    await expect(panel.locator(".revoke.armed")).toHaveCount(0);
    await expect(panel).toBeVisible();
    // The menu closing, armed, takes it with it.
    await it.press(item(panel));
    await expect(panel.locator(".revoke.armed")).toHaveCount(1);
    await away(it);
    await expect(panel).toBeHidden();
    panel = await menu(it);
    await expect(item(panel)).toHaveAccessibleName("Revoke permissions");
    expect(await kept(page, me.fid)).not.toBeNull();
    await it.close();
  });
}

test("by keyboard, Enter arms Revoke permissions and Enter again revokes; Tab away disarms it", async ({ page }, testInfo) => {
  await ready(page);
  const panel = await menu({ page, width: 1280, press: (c) => c.click(), close: async () => {} });
  const revoke = item(panel);
  // Reached by Tab: its explanation shows in its tooltip.
  await revoke.focus();
  await page.keyboard.press("Shift+Tab");
  await page.keyboard.press("Tab");
  await expect(revoke).toBeFocused();
  await expect(panel.locator(".revoke-what")).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(revoke).toHaveAccessibleName("Click again to revoke");
  // Focus leaving it for more than a moment disarms it.
  await page.keyboard.press("Tab");
  await expect(panel.locator(".revoke.armed")).toHaveCount(0, { timeout: 3_000 });
  await revoke.focus();
  await page.keyboard.press(" ");
  await expect(revoke).toHaveAccessibleName("Click again to revoke");
  await page.keyboard.press(" ");
  await expect.poll(() => kept(page, me.fid)).toBeNull();
  await expect(page.locator(".toast.shown")).toHaveText("Write permission revoked in this browser.");
  await page.screenshot({ path: testInfo.outputPath("revoke-1280-keyboard.png") });
});
