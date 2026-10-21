// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";
import { signIn } from "./helpers/farcaster";

/*
 * Every dialog fits a phone: never wider than the screen less its margins,
 * whatever it holds, with its × on screen. The full list of who liked or
 * recasted a cast keeps each name and each handle to one line, cut short
 * with an ellipsis when it is long; a short one shows whole.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in or writes");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const cast = { hash: hex(1), author: alice, text: "A cast that many people liked", ts, likes: 120, recasts: 3, replies: 0 };
/** Long names, as real ones can be (base.eth handles run long), and a short one. */
const people = [
  ...Array.from({ length: 11 }, (_, i) => ({
    fid: 1000 + i,
    username: `averyveryverylongusername-number-${i}.base.eth`,
    display_name: `Bartholomew Montgomery-Fitzwilliam the ${i + 2}th`,
  })),
  { fid: 7, username: "bo", display_name: "Bo" },
];

async function serve(page: Page, { writes = false } = {}) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [cast], notes: {}, meta } });
  });
  await page.route("**/api/v1/reactions?*", (route) => {
    const url = new URL(route.request().url());
    const limit = Number(url.searchParams.get("limit") ?? 10);
    return route.fulfill({ json: { hash: cast.hash, kind: "likes", users: people.slice(0, limit), more: limit < people.length, meta } });
  });
  // Writes: handed to the Farcaster app, or, with `writes`, from Allthing
  // once the reader approves its key.
  await page.route("**/api/v1/signer", (route) =>
    writes
      ? route.fulfill({ json: { app_fid: 3356318 } })
      : route.fulfill({ status: 404, json: { error: "Liking and recasting from Allthing isn't set up on this server" } }),
  );
  await page.route("**/api/v1/auth/qr", (route) => route.continue());
  await page.route("**/api/v1/scheduled", (route) => route.fulfill({ json: { casts: [] } }));
  const cors = { "access-control-allow-origin": "*" };
  await page.route("https://haatz.quilibrium.com/v1/**", (route) =>
    route.fulfill({ status: 400, headers: cors, json: { error: "Failed", error_detail: 'status: NotFound, message: "not found"' } }),
  );
  await page.route("https://farcaster.xyz/**", (route) => route.fulfill({ body: "<title>Farcaster</title>", contentType: "text/html" }));
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true,
  });
  return { context, page: await context.newPage() };
}

/** The dialog's box, and its ×'s, in the viewport; and whether the × is what a tap on it reaches. */
async function placed(dialog: Locator) {
  // Once it has come in: its entrance (`pop-in`) scales it up from 98%, so
  // a box measured on the way is up to 9 px narrower than the dialog.
  await dialog.evaluate((modal) => Promise.all(modal.getAnimations().map((a) => a.finished.catch(() => undefined))));
  return dialog.evaluate((modal) => {
    const box = modal.getBoundingClientRect();
    const close = modal.querySelector(".modal-close")!;
    const x = close.getBoundingClientRect();
    const hit = document.elementFromPoint(x.left + x.width / 2, x.top + x.height / 2);
    return {
      left: box.left, right: box.right, width: box.width,
      close: { left: x.left, right: x.right },
      reachable: hit === close || close.contains(hit),
      viewport: document.documentElement.clientWidth,
    };
  });
}

/**
 * Within the screen, with the same margin each side (or, for a sheet, the
 * screen's width exactly), and its × on screen and reachable.
 */
function fits(p: Awaited<ReturnType<typeof placed>>, name: string, sheet = false) {
  if (sheet) {
    expect(p.left, `${name}: a sheet's left edge`).toBeCloseTo(0, 0);
    expect(p.right, `${name}: a sheet's right edge`).toBeCloseTo(p.viewport, 0);
  } else {
    expect(p.left, `${name}: left margin`).toBeGreaterThanOrEqual(16);
    expect(p.right, `${name}: right edge`).toBeLessThanOrEqual(p.viewport - 16);
    expect(Math.abs(p.left - (p.viewport - p.right)), `${name}: centred`).toBeLessThanOrEqual(1);
  }
  expect(p.close.right, `${name}: its ×`).toBeLessThanOrEqual(p.viewport);
  expect(p.reachable, `${name}: its × takes a tap`).toBe(true);
}

/** Something wider than any screen, put in the dialog: it must not widen it. */
async function widen(dialog: Locator) {
  await dialog.evaluate((modal) => {
    const wide = document.createElement("p");
    wide.className = "test-wide";
    wide.textContent = "Wide".repeat(120);
    Object.assign(wide.style, { whiteSpace: "nowrap", overflow: "hidden", margin: "0" });
    modal.append(wide);
  });
}

/** Each row of the who-list: its name and handle on one line each, cut short when long. */
async function rows(dialog: Locator) {
  return dialog.locator(".who-list > li").evaluateAll((items) => items.map((li) => {
    const body = li.closest(".modal-body, .popover-panel")!.getBoundingClientRect();
    const name = li.querySelector(".who-name") as HTMLElement;
    const handle = li.querySelector(".who-link > :last-child") as HTMLElement;
    const line = (el: HTMLElement) => {
      const style = getComputedStyle(el);
      // The lines its text is laid out on: one per distinct top.
      const range = document.createRange();
      range.selectNodeContents(el);
      const tops = new Set([...range.getClientRects()].map((r) => Math.round(r.top)));
      return {
        text: el.textContent,
        oneLine: tops.size === 1,
        cut: el.scrollWidth > el.clientWidth + 0.5,
        ellipsis: style.textOverflow === "ellipsis" && style.whiteSpace === "nowrap" && style.overflow !== "visible",
        right: el.getBoundingClientRect().right,
      };
    };
    return { height: li.getBoundingClientRect().height, bodyRight: body.right, name: line(name), handle: line(handle) };
  }));
}

