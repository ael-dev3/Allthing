// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { expectDraft } from "./helpers/composer";
import { approvedKey, signIn } from "./helpers/farcaster";
import { standIn } from "./helpers/node";

/*
 * Parts of the app fetched the first time they're used (allthing-web's
 * `split`): the DAU dashboard's code comes with /dau, not before; the
 * composer's, React's and signing in's as they're about to be used; and
 * writing's only for a reader signed in.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const DAY = 86_400;
const now = Math.floor(Date.now() / 1000);
const today = Math.floor(now / DAY);
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const activity = {
  reached: now - 60, rolling_to: Math.floor((now - 60) / 3_600) * 3_600, rolling_24h: 6_174, previous_24h: 5_982,
  since: (today - 29) * DAY,
  days: Array.from({ length: 30 }, (_, i) => ({ day: today - 29 + i, active: 6_000 + i, complete: i < 29, actions: { cast: 100 } })),
  meta,
};

/**
 * The pages' code, which the app fetches ahead of use (`split::preload_pages`:
 * a post's a few seconds in, the others' as a link to them is pointed at),
 * the front page's poll's, which it fetches once the page is idle, and the
 * code parts share, which comes alongside: left out of the lists the tests
 * below compare.
 */
const ahead = (name: string) => name.startsWith("chunk_") || /^split_(post|user|channel|search|channels|poll)\.wasm$/.test(name);

/** The fixtures, and the WebAssembly the page fetches, by file name. */
async function serve(page: Page) {
  const modules: string[] = [];
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    if (path.endsWith(".wasm")) modules.push(path.split("/").pop()!.replace(/^allthing-[0-9a-f]+\.wasm$/, "app"));
  });
  await page.route("**/api/v1/**", (route) => route.fulfill({ json: { pool: [], meta } }));
  await page.route("**/api/v1/activity", (route) => route.fulfill({ json: activity }));
  return modules;
}

