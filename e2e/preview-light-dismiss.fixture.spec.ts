// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";
import { approvedKey, signIn } from "./helpers/farcaster";
import { NOW } from "./fixture-time";

/*
 * A panel a tap opened is dismissed lightly: the first tap outside it only
 * closes it. On a phone, a tap on the card around an open likes, recasts or
 * replies preview, or a profile card, closes the panel and nothing else: the
 * post under the tap opens on the next tap, not this one. The same goes for
 * a profile card opened from a preview's list, which leaves the list open.
 * A dialog opened from a preview keeps its own taps. On a computer, a
 * preview the pointer opened closes as the pointer leaves, and a click
 * elsewhere does what it does; React's picker, which a click opens, takes
 * the first click outside it, as a tapped panel does.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = (NOW - 19);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const dave = { fid: 7, username: "dave", display_name: "Dave" };
const erin = { fid: 8, username: "erin", display_name: "Erin" };
const cast = {
  hash: hex(1), author: alice, ts, likes: 2, recasts: 2, replies: 2,
  text: "A cast with likes, recasts and replies, and enough words to tap beside its counts' panels.",
};
const others = [2, 5, 6].map((n, i) => ({
  hash: hex(n), author: dave, ts: ts - 60 * (i + 1), likes: 0, recasts: 0, replies: 0,
  text: `Another cast further down (${i + 1}), with a few lines of words to tap on, out of the way of any panel that is open above it.`,
}));
const replies = [
  { hash: hex(3), author: dave, text: "A reply", ts: ts + 60, likes: 1, parent_hash: cast.hash },
  { hash: hex(4), author: erin, text: "Another reply", ts: ts + 120, likes: 0, parent_hash: cast.hash },
];
const cards = { "7": { bio: "Dave's bio", followers: 12, following: 3 }, "8": { bio: "Erin's bio", followers: 5, following: 1 } };

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [cast, ...others], notes: { users: { "42": { bio: "Alice's bio", followers: 9, following: 2 } } }, meta } });
  });
  await page.route("**/api/v1/reactions?*", (route) => {
    const url = new URL(route.request().url());
    return route.fulfill({ json: { hash: cast.hash, kind: url.searchParams.get("type") ?? "likes", users: [erin, dave], more: true, cards, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash");
    const found = others.find((c) => c.hash === hash) ?? cast;
    const children = found === cast ? replies.map((reply) => ({ cast: reply, children: [] })) : [];
    return route.fulfill({ json: { thread: { cast: found, children }, notes: { users: cards }, meta } });
  });
  await page.route("**/api/v1/replies?*", (route) => route.fulfill({ json: { hash: cast.hash, replies, notes: { users: cards }, meta } }));
  await page.route("**/api/v1/vote?*", (route) => route.fulfill({ json: { fid: 7, onchain: "stay", meta } }));
  await page.route("**/api/v1/wallets?*", (route) => route.fulfill({ json: { fid: 7, addresses: [], chains: [], total: 0, sources: [], fetched_at: Date.now() } }));
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: { user: { ...alice, follower_count: 9, following_count: 2 }, pool: [cast], notes: {}, meta } }));
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true,
  });
  return { context, page: await context.newPage() };
}

/**
 * A point on a card's text, well clear of every open panel and of every
 * other button and link (a finger's tap is drawn to a target near it), and
 * the card's hash.
 */
async function besidePanels(page: Page) {
  const point = await page.evaluate(() => {
    const panels = [...document.querySelectorAll(".popover-panel")].map((p) => p.getBoundingClientRect());
    const clear = 28;
    for (const text of document.querySelectorAll("#app .cast-card .cast-text")) {
      const box = text.getBoundingClientRect();
      for (let y = Math.max(box.top, 90) + 6; y < Math.min(box.bottom, innerHeight - 90); y += 6) {
        for (let x = box.left + 6; x < box.right - 6; x += 8) {
          const near = panels.some((p) => x >= p.left - clear && x <= p.right + clear && y >= p.top - clear && y <= p.bottom + clear);
          // Clear of other targets too: a finger's tap goes to a button
          // whose band (44 px tall on a touch screen) is near it.
          const targets = "a[href], button, [role='button']";
          const nearTarget = [[0, -26], [0, 26], [-26, 0], [26, 0], [0, 0]].some(([dx, dy]) =>
            document.elementFromPoint(x + dx, y + dy)?.closest(targets),
          );
          if (!near && !nearTarget && text.contains(document.elementFromPoint(x, y))) {
            return { x, y, hash: text.closest(".cast-card")!.getAttribute("data-hash")! };
          }
        }
      }
    }
    return null;
  });
  expect(point, "a spot on a card's text clear of the panels").not.toBeNull();
  return point!;
}