test("on a phone, the full list of likes fits the screen, one line per name and handle", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await serve(page);
  await page.goto("/");
  const card = page.locator(`.cast-card[data-hash="${cast.hash}"]`);
  await card.locator(".stat-likes .popover-trigger").tap();
  const preview = page.getByRole("dialog", { name: "Liked by" });
  await expect(preview.getByRole("button", { name: "See everyone →" })).toBeVisible();
  // The preview's rows, too: one line each, inside the panel.
  for (const row of await rows(preview)) {
    expect(row.name.oneLine && row.handle.oneLine, `preview: ${row.handle.text} on one line`).toBe(true);
    expect(row.handle.right, `preview: ${row.handle.text} inside the panel`).toBeLessThanOrEqual(row.bodyRight + 0.5);
  }
  await preview.getByRole("button", { name: "See everyone →" }).tap();
  const dialog = page.locator(".who-modal");
  await expect(dialog.locator(".who-list > li")).toHaveCount(people.length);
  fits(await placed(dialog), "Liked by");
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  // Nothing in it scrolls sideways.
  expect(await dialog.locator(".modal-body").evaluate((b) => b.scrollWidth <= b.clientWidth)).toBe(true);
  for (const row of await rows(dialog)) {
    const who = row.handle.text;
    expect(row.name.oneLine, `${who}: its name on one line`).toBe(true);
    expect(row.handle.oneLine, `${who}: on one line`).toBe(true);
    expect(row.height, `${who}: a one-line row`).toBeLessThanOrEqual(40);
    expect(row.handle.right, `${who}: inside the dialog`).toBeLessThanOrEqual(row.bodyRight + 0.5);
    expect(row.name.ellipsis && row.handle.ellipsis, `${who}: cut short with an ellipsis`).toBe(true);
    if (who === "@bo") expect(row.name.cut || row.handle.cut, "a short name and handle show whole").toBe(false);
    else expect(row.handle.cut || row.name.cut, `${who}: cut short`).toBe(true);
  }
  await page.screenshot({ path: testInfo.outputPath("who-dialog-390.png") });
  // The × closes it.
  await dialog.getByRole("button", { name: "Close", exact: true }).tap();
  await expect(dialog).toHaveCount(0);
  await context.close();
});

test("on a computer, the full list is 440 px wide and centred, each name and handle on one line", async ({ page }, testInfo) => {
  await serve(page);
  await page.goto("/");
  await page.locator(`.cast-card[data-hash="${cast.hash}"]`).locator(".stat-likes .popover-trigger").click();
  const dialog = page.locator(".who-modal");
  await expect(dialog.locator(".who-list > li")).toHaveCount(people.length);
  const p = await placed(dialog);
  expect(p.width).toBeCloseTo(440, 0);
  expect(Math.abs(p.left - (p.viewport - p.right))).toBeLessThanOrEqual(1);
  expect(p.reachable).toBe(true);
  // A long name and handle fit on their row here.
  for (const row of await rows(dialog)) expect(row.height).toBeLessThanOrEqual(40);
  await page.screenshot({ path: testInfo.outputPath("who-dialog-1280.png") });
});

test("on a phone, every dialog stays within the screen, whatever it holds", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await serve(page);
  await page.goto("/");
  const card = page.locator(`.cast-card[data-hash="${cast.hash}"]`);
  await expect(card).toBeVisible();
  const check = async (name: string, dialog: Locator, sheet = false) => {
    await expect(dialog).toBeVisible();
    fits(await placed(dialog), name, sheet);
    await widen(dialog);
    fits(await placed(dialog), `${name}, holding something wider than the screen`, sheet);
    await page.screenshot({ path: testInfo.outputPath(`${name.replace(/\W+/g, "-").toLowerCase()}-390.png`) });
    await dialog.getByRole("button", { name: "Close", exact: true }).tap();
    await expect(dialog).toHaveCount(0);
  };
  // The full who-list.
  await card.locator(".stat-likes .popover-trigger").tap();
  await page.getByRole("dialog", { name: "Liked by" }).getByRole("button", { name: "See everyone →" }).tap();
  await check("Liked by", page.locator(".who-modal"));
  // Signing in.
  await page.getByRole("button", { name: "Sign in with Farcaster" }).tap();
  await check("Sign in", page.locator(".signin-modal"));
  // A like handed to the Farcaster app.
  await card.getByRole("button", { name: "Like", exact: true }).tap();
  await check("Like in Farcaster", page.locator(".handoff-modal"));
  // The composer, which on a phone is a sheet the screen's width.
  await page.getByRole("button", { name: "Write a cast" }).first().tap();
  await check("New cast", page.locator(".compose-modal"), true);
  await context.close();
});

test("on a phone, the dialog that approves Allthing's key stays within the screen", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await signIn(page, me);
  await serve(page, { writes: true });
  await page.goto("/");
  const card = page.locator(`.cast-card[data-hash="${cast.hash}"]`);
  await card.getByRole("button", { name: "Like", exact: true }).tap();
  const dialog = page.locator(".key-modal");
  await expect(dialog).toBeVisible();
  fits(await placed(dialog), "Approve Allthing");
  await widen(dialog);
  fits(await placed(dialog), "Approve Allthing, holding something wider than the screen");
  await page.screenshot({ path: testInfo.outputPath("approve-390.png") });
  await context.close();
});