for (const width of [1280, 390]) {
  test(`the DAU dashboard's code is fetched on /dau, once, and not before, at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const modules = await serve(page);
    await page.goto("/");
    await expect(page.getByRole("combobox", { name: "Search casts, people and channels", exact: true })).toBeVisible();
    await page.waitForLoadState("networkidle");
    const own = () => modules.filter((name) => !ahead(name));
    expect(own()).toEqual(["app"]);

    await page.goto("/dau");
    await expect(page.locator(".dau-meter")).toContainText("6,174");
    expect(own()).toEqual(["app", "app", "split_dau.wasm"]);
    const fetched = own().length;
    // Away and back in the app: the code is already in.
    await page.getByRole("link", { name: "Allthing, home" }).click();
    await expect(page).toHaveURL(/\/$/);
    await page.goBack();
    await expect(page.locator(".dau-meter")).toContainText("6,174");
    expect(own()).toHaveLength(fetched);
    expect(errors).toEqual([]);
  });
}

test("a part that can't be fetched loads the page again once, then offers to try again", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await serve(page);
  // As when Allthing was deployed since the page was opened: the files of
  // the version it runs are gone.
  let blocked = true;
  await page.route("**/split_dau.wasm", (route) => blocked ? route.fulfill({ status: 404, contentType: "text/html", body: "<!doctype html>" }) : route.continue());
  let loads = 0;
  page.on("load", () => loads++);
  await page.goto("/dau");
  const alert = page.getByRole("alert");
  await expect(alert).toContainText("This part of Allthing couldn't load");
  expect(loads).toBe(2);
  await expect(page.locator(".dau-meter")).toHaveCount(0);

  blocked = false;
  await alert.getByRole("button", { name: "Try again" }).click();
  await expect(page.locator(".dau-meter")).toContainText("6,174");
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(loads).toBe(3);
  expect(errors).toEqual([]);
});

// The composer and React's Pepe picker: their code comes as they're about
// to be used, and the composer's text box is there at once, the first time
// too.

const alice = { fid: 42, username: "alice", display_name: "Alice" };
const post = { hash: `0x${"1".padStart(40, "0")}`, author: alice, text: "What should I read next?", ts: 1792235681, likes: 3, recasts: 0, replies: 0 };

/** A feed of one cast and its post page; the WebAssembly fetched past the app's own. */
async function serveFeed(page: Page) {
  const modules = await serve(page);
  await page.route("**/api/v1/home**", (route) => route.fulfill({ json: { pool: [post], notes: {}, meta } }));
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: { thread: { cast: post, children: [] }, notes: {}, meta } }));
  return () => modules.filter((name) => name !== "app" && !ahead(name));
}

for (const width of [1280, 390]) {
  test(`the composer's code comes as Cast is about to be used, not with the page, at width ${width}`, async ({ browser }, testInfo) => {
    const phone = width === 390;
    const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width, height: 844 }, isMobile: phone, hasTouch: phone });
    const page = await context.newPage();
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const parts = await serveFeed(page);
    await page.goto("/");
    await expect(page.locator(".cast-card").first()).toBeVisible();
    await page.waitForLoadState("networkidle");
    expect(parts()).toEqual([]);
    const cast = page.locator(phone ? ".cast-fab" : ".cast-pill");
    if (phone) {
      await cast.tap();
    } else {
      // Pointing at Cast fetches it, ahead of the click.
      await cast.hover();
      await expect.poll(parts).toContain("split_composer.wasm");
      await cast.click();
    }
    const composer = page.getByRole("dialog", { name: "New cast" });
    await expect(composer.getByRole("textbox")).toBeFocused();
    await page.keyboard.type("gm");
    // On a phone, the draft goes to the Farcaster app by a link.
    const post = composer.getByRole(phone ? "link" : "button", { name: "Post in Farcaster" });
    await expect(post).toBeVisible();
    await expect(composer.getByRole("button", { name: "Add an emoji" })).toBeVisible();
    expect(parts()).toContain("split_composer.wasm");
    await page.screenshot({ path: testInfo.outputPath(`composer-${width}.png`) });
    // Closed and opened again: the code is in, nothing more is fetched.
    const fetched = parts().length;
    await page.keyboard.press("Escape");
    await (phone ? cast.tap() : cast.click());
    await expectDraft(composer.getByRole("textbox"), "gm");
    await expect(post).toBeVisible();
    expect(parts()).toHaveLength(fetched);
    expect(errors).toEqual([]);
    await context.close();
  });
}

test("the first time, the composer's text box takes typing while the rest of it comes", async ({ page }, testInfo) => {
  await serveFeed(page);
  let release = () => {};
  const held = new Promise<void>((resolve) => { release = resolve; });
  await page.route("**/split_composer.wasm", async (route) => { await held; await route.continue(); });
  await page.goto("/");
  await page.getByRole("button", { name: "Write a cast" }).click();
  const composer = page.getByRole("dialog", { name: "New cast" });
  await expect(composer.getByRole("textbox")).toBeFocused();
  await page.keyboard.type("hello there");
  await expectDraft(composer.getByRole("textbox"), "hello there");
  // The bar says it's coming, as tall as the bar it makes way for.
  const bar = composer.locator(".compose-foot");
  await expect(bar).toContainText("Loading…");
  const pending = (await bar.boundingBox())!.height;
  await page.screenshot({ path: testInfo.outputPath("composer-pending-1280.png") });
  release();
  await expect(composer.getByRole("button", { name: "Post in Farcaster" })).toBeVisible();
  await expect(bar).not.toContainText("Loading…");
  expect(Math.abs((await bar.boundingBox())!.height - pending)).toBeLessThanOrEqual(2);
  await expectDraft(composer.getByRole("textbox"), "hello there");
});

