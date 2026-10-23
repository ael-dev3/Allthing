// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Locator, type Page } from "@playwright/test";
import { appCame } from "./helpers/app";
import { NOW } from "./fixture-time";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = (NOW - 19);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const bob = { fid: 43, username: "bob", display_name: "Bob" };
const carol = { fid: 44, username: "carol", display_name: "Carol" };
// 7 Nov 2023, the day the ID registry moved to OP Mainnet: what accounts
// registered before it report.
const migrated = 1699386171;
// What the payload says about the accounts it shows (`notes.users`).
const users = {
  "42": { bio: "Interested in reading conversations.", followers: 1221547, following: 77, registered_at: migrated },
  "43": { bio: "Replies a lot.", followers: 12, following: 3, registered_at: 1759222921 },
  "44": { bio: "Replies to replies.", followers: 5, following: 1, registered_at: 1759222921 },
};
const cast = { hash: hex(1), author: alice, text: "Hover card fixture", ts, likes: 2, replies: 1 };
const others = [
  { hash: hex(2), author: bob, text: "The next card", ts: ts - 60 },
  { hash: hex(3), author: carol, text: "The card after it", ts: ts - 120 },
];
const reply = { hash: hex(4), author: bob, text: "First reply", ts, parent_hash: hex(1), replies: 1 };
const nested = { hash: hex(5), author: carol, text: "Nested reply", ts, parent_hash: hex(4) };
const wallets = {
  fid: 42, addresses: [{ address: hex(9), kind: "evm", primary: true }],
  chains: [
    { chain: "base", name: "Base", kind: "evm", count: 30, percent: 75 },
    { chain: "ethereum", name: "Ethereum", kind: "evm", count: 10, percent: 25 },
  ],
  total: 40, sources: [], fetched_at: Date.now(),
};

async function serve(page: Page) {
  const requests = { vote: 0, wallets: 0, user: 0, labels: 0 };
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    // Later pages are empty, so the feed stops asking.
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [cast, ...others], notes: { users }, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
    thread: { cast, children: [{ cast: reply, children: [{ cast: nested }] }] }, notes: { users }, meta,
  } }));
  await page.route("**/api/v1/reactions?*", (route) => route.fulfill({ json: {
    hash: cast.hash, kind: "likes", users: [{ fid: 7, username: "dave" }], more: false, meta,
  } }));
  await page.route("**/api/v1/vote?*", (route) => {
    requests.vote += 1;
    const fid = Number(new URL(route.request().url()).searchParams.get("fid"));
    return route.fulfill({ json: { fid, onchain: "stay", app: "opted_in", meta } });
  });
  await page.route("**/api/v1/wallets?*", (route) => {
    requests.wallets += 1;
    return route.fulfill({ json: wallets });
  });
  await page.route("**/api/v1/user?*", (route) => {
    requests.user += 1;
    return route.fulfill({ json: { user: { ...alice, follower_count: 1221547 }, pool: [], meta } });
  });
  // The Farcaster app's spam labels are gone: nothing may ask for them.
  await page.route("**/data/labels/**", (route) => {
    requests.labels += 1;
    return route.fulfill({ status: 404, body: "" });
  });
  return requests;
}

const profileOf = (page: Page, name: string, username: string) =>
  page.getByRole("dialog", { name: `Profile: ${name} (@${username})`, exact: true });
const feedCard = (page: Page) => page.locator(`.cast-card[data-hash="${cast.hash}"]`);
// The chip's own links, not the ones in its card once that opens.
const chipLink = (card: Locator, which: "user-link" | "display-name" | "author") =>
  card.locator(`header.cast-meta > .byline > .user-chip > .${which}`);

// Hovering needs a mouse; touch screens open cards with a tap (tested below).
test.beforeEach(({}, testInfo) => {
  test.skip(Boolean(testInfo.project.use.hasTouch) && !testInfo.titlePath.some((t) => t.includes("touch")), "hover needs a mouse");
});

