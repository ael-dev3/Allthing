// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page, type TestInfo } from "@playwright/test";
import { NOW } from "./fixture-time";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = (NOW - 19);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const parsa = { fid: 9, username: "parsa", display_name: "parsa" };
const bob = { fid: 10, username: "bob", display_name: "Bob" };
const cast = { hash: hex(1), author: alice, text: "A cast with replies", ts, likes: 0, replies: 3 };
const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64");
// parsa's reply is an image with no text; bob's is a video with no text.
const replies = [
  { hash: hex(2), author: parsa, text: "", ts: ts - 60, likes: 3, parent_hash: cast.hash, embeds: [{ kind: "image", url: "https://imagedelivery.net/fixture/apples/original" }] },
  { hash: hex(3), author: bob, text: "", ts: ts - 120, likes: 2, parent_hash: cast.hash, embeds: [{ kind: "video", url: "https://stream.example/clip.m3u8" }] },
  { hash: hex(4), author: bob, text: "Morning parsa", ts: ts - 180, likes: 1, parent_hash: cast.hash },
];
const users = {
  "9": { bio: "Apples every morning.", followers: 4321, following: 12, registered_at: 1759222921 },
  "10": { bio: "Replies a lot.", followers: 5, following: 1 },
};

async function serve(page: Page) {
  const requests = { user: 0 };
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [cast], notes: {}, meta } });
  });
  // The preview reads the post's payload that hovering prefetched, or the replies on their own.
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
    thread: { cast, children: replies.map((reply) => ({ cast: reply })) }, notes: { users }, meta,
  } }));
  await page.route("**/api/v1/replies?*", (route) => route.fulfill({ json: { hash: cast.hash, replies, notes: { users }, meta } }));
  await page.route("**/api/v1/vote?*", (route) => route.fulfill({ json: { fid: 9, onchain: "stay", meta } }));
  await page.route("**/api/v1/wallets?*", (route) => route.fulfill({ json: { fid: 9, addresses: [], chains: [], total: 0, sources: [], fetched_at: Date.now() } }));
  await page.route("**/api/v1/user?*", (route) => {
    requests.user += 1;
    return route.fulfill({ status: 404, json: { error: "Not read in this test" } });
  });
  await page.route("https://imagedelivery.net/**", (route) => route.fulfill({ contentType: "image/png", body: png }));
  return requests;
}

test("a replies preview links its authors to their cards, and shows what a reply with no text holds", async ({ page }, testInfo: TestInfo) => {
  test.skip(Boolean(testInfo.project.use.hasTouch), "hover needs a mouse");
  await page.setViewportSize({ width: 1280, height: 900 });
  const requests = await serve(page);
  await page.goto("/");
  await page.locator(".cast-card:not(.skeleton)").first().locator(".stat-replies").hover();
  const panel = page.getByRole("dialog", { name: "Replies", exact: true });
  const rows = panel.locator(".reply-items > li");
  await expect(rows).toHaveCount(3);
  // parsa's image reply: its thumbnail loads, and no empty text line.
  const first = rows.nth(0);
  await expect(first.locator(".reply-text")).toHaveCount(0);
  await expect.poll(() => first.locator(".reply-thumbs img").evaluate((img: HTMLImageElement) => img.naturalWidth)).toBeGreaterThan(0);
  expect((await first.boundingBox())!.height).toBeLessThan(110);
  // bob's video reply says it is a video; the third shows its text.
  await expect(rows.nth(1).locator(".reply-label")).toHaveText("Video");
  await expect(rows.nth(2).locator(".reply-text")).toHaveText("Morning parsa");
  // The name is a link to the profile, and hovering it opens the complete card beside the preview.
  const name = first.locator(".reply-author-link");
  await expect(name).toHaveAttribute("href", "/u/parsa");
  await expect(first.locator("a.reply-item")).toHaveAttribute("href", `/p/${replies[0].hash}`);
  await name.hover();
  const card = page.locator(".popover-panel.profile-card");
  await expect(card).toBeVisible();
  await expect(card.locator(".profile-card-bio")).toHaveText("Apples every morning.");
  await expect(card.locator(".profile-card-counts")).toContainText("4,321 followers");
  await expect(card.locator(".fact", { hasText: "Joined" }).locator("dd")).toHaveText("30 Sep 2025");
  const [list, beside] = [await panel.boundingBox(), await card.boundingBox()];
  expect(beside!.x >= list!.x + list!.width || beside!.x + beside!.width <= list!.x).toBe(true);
  await expect(panel).toBeVisible();
  expect(requests.user).toBe(0);
});

