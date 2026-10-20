// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

/*
 * Parts of the app fetched the first time they're used (allthing-web's
 * `split`): the DAU dashboard's code comes with /dau, not before.
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
    expect(modules).toEqual(["app"]);

    await page.goto("/dau");
    await expect(page.locator(".dau-meter")).toContainText("6,174");
    // Code the parts share (chunk_*) comes alongside, in parallel.
    const own = () => modules.filter((name) => !name.startsWith("chunk_"));
    expect(own()).toEqual(["app", "app", "split_dau.wasm"]);
    const fetched = modules.length;
    // Away and back in the app: the code is already in.
    await page.getByRole("link", { name: "Allthing, home" }).click();
    await expect(page).toHaveURL(/\/$/);
    await page.goBack();
    await expect(page.locator(".dau-meter")).toContainText("6,174");
    expect(modules).toHaveLength(fetched);
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
  return () => modules.filter((name) => name !== "app");
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
    await expect(composer.getByRole("textbox")).toHaveValue("gm");
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
  await expect(composer.getByRole("textbox")).toHaveValue("hello there");
  // The bar says it's coming, as tall as the bar it makes way for.
  const bar = composer.locator(".compose-foot");
  await expect(bar).toContainText("Loading…");
  const pending = (await bar.boundingBox())!.height;
  await page.screenshot({ path: testInfo.outputPath("composer-pending-1280.png") });
  release();
  await expect(composer.getByRole("button", { name: "Post in Farcaster" })).toBeVisible();
  await expect(bar).not.toContainText("Loading…");
  expect(Math.abs((await bar.boundingBox())!.height - pending)).toBeLessThanOrEqual(2);
  await expect(composer.getByRole("textbox")).toHaveValue("hello there");
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
  await expect(composer.getByRole("textbox")).toHaveValue("keep me");
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