const opened = (page: Page) => page.locator(".popover-panel:not(.closing)");

for (const [name, trigger, label] of [
  ["likes", ".stat-likes .popover-trigger", "Liked by"],
  ["recasts", ".stat-recasts .popover-trigger", "Recasted by"],
  ["replies", ".stat-replies", "Replies"],
] as const) {
  test(`on a phone, a tap beside an open ${name} preview only closes it`, async ({ browser }, testInfo) => {
    const { context, page } = await phone(browser, testInfo);
    await serve(page);
    await page.goto("/");
    const card = page.locator(`.cast-card[data-hash="${cast.hash}"]`);
    await card.locator(trigger).tap();
    const panel = page.getByRole("dialog", { name: label, exact: true });
    await expect(panel).toBeVisible();
    const spot = await besidePanels(page);
    await page.touchscreen.tap(spot.x, spot.y);
    await expect(panel).toHaveCount(0);
    // It stayed on the feed: the tap didn't also open the post.
    await page.waitForTimeout(400);
    await expect(page).toHaveURL(/\/$/);
    await expect(card).toBeVisible();
    // The next tap there is the card's: it opens the post.
    await page.touchscreen.tap(spot.x, spot.y);
    await expect(page).toHaveURL(new RegExp(`/p/${spot.hash}$`));
    await context.close();
  });
}

test("on a phone, a tap on a link beside an open preview only closes the preview", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await serve(page);
  await page.goto("/");
  const card = page.locator(`.cast-card[data-hash="${cast.hash}"]`);
  await card.locator(".stat-likes .popover-trigger").tap();
  const panel = page.getByRole("dialog", { name: "Liked by", exact: true });
  await expect(panel).toBeVisible();
  // The time links to the post; a tap on it only closes the preview.
  const when = card.locator("a.when");
  const box = (await when.boundingBox())!;
  await page.touchscreen.tap(box.x + box.width / 2, box.y + box.height / 2);
  await expect(panel).toHaveCount(0);
  await page.waitForTimeout(400);
  await expect(page).toHaveURL(/\/$/);
  // Another count's preview opens on the next tap, not this one.
  await card.locator(".stat-recasts .popover-trigger").tap();
  await expect(page.getByRole("dialog", { name: "Recasted by", exact: true })).toBeVisible();
  await context.close();
});

test("on a phone, a tap beside an author's card only closes the card", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await serve(page);
  await page.goto("/");
  const card = page.locator(`.cast-card[data-hash="${cast.hash}"]`);
  await card.locator(".user-chip .author").tap();
  const profile = page.locator(".popover-panel.profile-card");
  await expect(profile).toContainText("Alice's bio");
  const spot = await besidePanels(page);
  await page.touchscreen.tap(spot.x, spot.y);
  await expect(profile).toHaveCount(0);
  await page.waitForTimeout(400);
  await expect(page).toHaveURL(/\/$/);
  await context.close();
});

test("on a phone, a tap beside a card opened from a preview's list closes that card, then the list", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await serve(page);
  await page.goto("/");
  const card = page.locator(`.cast-card[data-hash="${cast.hash}"]`);
  await card.locator(".stat-likes .popover-trigger").tap();
  const list = page.getByRole("dialog", { name: "Liked by", exact: true });
  await list.getByRole("link", { name: /Dave/ }).tap();
  const profile = page.locator(".popover-panel.profile-card");
  await expect(profile).toContainText("Dave's bio");
  // A tap on Erin, in the list above Dave's card, only closes Dave's card:
  // it doesn't open Erin's too.
  const erinLink = list.getByRole("link", { name: /Erin/ });
  const erinBox = (await erinLink.boundingBox())!;
  const clear = await profile.evaluate((p, b) => {
    const r = p.getBoundingClientRect();
    const [x, y] = [b.x + b.width / 2, b.y + b.height / 2];
    return x < r.left - 28 || x > r.right + 28 || y < r.top - 28 || y > r.bottom + 28;
  }, erinBox);
  expect(clear, "Erin's name is clear of Dave's card").toBe(true);
  await page.touchscreen.tap(erinBox.x + erinBox.width / 2, erinBox.y + erinBox.height / 2);
  await expect(profile).toHaveCount(0);
  await expect(list).toBeVisible();
  await page.waitForTimeout(300);
  await expect(page.locator(".popover-panel.profile-card")).toHaveCount(0);
  // The next tap on Erin opens hers.
  await erinLink.tap();
  await expect(page.locator(".popover-panel.profile-card")).toContainText("Erin's bio");
  // A tap on a card, beside both, closes both, and stays on the feed.
  const spot = await besidePanels(page);
  await page.touchscreen.tap(spot.x, spot.y);
  await expect(opened(page)).toHaveCount(0);
  await page.waitForTimeout(400);
  await expect(page).toHaveURL(/\/$/);
  await context.close();
});