test("on a phone, a tap on a reply's author opens their card and keeps the preview", async ({ browser }, testInfo: TestInfo) => {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true,
  });
  const page = await context.newPage();
  await serve(page);
  await page.goto("/");
  await page.locator(".cast-card:not(.skeleton)").first().locator(".stat-replies").tap();
  const panel = page.getByRole("dialog", { name: "Replies", exact: true });
  await expect(panel.locator(".reply-items > li")).toHaveCount(3);
  await expect.poll(() => panel.locator(".reply-thumbs img").first().evaluate((img: HTMLImageElement) => img.naturalWidth)).toBeGreaterThan(0);
  await panel.locator(".reply-author-link").first().tap();
  const card = page.locator(".popover-panel.profile-card");
  await expect(card.locator(".profile-card-bio")).toHaveText("Apples every morning.");
  await expect(card).toBeInViewport({ ratio: 1 });
  await expect(panel).toBeVisible();
  await expect(page).toHaveURL(/\/$/);
  await context.close();
});


for (const width of [390, 1280]) {
  test(`a tall replies preview stays below the actual header at ${width}px`, async ({ browser }, testInfo) => {
    const context = await browser.newContext({
      baseURL: testInfo.project.use.baseURL, viewport: { width, height: 844 }, isMobile: width === 390, hasTouch: true,
    });
    const page = await context.newPage();
    await serve(page);
    const many = Array.from({ length: 10 }, (_, i) => ({
      hash: hex(100 + i), author: bob, text: `Reply ${i}: a full row of text for the tall preview.`, ts: ts - i, parent_hash: cast.hash,
    }));
    await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
      thread: { cast, children: many.map((reply) => ({ cast: reply })) }, notes: { users }, meta,
    } }));
    await page.route("**/api/v1/replies?*", (route) => route.fulfill({ json: { hash: cast.hash, replies: many, notes: { users }, meta } }));
    await page.goto("/");
    await expect(page.locator("#boot")).toBeHidden();
    const button = page.locator("#app .cast-card:not(.skeleton) .stat-replies").first();
    await expect(button).toBeVisible();
    // Place a real trigger where the old 72px allowance let a full panel
    // overlap the phone's two-row header. The actual header is unchanged.
    await page.addStyleTag({ content: ".page { min-height: 2000px } .cast-card { margin-top: 400px }" });
    await button.evaluate((el) => scrollBy(0, el.getBoundingClientRect().top - 454));
    await expect.poll(() => button.evaluate((el) => Math.round(el.getBoundingClientRect().top))).toBe(454);
    await button.tap();
    const panel = page.getByRole("dialog", { name: "Replies", exact: true });
    await expect(panel.locator(".reply-items > li")).toHaveCount(10);
    await panel.evaluate((el) => Promise.all(el.getAnimations().map((animation) => animation.finished)));
    const header = await page.locator(".site-header").boundingBox();
    const box = await panel.boundingBox();
    expect(header && box).toBeTruthy();
    expect(box!.y).toBeGreaterThanOrEqual(header!.y + header!.height - 1);
    expect(box!.y + box!.height).toBeLessThanOrEqual(845);
    await button.tap();
    await expect(panel).toHaveCount(0);
    await context.close();
  });
}
