// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";
import { appCame } from "./helpers/app";
import { approvedKey, signIn } from "./helpers/farcaster";

/*
 * The pack's emoji in excerpts of a cast. The Replies preview, the casts a
 * post's reply answers, search suggestions, what the composer shows of the cast a
 * reply answers and the one a quote quotes, the
 * media viewer's caption and the scheduled list show ":pepe_name:" as the
 * emoji, as cast text does, with nothing in them linked; and an excerpt is
 * never cut inside a shortcode.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const bob = { fid: 7, username: "bob", display_name: "Bob" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const NODE = "https://haatz.quilibrium.com/v1";
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" };
const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64");
const post = {
  hash: hex(1), author: alice, text: "Who's up? :pepe_gm:", ts, likes: 2, replies: 2,
  embeds: [{ kind: "image", url: "https://imagedelivery.net/fixture/sunrise/original" }],
};
const replies = [
  { hash: hex(2), author: bob, text: "Up :pepe_wave: ask /Base or @carol", ts: ts - 60, likes: 1, replies: 0, parent_hash: post.hash },
  // Before, 220 characters of this cut its twentieth shortcode in two.
  { hash: hex(3), author: bob, text: `so good ${":pepe_clap:".repeat(25)}`, ts: ts - 120, likes: 0, replies: 0, parent_hash: post.hash },
];
// Longer than a suggestion's 90 characters' room, without a space to cut at.
const long = { hash: hex(4), author: alice, text: `gm ${":pepe_gm:".repeat(40)}`, ts: ts - 180, likes: 0, replies: 0 };

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [post], notes: {}, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => {
    const reply = replies.find((r) => r.hash === new URL(route.request().url()).searchParams.get("hash"));
    return route.fulfill({
      json: reply
        ? { thread: { cast: reply, children: [] }, ancestors: [post], notes: {}, meta }
        : { thread: { cast: post, children: replies.map((cast) => ({ cast, children: [] })) }, notes: {}, meta },
    });
  });
  await page.route("**/api/v1/replies?*", (route) => route.fulfill({ json: { hash: post.hash, replies, notes: {}, meta } }));
  await page.route("**/api/v1/search?*", (route) => {
    const query = new URL(route.request().url()).searchParams.get("q");
    return route.fulfill({ json: { query, users: [], channels: [], casts: [post, long], meta } });
  });
  await page.route("**/api/v1/signer", (route) => route.fulfill({ json: { app_fid: 9152 } }));
  await page.route(`${NODE}/**`, (route) => route.fulfill({ status: 404, headers: cors, json: {} }));
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  await page.route("https://imagedelivery.net/**", (route) => route.fulfill({ contentType: "image/png", body: png }));
}

/** Signed in, with a key approved an hour ago, on `path`; `more` serves what else it needs. */
async function ready(page: Page, path: string, more?: () => Promise<void>) {
  await signIn(page, me);
  await serve(page);
  await more?.();
  await page.goto(path);
  await expect(page.locator(".post, .scheduled-head").first()).toBeVisible();
  await approvedKey(page, me.fid, 3_600_000);
  await page.reload();
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  return { context, page: await context.newPage() };
}

/**
 * What an excerpt shows: its emoji once loaded, and whether each shows
 * whole, its top and bottom not cut by a box that clips (a one-line
 * excerpt's, a popover's), though a one-line excerpt's end may leave some
 * out; its words without them; and how many links are in it.
 */
async function shows(excerpt: Locator) {
  await expect(excerpt).toBeVisible();
  return excerpt.evaluate(async (el) => {
    const images = [...el.querySelectorAll("img")];
    await Promise.all(images.map((img) => img.decode().catch(() => null)));
    const whole = (img: HTMLImageElement) => {
      const r = img.getBoundingClientRect();
      for (let box = img.parentElement; box && box !== document.body; box = box.parentElement) {
        const style = getComputedStyle(box);
        if (style.overflowX === "visible" && style.overflowY === "visible") continue;
        const b = box.getBoundingClientRect();
        if (r.top < b.top - 0.5 || r.bottom > b.bottom + 0.5) return false;
      }
      return true;
    };
    return {
      emoji: images.map((img) => ({ alt: img.alt, loaded: img.naturalWidth > 0, whole: whole(img) })),
      text: el.textContent,
      links: el.querySelectorAll("a").length,
    };
  });
}

const one = (alt: string) => [{ alt, loaded: true, whole: true }];

test("the Replies preview shows a reply's emoji, with nothing in it linked, and a run of them whole", async ({ page }, testInfo) => {
  await serve(page);
  await page.goto("/");
  await appCame(page);
  await page.locator(".cast-card:not(.skeleton)").first().locator(".stat-replies").hover();
  const panel = page.getByRole("dialog", { name: "Replies", exact: true });
  const rows = panel.locator(".reply-items > li");
  await expect(rows).toHaveCount(2);
  expect(await shows(rows.nth(0).locator(".reply-text"))).toEqual({ emoji: one(":pepe_wave:"), text: "Up  ask /Base or @carol", links: 0 });
  // Each emoji counts as about its room, three characters, so all 25 fit.
  const run = await shows(rows.nth(1).locator(".reply-text"));
  expect(run.text).toBe("so good ");
  expect(run.emoji).toEqual(Array(25).fill(one(":pepe_clap:")[0]));
  await page.screenshot({ path: testInfo.outputPath("replies-preview-1280.png") });
});

