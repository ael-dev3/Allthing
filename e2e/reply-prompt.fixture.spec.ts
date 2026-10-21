// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";
import { signIn } from "./helpers/farcaster";

/*
 * Under a post, a reader signed in finds "Cast your reply", as under a cast
 * in the Farcaster app's conversations: their picture, the faint words and
 * a dimmed Reply, in a row 52 px tall. A click, a tap, Enter or Space opens
 * Reply for that cast. It is there in the frame the post first shows, so
 * nothing moves when it comes; signed out, there is none.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const now = Math.floor(Date.now() / 1000);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const pfp = (name: string) => `https://imagedelivery.net/fixture/${name}/original`;
const alice = { fid: 42, username: "alice", display_name: "Alice", pfp_url: pfp("alice") };
const bob = { fid: 7, username: "bob", display_name: "Bob", pfp_url: pfp("bob") };
const me = { fid: 1234, username: "reader", display_name: "Reader", pfp_url: pfp("reader") };
const post = { hash: hex(1), author: alice, text: "What should I read next?", ts: now - 3600, likes: 3, replies: 1 };
const reply = { hash: hex(2), author: bob, text: "Hyperion", ts: now - 600, likes: 0, replies: 0, parent_hash: post.hash };
const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64");

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => route.fulfill({ json: { pool: [post], notes: {}, meta } }));
  await page.route("**/api/v1/cast?*", (route) =>
    route.fulfill({ json: { thread: { cast: post, children: [{ cast: reply, children: [] }] }, notes: {}, meta } }),
  );
  await page.route("https://imagedelivery.net/**", (route) => route.fulfill({ contentType: "image/png", body: png }));
  await page.route("https://haatz.quilibrium.com/**", (route) => route.fulfill({ headers: { "access-control-allow-origin": "*" }, json: { messages: [] } }));
}

/** Notes, from the page's start, whether the row was there in the frame the post first showed. */
async function watchFirstFrame(page: Page) {
  await page.addInitScript(() => {
    const w = window as unknown as { __firstPost?: { prompt: boolean } };
    const check = () => {
      if (w.__firstPost || !document.querySelector(".post:not(.skeleton)")) return;
      w.__firstPost = { prompt: Boolean(document.querySelector(".reply-prompt")) };
    };
    new MutationObserver(check).observe(document, { childList: true, subtree: true });
  });
}

/** Every animation that ends (not a skeleton's shimmer), ended: the row rises in with the post. */
const settled = (page: Page) =>
  page.evaluate(() =>
    Promise.all(
      document
        .getAnimations()
        .filter((a) => a.effect?.getComputedTiming().iterations !== Infinity)
        .map((a) => a.finished.catch(() => {})),
    ),
  );

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  return { context, page: await context.newPage() };
}

for (const width of [1280, 390] as const) {
  test(`at ${width}, signed in, "Cast your reply" sits under the post from its first frame, and opens Reply`, async ({ browser }, testInfo) => {
    const touch = width === 390;
    const { context, page } = touch
      ? await phone(browser, testInfo)
      : await (async () => {
          const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 1280, height: 900 } });
          return { context, page: await context.newPage() };
        })();
    await signIn(page, me);
    await serve(page);
    await watchFirstFrame(page);
    await page.goto(`/p/${post.hash}`);
    const prompt = page.getByRole("button", { name: "Cast your reply" });
    await expect(prompt).toBeVisible();
    expect(await page.evaluate(() => (window as unknown as { __firstPost?: { prompt: boolean } }).__firstPost)).toEqual({ prompt: true });
    await expect(prompt).toHaveAttribute("aria-haspopup", "dialog");
    await expect(prompt.locator(".avatar")).toHaveAttribute("src", /fixture\/reader\//);
    await expect(prompt.locator(".reply-prompt-text")).toHaveText("Cast your reply");
    await expect(prompt.locator(".reply-prompt-pill")).toHaveText("Reply");
    // 52 px, as wide as the post, between it and its replies.
    await expect(page.locator(".comment")).toHaveCount(1);
    await settled(page);
    const laid = await page.evaluate(() => {
      const box = (s: string) => document.querySelector(s)!.getBoundingClientRect();
      const [row, card, replies, avatar] = [box(".reply-prompt"), box(".post"), box(".replies"), box(".reply-prompt .avatar")];
      return { height: row.height, left: row.left - card.left, right: card.right - row.right, below: row.top - card.bottom, above: replies.top - row.bottom, avatar: [avatar.width, avatar.height] };
    });
    expect(Math.round(laid.height)).toBe(52);
    expect([Math.round(laid.left), Math.round(laid.right)]).toEqual([0, 0]);
    expect(laid.below).toBeGreaterThan(0);
    expect(laid.above).toBeGreaterThan(0);
    expect(laid.avatar.map(Math.round)).toEqual([32, 32]);
    // On a phone, the screen's width but its gutters.
    if (touch) expect((await prompt.boundingBox())!.width).toBeGreaterThanOrEqual(390 - 2 * 16 - 1);
    await page.screenshot({ path: testInfo.outputPath(`prompt-${width}.png`) });
    // A press opens Reply for the post, its box focused.
    await (touch ? prompt.tap() : prompt.click());
    const dialog = page.getByRole("dialog", { name: "Reply" });
    await expect(dialog.locator(".reply-parent-handle")).toHaveText("@alice");
    await expect(dialog.getByRole("textbox", { name: "Cast your reply" })).toBeFocused();
    await page.screenshot({ path: testInfo.outputPath(`prompt-opened-${width}.png`) });
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    await expect(prompt).toBeFocused();
    await context.close();
  });
}

test("Enter or Space on it opens Reply, as a click does", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await signIn(page, me);
  await serve(page);
  await page.goto(`/p/${post.hash}`);
  const prompt = page.getByRole("button", { name: "Cast your reply" });
  const dialog = page.getByRole("dialog", { name: "Reply" });
  for (const key of ["Enter", " "]) {
    await prompt.focus();
    await page.keyboard.press(key);
    await expect(dialog.getByRole("textbox", { name: "Cast your reply" })).toBeFocused();
    await expect(dialog.locator(".reply-parent-text")).toHaveText("What should I read next?");
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
  }
});

test("opened from the feed, it's there with the post at once, and stays put as the thread comes", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await signIn(page, me);
  await serve(page);
  // The thread waits until the post shown from the feed has been measured.
  let release = () => {};
  const held = new Promise<void>((resolve) => (release = resolve));
  await page.route("**/api/v1/cast?*", async (route) => {
    await held;
    await route.fulfill({ json: { thread: { cast: post, children: [{ cast: reply, children: [] }] }, notes: {}, meta } });
  });
  await page.goto("/");
  await page.locator(".cast-card .cast-text").first().click();
  const prompt = page.getByRole("button", { name: "Cast your reply" });
  await expect(page.locator(".replies[aria-busy=true]")).toBeVisible();
  await expect(prompt).toBeVisible();
  await settled(page);
  const before = (await prompt.boundingBox())!;
  release();
  await expect(page.locator(".comment")).toHaveCount(1);
  await settled(page);
  const after = (await prompt.boundingBox())!;
  expect([Math.round(after.x), Math.round(after.y), after.height]).toEqual([Math.round(before.x), Math.round(before.y), 52]);
});

test("signed out, there's no row: Reply is under the post as before", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await serve(page);
  await page.goto(`/p/${post.hash}`);
  await expect(page.locator(".comment")).toHaveCount(1);
  await expect(page.locator(".reply-prompt")).toHaveCount(0);
  await expect(page.locator(".post").getByRole("button", { name: "Reply to @alice" })).toBeVisible();
});