test("a composer whose code can't be fetched says so, keeping the draft, and doesn't reload by itself", async ({ page }) => {
  await serveFeed(page);
  await page.route("**/split_composer.wasm", (route) => route.fulfill({ status: 404, contentType: "text/html", body: "<!doctype html>" }));
  let loads = 0;
  page.on("load", () => loads++);
  await page.goto("/");
  await page.getByRole("button", { name: "Write a cast" }).click();
  const composer = page.getByRole("dialog", { name: "New cast" });
  await page.keyboard.type("keep me");
  await expect(composer.getByRole("alert")).toHaveText("This part of Allthing couldn't load. Reload");
  await expectDraft(composer.getByRole("textbox"), "keep me");
  expect(loads).toBe(1);
});

test("React's Pepe picker comes as React is about to be used", async ({ page }) => {
  const parts = await serveFeed(page);
  await page.goto(`/p/${post.hash}`);
  const react = page.locator(".post").getByRole("button", { name: "React to @alice's cast with an emoji" });
  await expect(react).toBeVisible();
  await page.waitForLoadState("networkidle");
  expect(parts()).toEqual([]);
  await react.hover();
  await expect.poll(parts).toContain("split_react.wasm");
  await react.click();
  const panel = page.getByRole("dialog", { name: "React with a Pepe" });
  await expect(panel.getByRole("searchbox")).toBeVisible();
});

test("the sign-in's code comes as Sign in is about to be used, not with the page", async ({ page }) => {
  const parts = await serveFeed(page);
  await page.goto("/");
  await expect(page.locator(".cast-card").first()).toBeVisible();
  await page.waitForLoadState("networkidle");
  expect(parts()).toEqual([]);
  await page.getByRole("button", { name: "Sign in with Farcaster" }).hover();
  await expect.poll(parts).toContain("split_signin.wasm");
});

test("/scheduled fetches its list's code with the page", async ({ page }) => {
  const parts = await serveFeed(page);
  await page.goto("/scheduled");
  // Signed out, the list asks to sign in first.
  await expect(page.getByText("Sign in to see your scheduled casts.")).toBeVisible();
  expect(parts()).toContain("split_scheduled.wasm");
  expect(parts()).not.toContain("split_signin.wasm");
});

// Writing: signing and sending, approving a key and reading the one kept,
// and looking on the node for what was handed to the Farcaster app. A
// signed-out reader's page never fetches its code; a signed-in reader's
// fetches it as soon as it knows who they are.

const reader = { fid: 1234, username: "reader", display_name: "Reader" };
const NODE = "https://haatz.quilibrium.com/v1";
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "authorization, content-type" };

/** Writes set up on the Worker, and a node that takes every message, and shows it read back. */
async function serveWrites(page: Page) {
  const { sent } = await standIn(page, { fid: reader.fid });
  await page.route("**/api/v1/signer", (route) => route.fulfill({ json: { app_fid: 3356318 } }));
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  return sent;
}

test("a signed-out reader's page fetches no writing code, and a like goes to the Farcaster app", async ({ page }) => {
  const parts = await serveFeed(page);
  await page.goto("/");
  const like = page.locator(".cast-card").first().getByRole("button", { name: "Like", exact: true });
  await expect(like).toBeVisible();
  await like.click();
  await expect(page.getByRole("dialog", { name: "Like in Farcaster" })).toBeVisible();
  await page.waitForLoadState("networkidle");
  expect(parts()).not.toContain("split_writer.wasm");
});

test("a signed-in reader's page fetches writing's code as soon as it knows them, and a like goes out with it", async ({ page }) => {
  const parts = await serveFeed(page);
  const sent = await serveWrites(page);
  await signIn(page, reader);
  await page.goto("/");
  await approvedKey(page, reader.fid, 600_000);
  await page.reload();
  await expect.poll(parts).toContain("split_writer.wasm");
  const like = page.locator(".cast-card").first().getByRole("button", { name: "Like", exact: true });
  await expect(like).toHaveAttribute("aria-pressed", "false");
  await like.click();
  await expect(like).toHaveAttribute("aria-pressed", "true");
  await expect.poll(() => sent.length).toBe(1);
  await expect(page.getByRole("dialog", { name: "Like in Farcaster" })).toHaveCount(0);
});

