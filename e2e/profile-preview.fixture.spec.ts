// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { appCame } from "./helpers/app";
import { NOW } from "./fixture-time";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = (NOW - 19);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const bob = { fid: 43, username: "bob", display_name: "Bob" };
const dave = { fid: 45, username: "dave", display_name: "Dave" };
// 7 Nov 2023, the day the ID registry moved to OP Mainnet: what accounts
// registered before it report.
const migrated = 1699386171;
// What the feed's payload says about its authors: nothing about dave.
const users = {
  "42": { bio: "Interested in reading conversations.", followers: 1221547, following: 77, registered_at: migrated },
  "43": { bio: "Replies a lot.", followers: 12, following: 3, registered_at: 1759222921 },
};
const feed = [
  { hash: hex(1), author: alice, text: "A cast by Alice", ts },
  { hash: hex(2), author: bob, text: "A cast by Bob", ts: ts - 60 },
  { hash: hex(3), author: dave, text: "A cast by Dave", ts: ts - 120 },
];

/** Serves the feed at once and each profile only when `release` is called. */
async function serve(page: Page) {
  const pending: (() => void)[] = [];
  const requests: string[] = [];
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : feed, notes: { users }, meta } });
  });
  await page.route("**/api/v1/vote?*", (route) => route.fulfill({ json: { fid: 0, onchain: "stay", meta } }));
  await page.route("**/api/v1/wallets?*", (route) => route.fulfill({ json: { fid: 0, addresses: [], chains: [], total: 0, sources: [], fetched_at: Date.now() } }));
  await page.route("**/api/v1/user?*", (route) => {
    const name = new URL(route.request().url()).searchParams.get("name")!;
    requests.push(name);
    const author = [alice, bob, dave].find((a) => a.username === name)!;
    const user = {
      ...author,
      bio: `${name}'s whole bio, longer than the start a card carries.`,
      follower_count: 1000,
      following_count: 10,
      registered_at: 1759222921,
    };
    const pool = [{ hash: hex(100 + author.fid), author, text: `${name}'s own cast`, ts }];
    pending.push(() => void route.fulfill({ json: { user, pool, notes: {}, meta } }));
  });
  return { requests, release: () => pending.splice(0).forEach((send) => send()) };
}

const open = async (page: Page, username: string) => {
  // The app's own cards, not the ones drawn before it came, whose links a
  // dispatched click would follow as a page load.
  await appCame(page);
  // The handle's link, followed as the router follows it (a tap on touch
  // screens would show the profile card first).
  await page.locator(`.cast-card header.cast-meta > .byline > .user-chip > a.author[href="/u/${username}"]`).dispatchEvent("click");
  await expect(page).toHaveURL(new RegExp(`/u/${username}$`));
};

test("a profile opened from the feed shows its header at once, then its casts", async ({ page }) => {
  const api = await serve(page);
  await page.goto("/");
  await expect(page.locator(".cast-card").first()).toBeVisible();
  await open(page, "bob");

  // Its payload hasn't arrived: the header comes from the feed.
  const header = page.locator("section.profile");
  await expect(header.locator(".profile-name")).toHaveText("Bob", { timeout: 300 });
  await expect(header.locator(".handle")).toHaveText("@bob · fid 43");
  await expect(header.locator(".counts")).toContainText("12 followers");
  await expect(header.locator(".counts")).toContainText("3 following");
  await expect(header.locator(".bio")).toHaveText("Replies a lot.");
  await expect(header.locator(".fact", { hasText: "Joined" }).locator("dd")).toHaveText("30 Sep 2025");
  await expect(page.locator(".cast-list[aria-busy=true]")).toBeVisible();
  await expect(page).toHaveTitle("Bob (@bob) · Allthing");
  expect(api.requests).toEqual(["bob"]);

  // Then the profile itself.
  api.release();
  await expect(page.locator(".cast-card .cast-text")).toHaveText("bob's own cast");
  await expect(header.locator(".bio")).toHaveText("bob's whole bio, longer than the start a card carries.");
  await expect(header.locator(".counts")).toContainText("1,000 followers");
  await expect(page.locator(".cast-list[aria-busy=true]")).toHaveCount(0);
  expect(api.requests).toEqual(["bob"]);
});

test("a preview says nothing it doesn't know: no join date the profile would correct, no bio or counts without a card", async ({ page }) => {
  const api = await serve(page);
  await page.goto("/");
  await expect(page.locator(".cast-card").first()).toBeVisible();

  // Alice registered before the migration: her card can't date it yet.
  await open(page, "alice");
  const header = page.locator("section.profile");
  await expect(header.locator(".profile-name")).toHaveText("Alice", { timeout: 300 });
  await expect(header.locator(".counts")).toContainText("1,221,547 followers");
  await expect(header.locator(".fact", { hasText: "Joined" })).toHaveCount(0);
  await expect(header).not.toContainText("before Nov 2023");

  // Nothing came with the feed about dave: placeholders, not "No bio." or zeros.
  await page.goBack();
  await expect(page.locator(".cast-card").first()).toBeVisible();
  await open(page, "dave");
  await expect(header.locator(".profile-name")).toHaveText("Dave", { timeout: 300 });
  await expect(header).toHaveAttribute("aria-busy", "true");
  await expect(header.locator(".counts .sk")).toBeVisible();
  await expect(header).not.toContainText("No bio.");
  await expect(header).not.toContainText("0 followers");

  api.release();
  await expect(header.locator(".bio")).toHaveText("dave's whole bio, longer than the start a card carries.");
  await expect(header).not.toHaveAttribute("aria-busy", "true");
});

test("a profile visited directly waits for its payload, as before", async ({ page }) => {
  const api = await serve(page);
  await page.goto("/u/bob");
  await expect(page.locator(".profile.skeleton")).toBeVisible();
  await expect(page.locator(".profile-name")).toHaveCount(0);
  await expect.poll(() => api.requests).toEqual(["bob"]);
  api.release();
  await expect(page.locator(".profile-name")).toHaveText("Bob");
});