test("cards in view preload their authors' vote and wallets, so a hovered card opens complete", async ({ page }) => {
  const requests = await serve(page);
  await page.goto("/");
  await expect(chipLink(feedCard(page), "author")).toHaveText("@alice");
  // The three authors in view: their vote and wallets, before any hover.
  await expect.poll(() => requests.vote + requests.wallets).toBe(6);
  expect(requests.user).toBe(0);

  await chipLink(feedCard(page), "user-link").hover();
  const profile = profileOf(page, "Alice", "alice");
  // Open within the 100 ms delay, and complete in its first frame: no loading state.
  await expect(profile).toBeVisible({ timeout: 500 });
  await expect(profile.locator(".vote-fact dd")).toHaveText("Voted yes", { timeout: 100 });
  await expect(profile.locator(".vote-loading, .share-bar.loading")).toHaveCount(0);
  await expect(profile).toBeInViewport({ ratio: 1 });
  await expect(profile.getByRole("link", { name: /Alice\s*@alice/ })).toHaveAttribute("href", "/u/alice");
  await expect(profile.locator(".profile-card-bio")).toHaveText("Interested in reading conversations.");
  await expect(profile.locator(".profile-card-counts")).toContainText("1,221,547 followers");
  await expect(profile.locator(".profile-card-counts")).toContainText("77 following");
  // Registered on the migration day, and no earlier date known: the
  // profile's own wording.
  await expect(profile.locator(".fact", { hasText: "Joined" }).locator("dd")).toHaveText("before Nov 2023");

  await expect(profile.locator(".share-bar[role=img]")).toHaveAttribute("aria-label", "Base 75%, Ethereum 25%");
  await expect(profile.locator(".share-list li")).toHaveText([/Base\s*75%/, /Ethereum\s*25%/]);
  // Opening it read nothing more.
  expect(requests.vote + requests.wallets).toBe(6);
  // No account label: the card shows when it joined, the vote and the
  // wallets, and the cast stays listed.
  await expect(profile.locator(".fact dt")).toHaveText(["Joined", /NS vote/]);
  await expect(profile).not.toContainText(/label|spam|nerfed/i);
  expect(requests.labels).toBe(0);
  await expect(feedCard(page).locator(".cast-text")).toHaveText("Hover card fixture");
  // No scores, and no read of the profile's own payload.
  await expect(profile).not.toContainText(/score/i);
  expect(requests.user).toBe(0);
  await expect(profile.getByRole("link", { name: "View profile →", exact: true })).toHaveAttribute("href", "/u/alice");

  await page.keyboard.press("Escape");
  await expect(profile).toHaveCount(0);

  // A mouse click on the handle goes to the profile, as before.
  await chipLink(feedCard(page), "author").click();
  await expect(page).toHaveURL(/\/u\/alice$/);
});

test("a card's picture shows the small copy the feed loaded until its own has loaded", async ({ page }) => {
  await serve(page);
  // Alice has a picture. The feed's small copy comes at once; the card's
  // larger copy doesn't come during the test.
  const pfp = "https://imagedelivery.net/fixture/alice/original";
  const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64");
  await page.route("https://imagedelivery.net/**", (route) => {
    if (route.request().url().includes("w=64")) return route.fulfill({ contentType: "image/png", body: png });
  });
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [{ ...cast, author: { ...alice, pfp_url: pfp } }], notes: { users }, meta } });
  });
  await page.goto("/");
  await appCame(page);
  const small = feedCard(page).locator("header.cast-meta .avatar").first();
  await expect.poll(() => small.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBeGreaterThan(0);
  await chipLink(feedCard(page), "user-link").hover();
  const picture = profileOf(page, "Alice", "alice").locator("img.avatar-large");
  await expect(picture).toHaveAttribute("src", /w=140/);
  expect(await picture.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBe(0);
  expect(await picture.evaluate((img) => getComputedStyle(img).backgroundImage)).toContain(await small.getAttribute("src"));
});

test("the card is solid and on top of the cards below it; clicks inside it stay in it", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  await chipLink(feedCard(page), "author").hover();
  const profile = profileOf(page, "Alice", "alice");
  await expect(profile).toBeVisible();
  await expect(profile.locator(".vote-fact dd")).toHaveText("Voted yes");

  // It opens over the next card in the feed, and nothing there shows through
  // or covers it.
  const [box, next] = await Promise.all([profile.boundingBox(), page.locator(".cast-card").nth(1).boundingBox()]);
  expect(box!.y + box!.height).toBeGreaterThan(next!.y);
  expect(await profile.evaluate((panel) => getComputedStyle(panel).backgroundColor)).toBe("rgb(255, 255, 255)");
  const onTop = (panel: Element) => {
    const rect = panel.getBoundingClientRect();
    const hit = document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2);
    return hit !== null && panel.contains(hit);
  };
  expect(await profile.evaluate(onTop)).toBe(true);

  // The pointer can move into the card, and a click on its text neither
  // closes it nor opens the post.
  await profile.locator(".profile-card-bio").click();
  await page.waitForTimeout(500);
  await expect(page).toHaveURL(/\/$/);
  await expect(profile).toBeVisible();
  // Leaving closes it.
  await page.mouse.move(5, 5);
  await expect(profile).toHaveCount(0);
});

