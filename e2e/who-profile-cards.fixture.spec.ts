// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page, type TestInfo } from "@playwright/test";
import { NOW } from "./fixture-time";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = (NOW - 19);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const dave = { fid: 7, username: "dave", display_name: "Dave" };
const erin = { fid: 8, username: "erin", display_name: "Erin" };
const cast = { hash: hex(1), author: alice, text: "A cast two people liked", ts, likes: 2, replies: 0 };

async function serve(page: Page) {
  const requests = { user: 0 };
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [cast], notes: { users: {} }, meta } });
  });
  // The list carries its people's cards, as the Worker builds it from the
  // user objects that come with the reactions.
  await page.route("**/api/v1/reactions?*", (route) => route.fulfill({ json: {
    hash: cast.hash, kind: "likes", users: [dave, erin], more: false, meta,
    cards: { "7": { bio: "Dave's bio", followers: 1234, following: 56, registered_at: 1759222921 } },
  } }));
  await page.route("**/api/v1/vote?*", (route) => route.fulfill({ json: { fid: 7, onchain: "stay", app: "opted_in", meta } }));
  await page.route("**/api/v1/wallets?*", (route) => route.fulfill({ json: {
    fid: 7, addresses: [], chains: [], total: 0, sources: [], fetched_at: Date.now(),
  } }));
  await page.route("**/api/v1/user?*", (route) => {
    requests.user += 1;
    return route.fulfill({ json: { user: { ...dave, follower_count: 3, following_count: 1, bio: "Dave's bio" }, pool: [], notes: {}, meta } });
  });
  return requests;
}

const overlaps = (a: { x: number; width: number }, b: { x: number; width: number }) =>
  a.x < b.x + b.width && b.x < a.x + a.width;

test.describe("profile cards in the who-list", () => {
  test("hovering a name in Liked by opens its card beside the list, which stays open", async ({ page }, testInfo: TestInfo) => {
    test.skip(Boolean(testInfo.project.use.hasTouch), "hover needs a mouse");
    await page.setViewportSize({ width: 1280, height: 900 });
    const requests = await serve(page);
    await page.goto("/");
    const card = page.locator(".cast-card:not(.skeleton)").first();
    await card.locator(".stat-likes").hover();
    const list = page.getByRole("dialog", { name: "Liked by" });
    await expect(list).toBeVisible();
    await list.getByRole("link", { name: /Dave/ }).hover();
    const profile = page.locator(".popover-panel.profile-card");
    await expect(profile).toBeVisible();
    await expect(profile).toContainText("@dave");
    // Complete at once, from the list: bio, counts and join date, and no
    // read of the profile.
    await expect(profile.locator(".profile-card-bio")).toHaveText("Dave's bio");
    await expect(profile.locator(".profile-card-counts")).toContainText("1,234 followers");
    await expect(profile.locator(".profile-card-counts")).toContainText("56 following");
    await expect(profile.locator(".fact", { hasText: "Joined" }).locator("dd")).toHaveText("30 Sep 2025");
    await expect(profile).not.toContainText("on the profile");
    expect(requests.user).toBe(0);
    // Beside the list: no overlap with it, and the card is what the pointer
    // reaches (nothing covers it), with a solid background.
    const [listBox, cardBox] = [await list.boundingBox(), await profile.boundingBox()];
    expect(listBox && cardBox).toBeTruthy();
    expect(overlaps({ x: listBox!.x, width: listBox!.width }, { x: cardBox!.x, width: cardBox!.width })).toBe(false);
    const topmost = await page.evaluate(([x, y]) => {
      const el = document.elementFromPoint(x, y);
      return !!el?.closest(".profile-card");
    }, [cardBox!.x + cardBox!.width / 2, cardBox!.y + cardBox!.height / 2]);
    expect(topmost).toBe(true);
    // Level with the list, the way it opened, not hanging from the name.
    const level = Math.abs(cardBox!.y - listBox!.y) < 2 || Math.abs(cardBox!.y + cardBox!.height - (listBox!.y + listBox!.height)) < 2;
    expect(level).toBe(true);
    const background = await profile.evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(background).not.toMatch(/rgba\(.*,\s*0\)|transparent/);
    // Moving into the card keeps both open.
    await profile.hover();
    await expect(list).toBeVisible();
    await expect(profile).toBeVisible();
  });

  test("on a phone, a tap on a name in Liked by opens its complete card", async ({ browser }, testInfo: TestInfo) => {
    const context = await browser.newContext({
      baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true,
    });
    const page = await context.newPage();
    const requests = await serve(page);
    await page.goto("/");
    await page.locator(".cast-card:not(.skeleton)").first().locator(".stat-likes .stat-count").tap();
    const list = page.getByRole("dialog", { name: "Liked by" });
    await expect(list).toBeVisible();
    await list.getByRole("link", { name: /Dave/ }).tap();
    const profile = page.locator(".popover-panel.profile-card");
    await expect(profile.locator(".profile-card-bio")).toHaveText("Dave's bio");
    await expect(profile.locator(".profile-card-counts")).toContainText("1,234 followers");
    await expect(profile).toBeInViewport({ ratio: 1 });
    await expect(page).toHaveURL(/\/$/);
    expect(requests.user).toBe(0);
    await context.close();
  });

  test("a tap on a name in Liked by opens its card and doesn't follow the link", async ({ page }, testInfo: TestInfo) => {
    test.skip(!testInfo.project.use.hasTouch, "tap needs a touch screen");
    await serve(page);
    await page.goto("/");
    const card = page.locator(".cast-card:not(.skeleton)").first();
    await card.locator(".stat-likes .stat-count").tap();
    const list = page.getByRole("dialog", { name: "Liked by" });
    await expect(list).toBeVisible();
    await list.getByRole("link", { name: /Dave/ }).tap();
    const profile = page.locator(".popover-panel.profile-card");
    await expect(profile).toBeVisible();
    await expect(profile).toContainText("@dave");
    await expect(list).toBeVisible();
    await expect(page).toHaveURL(/\/$/);
  });
});