test("the cast a post's reply answers shows its emoji, as a card", async ({ page }, testInfo) => {
  await serve(page);
  await page.goto(`/p/${replies[0].hash}`);
  expect((await shows(page.locator(".thread-chain .cast-card .cast-text"))).emoji).toEqual(one(":pepe_gm:"));
  await page.screenshot({ path: testInfo.outputPath("thread-chain.png") });
});

test("search suggestions show a post's emoji, and cut a long one between them", async ({ page }, testInfo) => {
  await serve(page);
  await page.goto("/");
  await appCame(page);
  await page.getByRole("combobox", { name: "Search casts, people and channels", exact: true }).fill("pepe");
  const titles = page.getByRole("listbox", { name: "Suggestions" }).getByRole("group", { name: "Posts" }).locator(".suggestion-title");
  await expect(titles).toHaveCount(2);
  expect(await shows(titles.nth(0))).toEqual({ emoji: one(":pepe_gm:"), text: "Who's up? ", links: 0 });
  // 90 characters' room: "gm " and 29 emoji, then "…". Before, the cut
  // fell inside the tenth shortcode, which showed as ":pepe_…".
  const cut = await shows(titles.nth(1));
  expect(cut.text).toBe("gm …");
  expect(cut.emoji).toEqual(Array(29).fill(one(":pepe_gm:")[0]));
  await page.screenshot({ path: testInfo.outputPath("search-suggestions.png") });
});

test("the composer's reply and quote show the cast's emoji", async ({ page }, testInfo) => {
  await ready(page, `/p/${post.hash}`);
  await page.getByRole("button", { name: "Reply to @alice" }).click();
  const replying = page.getByRole("dialog", { name: "Reply" });
  expect(await shows(replying.locator(".reply-parent-text"))).toEqual({ emoji: one(":pepe_gm:"), text: "Who's up? ", links: 0 });
  await page.screenshot({ path: testInfo.outputPath("composer-reply.png") });
  await page.keyboard.press("Escape");
  await expect(replying).toBeHidden();
  await page.getByRole("button", { name: "Quote @bob's cast" }).first().click();
  const quoting = page.getByRole("dialog", { name: "Quote" });
  expect(await shows(quoting.locator(".compose-excerpt"))).toEqual({ emoji: one(":pepe_wave:"), text: "Up  ask /Base or @carol", links: 0 });
});

test("the media viewer's caption shows the cast's emoji", async ({ page }, testInfo) => {
  await serve(page);
  await page.goto(`/p/${post.hash}`);
  await page.locator(".post .tile-image").click();
  const viewer = page.getByRole("dialog", { name: "Media viewer", exact: true });
  expect(await shows(viewer.locator(".lb-text"))).toEqual({ emoji: one(":pepe_gm:"), text: "@alice · Who's up? ", links: 0 });
  await page.screenshot({ path: testInfo.outputPath("media-viewer.png") });
});

test("the scheduled list shows a draft's emoji, on its own lines", async ({ page }, testInfo) => {
  const at = Math.floor(Date.now() / 1000) + 3600;
  const draft = JSON.stringify({ text: "Later :pepe_gm:\nand :PEPE_WAVE: again" });
  await ready(page, "/scheduled", () =>
    page.route("**/api/v1/scheduled", (route) => route.fulfill({ json: { casts: [{ id: 1, at, hash: hex(9), state: "waiting", draft }] } })),
  );
  const text = page.locator(".scheduled-item .scheduled-text");
  expect(await shows(text)).toEqual({
    // In any case, as in cast text; its alt text is the shortcode as the pack has it.
    emoji: [...one(":pepe_gm:"), ...one(":pepe_wave:")],
    text: "Later \nand  again",
    links: 0,
  });
  // Two lines, as written.
  const [first, second] = await text.locator("img").evaluateAll((imgs) => imgs.map((img) => img.getBoundingClientRect().top));
  expect(second - first).toBeGreaterThan(15);
  await page.screenshot({ path: testInfo.outputPath("scheduled.png") });
});

test("at 390, the emoji fit the preview, the reply's context and the composer", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await signIn(page, me);
  await serve(page);
  await page.goto("/");
  await appCame(page);
  await page.locator(".cast-card:not(.skeleton)").first().locator(".stat-replies").tap();
  const panel = page.getByRole("dialog", { name: "Replies", exact: true });
  expect((await shows(panel.locator(".reply-text").first())).emoji).toEqual(one(":pepe_wave:"));
  const run = await shows(panel.locator(".reply-text").nth(1));
  expect(run.emoji).toEqual(Array(25).fill(one(":pepe_clap:")[0]));
  await page.screenshot({ path: testInfo.outputPath("replies-preview-390.png") });
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);

  await page.goto(`/p/${replies[0].hash}`);
  expect((await shows(page.locator(".thread-chain .cast-text"))).emoji).toEqual(one(":pepe_gm:"));
  await approvedKey(page, me.fid, 3_600_000);
  await page.reload();
  await page.getByRole("button", { name: "Reply to @bob" }).click();
  const replying = page.getByRole("dialog", { name: "Reply" });
  expect(await shows(replying.locator(".reply-parent-text"))).toEqual({ emoji: one(":pepe_wave:"), text: "Up  ask /Base or @carol", links: 0 });
  await page.screenshot({ path: testInfo.outputPath("composer-reply-390.png") });
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await context.close();
});