test("on a phone, the full list opened from a preview takes its first tap", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await serve(page);
  await page.goto("/");
  const card = page.locator(`.cast-card[data-hash="${cast.hash}"]`);
  await card.locator(".stat-likes .popover-trigger").tap();
  await page.getByRole("dialog", { name: "Liked by", exact: true }).getByRole("button", { name: "See everyone →" }).tap();
  const dialog = page.locator(".who-modal");
  await expect(dialog.getByRole("link", { name: /Dave/ })).toBeVisible();
  // Its × closes it at the first tap.
  await dialog.getByRole("button", { name: "Close", exact: true }).tap();
  await expect(dialog).toHaveCount(0);
  await expect(page).toHaveURL(/\/$/);
  await context.close();
});

test("on a phone, Escape closes a tapped preview", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await serve(page);
  await page.goto("/");
  const card = page.locator(`.cast-card[data-hash="${cast.hash}"]`);
  await card.locator(".stat-likes .popover-trigger").tap();
  const panel = page.getByRole("dialog", { name: "Liked by", exact: true });
  await expect(panel).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(panel).toHaveCount(0);
  await context.close();
});

test("on a computer, a preview the pointer opened closes as it leaves, and a click elsewhere does what it does", async ({ page }, testInfo) => {
  test.skip(Boolean(testInfo.project.use.hasTouch), "hover needs a mouse");
  await serve(page);
  await page.goto("/");
  const card = page.locator(`.cast-card[data-hash="${cast.hash}"]`);
  const panel = page.getByRole("dialog", { name: "Liked by", exact: true });
  // Pointing away closes it.
  await card.locator(".stat-likes .popover-trigger").hover();
  await expect(panel).toBeVisible();
  await page.mouse.move(5, 300);
  await expect(panel).toHaveCount(0);
  // Open, a click on the card's text opens the post at once.
  await card.locator(".stat-likes .popover-trigger").hover();
  await expect(panel).toBeVisible();
  const spot = await besidePanels(page);
  await page.mouse.click(spot.x, spot.y);
  await expect(page).toHaveURL(new RegExp(`/p/${spot.hash}$`));
});

test("on a computer, a click on a link while a preview is open follows it", async ({ page }, testInfo) => {
  test.skip(Boolean(testInfo.project.use.hasTouch), "hover needs a mouse");
  await serve(page);
  await page.goto("/");
  const card = page.locator(`.cast-card[data-hash="${cast.hash}"]`);
  await card.locator(".stat-likes .popover-trigger").hover();
  const panel = page.getByRole("dialog", { name: "Liked by", exact: true });
  await expect(panel).toBeVisible();
  await panel.getByRole("link", { name: /Dave/ }).click();
  await expect(page).toHaveURL(/\/u\/dave$/);
});

test("on a computer, React's picker, which a click opens, closes at the first click outside it", async ({ page }, testInfo) => {
  test.skip(Boolean(testInfo.project.use.hasTouch), "a mouse's click");
  const me = { fid: 1234, username: "reader", display_name: "Reader" };
  const cors = { "access-control-allow-origin": "*" };
  await signIn(page, me);
  await serve(page);
  await page.route("**/api/v1/signer", (route) => route.fulfill({ json: { app_fid: 9152 } }));
  await page.route("https://haatz.quilibrium.com/v1/**", (route) =>
    route.fulfill({ status: 400, headers: cors, json: { error: "Failed", error_detail: 'status: NotFound, message: "not found"' } }),
  );
  await page.goto(`/p/${cast.hash}`);
  await expect(page.locator(".post")).toBeVisible();
  await approvedKey(page, me.fid, 3_600_000);
  await page.reload();
  const react = page.locator(".post").getByRole("button", { name: "React to @alice's cast with an emoji" });
  await react.click();
  const picker = page.getByRole("dialog", { name: "React with a Pepe" });
  await expect(picker).toBeVisible();
  // A reply's time links to it: the first click only closes the picker.
  const when = page.locator(".comment a.when").first();
  await when.click();
  await expect(picker).toHaveCount(0);
  await page.waitForTimeout(400);
  await expect(page).toHaveURL(new RegExp(`/p/${cast.hash}$`));
  // The next follows it.
  const target = (await when.getAttribute("href"))!;
  await when.click();
  await expect(page).toHaveURL(new RegExp(`${target}$`));
});