test("keyboard focus resting on a name opens the card, Tab goes through it and on, Escape returns", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  await appCame(page);
  const handle = chipLink(feedCard(page), "author");
  await expect(handle).toBeVisible();
  for (let i = 0; i < 40 && !(await handle.evaluate((el) => el === document.activeElement)); i++) {
    await page.keyboard.press("Tab");
  }
  await expect(handle).toBeFocused();
  const profile = profileOf(page, "Alice", "alice");
  await expect(profile).toBeVisible();
  await page.keyboard.press("Tab");
  await expect(profile.getByRole("link", { name: /Alice\s*@alice/ })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(profile).toHaveCount(0);
  await expect(handle).toBeFocused();

  // Focus is never trapped: from the card's last link, Tab goes on to the
  // post's own link, and the card closes behind it.
  await page.keyboard.press("Shift+Tab");
  await page.keyboard.press("Tab");
  await expect(handle).toBeFocused();
  await expect(profile).toBeVisible();
  const when = feedCard(page).locator("a.when");
  // Through the card: its name, Favorite, Show less, the facts' links and View profile.
  for (let i = 0; i < 10 && !(await when.evaluate((el) => el === document.activeElement)); i++) {
    await page.keyboard.press("Tab");
  }
  await expect(when).toBeFocused();
  await expect(profile).toHaveCount(0);
});

test("cards on the post page reach past a nested reply and over the replies", async ({ page }) => {
  await serve(page);
  await page.goto(`/p/${cast.hash}`);
  const nestedPicture = page.locator(".children .comment-head > .byline > .user-chip > .user-link");
  await expect(nestedPicture).toBeVisible();
  await nestedPicture.hover();
  const carolCard = profileOf(page, "Carol", "carol");
  await expect(carolCard).toBeVisible();
  // Not cut off by the reply around it.
  await expect(carolCard).toBeInViewport({ ratio: 1 });
  await expect(carolCard.locator(".profile-card-bio")).toHaveText("Replies to replies.");
  await page.mouse.move(5, 5);
  await expect(carolCard).toHaveCount(0);

  await page.locator(".post-author .display-name").hover();
  const author = profileOf(page, "Alice", "alice");
  await expect(author).toBeVisible();
  await expect(author).toBeInViewport({ ratio: 1 });
  // Nothing below covers its link (a trial click checks what is on top).
  await author.getByRole("link", { name: "View profile →", exact: true }).click({ trial: true });
});

test("people in the full list of likes show cards, without the profile's payload", async ({ page }) => {
  const requests = await serve(page);
  await page.goto("/");
  await page.getByRole("button", { name: "2 likes: show who", exact: true }).click();
  // The row's own link (the card that opens has one with the same name).
  const row = page.locator(".who-modal .who-link", { hasText: "@dave" });
  await row.hover();
  const profile = profileOf(page, "dave", "dave");
  await expect(profile).toBeVisible();
  await expect(profile).toBeInViewport({ ratio: 1 });
  // Nothing about dave came with the page: the card says where to look.
  await expect(profile).toContainText("The bio and follower counts are on the profile.");
  await expect(profile.locator(".vote-fact dd")).toHaveText("Voted yes");
  expect(requests.user).toBe(0);
  await row.click();
  await expect(page).toHaveURL(/\/u\/dave$/);
  await expect(page.locator(".who-modal")).toHaveCount(0);
});

test.describe("on a touch screen", () => {
  test.use({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });

  test("a tap on a picture shows the card instead of opening the profile; View profile opens it", async ({ page }) => {
    const requests = await serve(page);
    await page.goto("/");
    await chipLink(feedCard(page), "user-link").tap();
    const profile = profileOf(page, "Alice", "alice");
    await expect(profile).toBeVisible();
    await expect(profile).toBeInViewport({ ratio: 1 });
    await expect(page).toHaveURL(/\/$/);
    await expect(profile.locator(".vote-fact dd")).toHaveText("Voted yes");
    await expect(profile.locator(".share-bar[role=img]")).toHaveAttribute("aria-label", "Base 75%, Ethereum 25%");
    // A tap inside the card leaves it open and doesn't open the post.
    await profile.locator(".profile-card-bio").tap();
    await page.waitForTimeout(500);
    await expect(profile).toBeVisible();
    await expect(page).toHaveURL(/\/$/);
    expect(requests.user).toBe(0);
    // The card's link opens the profile.
    await profile.getByRole("link", { name: "View profile →", exact: true }).tap();
    await expect(page).toHaveURL(/\/u\/alice$/);
  });

  test("a second tap on the name closes the card", async ({ page }) => {
    await serve(page);
    await page.goto("/");
    const name = chipLink(feedCard(page), "display-name");
    await name.tap();
    const profile = profileOf(page, "Alice", "alice");
    await expect(profile).toBeVisible();
    await name.tap();
    await expect(profile).toHaveCount(0);
    await expect(page).toHaveURL(/\/$/);
  });
});
