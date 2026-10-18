// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const bob = { fid: 43, username: "bob", display_name: "Bob" };
const carol = { fid: 44, username: "carol", display_name: "Carol" };
// Registered on 7 Nov 2023, the day the ID registry moved to OP Mainnet.
const migrated = 1699386171;
const cards = {
  "42": { bio: "Interested in reading conversations.", followers: 1221547, following: 77, registered_at: migrated },
  "43": { bio: "Replies a lot.", followers: 12, following: 3, registered_at: 1759222921 },
  "44": { bio: "Replies to replies.", followers: 5, following: 1, registered_at: 1759222921 },
};
const cast = { hash: hex(1), author: alice, text: "Hover card fixture", ts, likes: 2, replies: 1 };
const reply = { hash: hex(2), author: bob, text: "First reply", ts, parent_hash: hex(1), replies: 1 };
const nested = { hash: hex(3), author: carol, text: "Nested reply", ts, parent_hash: hex(2) };
// The Farcaster app's label for fid 42: likely spam ('0').
const labels = { source: "fixture", built_at: ts, newest_label: ts, shard_size: 32768, shards: 1,
  counts: { likely_spam: 1, not_spam: 0, nerfed: 0 } };

async function serve(page: Page) {
  const requests = { vote: 0, user: 0 };
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home", (route) => route.fulfill({ json: { pool: [cast], notes: { users: cards }, meta } }));
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
    thread: { cast, children: [{ cast: reply, children: [{ cast: nested }] }] }, notes: { users: cards }, meta,
  } }));
  await page.route("**/api/v1/reactions?*", (route) => route.fulfill({ json: {
    hash: cast.hash, kind: "likes", users: [{ fid: 7, username: "dave" }], more: false, meta,
  } }));
  await page.route("**/api/v1/vote?*", (route) => {
    requests.vote += 1;
    const fid = Number(new URL(route.request().url()).searchParams.get("fid"));
    return route.fulfill({ json: { fid, onchain: "stay", app: "opted_in", meta } });
  });
  await page.route("**/api/v1/user?*", (route) => {
    requests.user += 1;
    return route.fulfill({ json: { user: { ...alice, follower_count: 1221547 }, pool: [], meta } });
  });
  await page.route("**/data/labels/spam/meta.json", (route) => route.fulfill({ json: labels }));
  await page.route("**/data/labels/spam/0000.txt", (route) =>
    route.fulfill({ contentType: "text/plain", body: `${"-".repeat(42)}0` }));
  return requests;
}

const profileOf = (page: Page, name: string, username: string) =>
  page.getByRole("dialog", { name: `Profile: ${name} (@${username})`, exact: true });

test("hovering a picture shows the account's card; a click on the name still opens the profile", async ({ page }) => {
  const requests = await serve(page);
  await page.goto("/");
  const feedCard = page.locator(".cast-card").first();
  const chip = feedCard.locator(".user-chip");
  await expect(chip).toBeVisible();
  expect(requests.vote).toBe(0);

  await chip.locator(".avatar").hover();
  const profile = profileOf(page, "Alice", "alice");
  await expect(profile).toBeVisible();
  await expect(profile).toBeInViewport({ ratio: 1 });
  await expect(profile.getByRole("link", { name: /Alice\s*@alice/ })).toHaveAttribute("href", "/u/alice");
  await expect(profile).toContainText("Interested in reading conversations.");
  await expect(profile).toContainText("1,221,547 followers");
  await expect(profile).toContainText("77 following");
  // Registered on the migration day: the profile's own wording.
  await expect(profile.locator(".fact", { hasText: "Registered" }).locator("dd")).toHaveText("before Nov 2023");
  // The vote loads once the card is open, and the label shows (from the
  // Farcaster app's dataset), while the cast itself stays listed.
  await expect(profile.locator(".vote-fact dd")).toHaveText("Voted yes");
  expect(requests.vote).toBe(1);
  await expect(profile.locator(".fact", { hasText: "Farcaster app label" }).locator("dd")).toHaveText("Labeled likely spam");
  await expect(feedCard.locator(".cast-text")).toHaveText("Hover card fixture");
  // No scores, and no request for the profile's own payload.
  await expect(profile).not.toContainText(/score/i);
  await expect(page.locator(".score")).toHaveCount(0);
  expect(requests.user).toBe(0);

  await page.keyboard.press("Escape");
  await expect(profile).toHaveCount(0);

  // A mouse click on the handle goes to the profile, as before.
  await chip.locator(".author").click();
  await expect(page).toHaveURL(/\/u\/alice$/);
});

