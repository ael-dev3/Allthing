// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Locator, type Page } from "@playwright/test";
import { NOW } from "./fixture-time";

/*
 * No native tooltips on a post or in a popover. A browser's title tooltip is
 * drawn in the system's style over Allthing's own popovers, which are solid.
 * What the titles said is read out instead (aria-description), and the post's
 * signing key is shown in its meta line.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = (NOW - 19);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const bob = { fid: 43, username: "bob", display_name: "Bob" };
const users = {
  "42": { bio: "Reads conversations :pepe_no:", followers: 1200, following: 77, registered_at: 1759222921 },
  "43": { bio: "Replies a lot.", followers: 12, following: 3, registered_at: 1759222921 },
};
const post = { hash: hex(1), author: alice, text: "Tooltip fixture :pepe_no:", ts, likes: 3, recasts: 1, replies: 2 };
const reply = { hash: hex(2), author: bob, text: "A reply :pepe_no:", ts: ts + 60, parent_hash: hex(1) };
const again = { hash: hex(3), author: alice, text: "The poster again", ts: ts + 120, parent_hash: hex(1) };
const origins = Object.fromEntries([1, 2, 3].map((n) => [hex(n), { signer: hex(76 + n), app_fid: 9152 }]));
const apps = { "9152": { fid: 9152, username: "warpcast", display_name: "Warpcast" } };
const wallets = {
  fid: 43, addresses: [{ address: hex(9), kind: "evm", primary: true }],
  chains: [
    { chain: "base", name: "Base", kind: "evm", count: 30, percent: 75 },
    { chain: "ethereum", name: "Ethereum", kind: "evm", count: 10, percent: 25 },
  ],
  total: 40, sources: [], fetched_at: Date.now(),
};

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
    thread: { cast: post, children: [{ cast: reply }, { cast: again }] }, notes: { users, origins, apps }, meta,
  } }));
  await page.route("**/api/v1/reactions?*", (route) => route.fulfill({ json: {
    hash: post.hash, kind: new URL(route.request().url()).searchParams.get("kind") ?? "likes", users: [bob], more: false, meta,
  } }));
  await page.route("**/api/v1/vote?*", (route) => {
    const fid = Number(new URL(route.request().url()).searchParams.get("fid"));
    return route.fulfill({ json: { fid, onchain: "stay", app: "opted_in", meta } });
  });
  await page.route("**/api/v1/wallets?*", (route) => route.fulfill({ json: wallets }));
}

async function open(page: Page) {
  await serve(page);
  await page.goto(`/p/${post.hash}`);
  await expect(page.locator(".replies .comment")).toHaveCount(2);
}

/** A popover is solid, and nothing in it or around it has a native tooltip. */
async function solidWithoutTooltips(panel: Locator) {
  await expect(panel).toBeVisible();
  expect(await panel.evaluate((p) => getComputedStyle(p).backgroundColor)).toBe("rgb(255, 255, 255)");
  await expect(panel.locator("[title]")).toHaveCount(0);
  // A title on an element the panel sits inside shows over the panel too.
  expect(await panel.evaluate((p) => p.parentElement?.closest("[title]")?.getAttribute("title") ?? null)).toBeNull();
}

test("a post and its replies have no native tooltips, and say what they said another way", async ({ page }) => {
  await open(page);
  await expect(page.locator(".post [title], .replies [title]")).toHaveCount(0);
  // The signing key, in the post's meta line.
  await expect(page.locator(".post .post-via")).toHaveText("via Farcaster · key 0x0000…004d");
  // A reply's app, its time, the poster's mark and an emoji: read out.
  const first = page.locator(".replies .comment").first();
  await expect(first.locator(".origin")).toHaveAttribute("aria-description", "Signed with key 0x0000…004e requested by Farcaster (app fid 9152).");
  await expect(first.locator("time")).toHaveAttribute("aria-description", /UTC$/);
  await expect(page.locator(".op-mark")).toHaveAttribute("aria-description", "Original poster");
  await expect(page.locator(".post").getByRole("img", { name: ":pepe_no:" })).toHaveAttribute("aria-description", "Pepe holds up a NO sign");
});

test("the post's popovers are solid, with no native tooltip inside: likes, recasts, replies and a profile card", async ({ page }) => {
  await open(page);
  const foot = page.locator(".post .engagement");
  for (const [trigger, name] of [["3 likes: show who", "Liked by"], ["1 recast: show who", "Recasted by"], ["2 replies: preview", "Replies"]]) {
    await foot.getByRole("button", { name: trigger, exact: true }).hover();
    const panel = page.getByRole("dialog", { name, exact: true });
    await expect(panel.getByRole("link").first()).toBeVisible();
    await solidWithoutTooltips(panel);
    await page.mouse.move(5, 5);
    await expect(panel).toBeHidden();
  }
  // A reply's author: the card, its facts loaded, says nothing in a tooltip.
  await page.locator(".replies .comment").first().getByRole("link", { name: "Bob" }).first().hover();
  const card = page.locator(".popover-panel.profile-card");
  await expect(card.locator(".vote")).toHaveText("Voted yes");
  await expect(card.locator(".share-list li")).toHaveCount(2);
  await solidWithoutTooltips(card);
  await expect(card.locator(".share-list li").first()).toHaveAttribute("aria-description", "30 transactions sent");
  await expect(card.locator(".vote")).toHaveAttribute("aria-description", /stay/i);
  await expect(card.getByRole("link", { name: "What was the NS vote? (docs/VOTE.md)" })).toHaveCount(1);
});

test("at 390, a tapped popover on a post is solid, with no native tooltip inside", async ({ browser }, testInfo) => {
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  const page = await context.newPage();
  await open(page);
  await page.locator(".post .engagement").getByRole("button", { name: "2 replies: preview", exact: true }).tap();
  const replies = page.getByRole("dialog", { name: "Replies", exact: true });
  await expect(replies.getByRole("link").first()).toBeVisible();
  await solidWithoutTooltips(replies);
  const box = (await replies.boundingBox())!;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(390);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await context.close();
});