test("with writes off, a signed-in reader's page fetches writing's code before any write, to look for hand-offs", async ({ page }) => {
  // No /api/v1/signer here: every write goes to the Farcaster app, and
  // writing's code looks on the node for it from the first hand-off on.
  const parts = await serveFeed(page);
  await signIn(page, reader);
  await page.goto("/");
  await expect(page.locator(".cast-card").first()).toBeVisible();
  await expect.poll(parts).toContain("split_writer.wasm");
});

test("when writing's code can't be fetched, a signed-in reader's like goes to the Farcaster app", async ({ page }) => {
  await serveFeed(page);
  const sent = await serveWrites(page);
  await page.route("**/split_writer.wasm", (route) => route.fulfill({ status: 404, contentType: "text/html", body: "<!doctype html>" }));
  await signIn(page, reader);
  await page.goto("/");
  await approvedKey(page, reader.fid, 600_000);
  await page.reload();
  const like = page.locator(".cast-card").first().getByRole("button", { name: "Like", exact: true });
  await expect(like).toHaveAttribute("aria-haspopup", "dialog");
  await like.click();
  await expect(page.getByRole("dialog", { name: "Like in Farcaster" })).toBeVisible();
  expect(sent).toEqual([]);
});

// Pages of their own: a post, a profile, a channel, search and the
// channels. A post's code comes a few seconds after the app starts, and as
// a card is pointed at; the others' as a link to them is pointed at,
// touched or focused, and search's as the search box is; and a page's with
// the app when a visit starts on it.

test("a post's code comes a few seconds after the app starts, the other pages' only as they're about to be used", async ({ page }) => {
  const modules = await serve(page);
  await page.route("**/api/v1/home**", (route) => route.fulfill({ json: { pool: [post], notes: {}, meta } }));
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: { thread: { cast: post, children: [] }, notes: {}, meta } }));
  await page.goto("/");
  await expect(page.locator(".cast-card").first()).toBeVisible();
  await expect.poll(() => modules, { timeout: 15_000 }).toContain("split_post.wasm");
  await page.waitForTimeout(500);
  for (const part of ["split_user.wasm", "split_channel.wasm", "split_search.wasm", "split_channels.wasm"]) expect(modules).not.toContain(part);
  // Pointing at a profile's link, focusing the search box.
  await page.locator('.cast-card a[href="/u/alice"]').first().hover();
  await expect.poll(() => modules).toContain("split_user.wasm");
  await page.getByRole("combobox", { name: "Search casts, people and channels", exact: true }).focus();
  await expect.poll(() => modules).toContain("split_search.wasm");
  // A post opened now fetches nothing more.
  await page.waitForLoadState("networkidle");
  const fetched = modules.length;
  await page.locator(".cast-card").first().getByText(post.text).click();
  await expect(page).toHaveURL(new RegExp(`/p/${post.hash}$`));
  await expect(page.locator(".post")).toContainText(post.text);
  expect(modules).toHaveLength(fetched);
});

test("a visit that starts on a post fetches the post's code with the app, and uses what it fetched", async ({ page }) => {
  const notices: string[] = [];
  page.on("console", (message) => {
    if (/preload/i.test(message.text())) notices.push(message.text());
  });
  const modules = await serve(page);
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: { thread: { cast: post, children: [] }, notes: {}, meta } }));
  await page.goto(`/p/${post.hash}`);
  await expect(page.locator(".post")).toContainText(post.text);
  // index.html asked for it before the app did.
  await expect(page.locator('head link[rel="preload"][href$="/split_post.wasm"]')).toHaveCount(1);
  expect(modules).toContain("split_post.wasm");
  // The browser says so when a preload goes unused, or isn't what the
  // loader asked for, after a few seconds.
  await page.waitForTimeout(3_500);
  expect(notices).toEqual([]);
});