test("clicking inside a card neither closes it nor opens the post", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  await page.locator(".cast-card").first().locator(".user-chip .author").hover();
  const profile = profileOf(page, "Alice", "alice");
  await expect(profile).toBeVisible();
  await profile.locator(".profile-card-bio").click();
  await page.waitForTimeout(300);
  await expect(page).toHaveURL(/\/$/);
  await expect(profile).toBeVisible();
  // Moving away closes it.
  await page.mouse.move(5, 5);
  await expect(profile).toHaveCount(0);
});

test("keyboard focus resting on a name opens the card, Tab reaches it, Escape returns", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  const handle = page.locator(".cast-card").first().locator(".user-chip .author");
  await expect(handle).toBeVisible();
  await page.keyboard.press("Tab");
  await handle.focus();
  const profile = profileOf(page, "Alice", "alice");
  await expect(profile).toBeVisible();
  await page.keyboard.press("Tab");
  await expect(profile.getByRole("link", { name: /Alice\s*@alice/ })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(profile).toHaveCount(0);
  await expect(handle).toBeFocused();
});

test("a nested reply's card reaches past the reply instead of being cut off", async ({ page }) => {
  await serve(page);
  await page.goto(`/p/${cast.hash}`);
  const chip = page.locator(".children .comment-head .user-chip");
  await expect(chip).toBeVisible();
  await chip.locator(".avatar").hover();
  const profile = profileOf(page, "Carol", "carol");
  await expect(profile).toBeVisible();
  await expect(profile).toBeInViewport({ ratio: 1 });
  await expect(profile).toContainText("Replies to replies.");
  await page.mouse.move(5, 5);
  await expect(profile).toHaveCount(0);
  // The post's author has a card too, over the replies below it.
  await page.locator(".post-author .display-name").hover();
  const author = profileOf(page, "Alice", "alice");
  await expect(author).toBeVisible();
  await expect(author).toBeInViewport({ ratio: 1 });
  // Nothing below it covers its link (a trial click checks what is on top).
  await author.getByRole("link", { name: "View profile →", exact: true }).click({ trial: true });
});

test("people in the full list of likes show cards that the list doesn't cut off", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  await page.getByRole("button", { name: "2 likes: show who", exact: true }).click();
  const row = page.locator(".who-modal").getByRole("link", { name: /@dave/ });
  await row.hover();
  const profile = profileOf(page, "dave", "dave");
  await expect(profile).toBeVisible();
  await expect(profile).toBeInViewport({ ratio: 1 });
  // Nothing about dave came with the page: the card says where to look.
  await expect(profile).toContainText("The bio and follower counts are on the profile.");
  await expect(profile.locator(".vote-fact dd")).toHaveText("Voted yes");
  await row.click();
  await expect(page).toHaveURL(/\/u\/dave$/);
  await expect(page.locator(".who-modal")).toHaveCount(0);
});

test.describe("on a touch screen", () => {
  test.use({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });

  test("a tap on a picture shows the card instead of opening the profile", async ({ page }) => {
    await serve(page);
    await page.goto("/");
    const chip = page.locator(".cast-card").first().locator(".user-chip");
    await chip.locator(".avatar").tap();
    const profile = profileOf(page, "Alice", "alice");
    await expect(profile).toBeVisible();
    await expect(profile).toBeInViewport({ ratio: 1 });
    await expect(page).toHaveURL(/\/$/);
    await expect(profile.locator(".vote-fact dd")).toHaveText("Voted yes");
    // A tap inside the card leaves it open and doesn't open the post.
    await profile.locator(".profile-card-bio").tap();
    await page.waitForTimeout(300);
    await expect(profile).toBeVisible();
    await expect(page).toHaveURL(/\/$/);
    // The card's link opens the profile.
    await profile.getByRole("link", { name: "View profile →", exact: true }).tap();
    await expect(page).toHaveURL(/\/u\/alice$/);
  });

  test("a second tap on the name closes the card", async ({ page }) => {
    await serve(page);
    await page.goto("/");
    const name = page.locator(".cast-card").first().locator(".user-chip .display-name");
    await name.tap();
    const profile = profileOf(page, "Alice", "alice");
    await expect(profile).toBeVisible();
    await name.tap();
    await expect(profile).toHaveCount(0);
    await expect(page).toHaveURL(/\/$/);
  });
});
