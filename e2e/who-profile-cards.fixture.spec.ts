// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page, type TestInfo } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
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
  await page.route("**/api/v1/reactions?*", (route) => route.fulfill({ json: {
    hash: cast.hash, kind: "likes", users: [dave, erin], more: false, meta,
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
    await serve(page);
    await page.goto("/");
    const card = page.locator(".cast-card:not(.skeleton)").first();
    await card.locator(".stat-likes").hover();
    const list = page.getByRole("dialog", { name: "Liked by" });
    await expect(list).toBeVisible();
    await list.getByRole("link", { name: /Dave/ }).hover();
    const profile = page.locator(".popover-panel.profile-card");
    await expect(profile).toBeVisible();
    await expect(profile).toContainText("@dave");
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
    const background = await profile.evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(background).not.toMatch(/rgba\(.*,\s*0\)|transparent/);
    // Moving into the card keeps both open.
    await profile.hover();
    await expect(list).toBeVisible();
    await expect(profile).toBeVisible();
  });

  test("a tap on a name in Liked by opens its card and doesn't follow the link", async ({ page }, testInfo: TestInfo) => {
    test.skip(!testInfo.project.use.hasTouch, "tap needs a touch screen");
    await serve(page);
    await page.goto("/");
    const card = page.locator(".cast-card:not(.skeleton)").first();
    await card.locator(".stat-likes").tap();
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
