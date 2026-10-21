// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type TestInfo } from "@playwright/test";
import { appCame } from "./helpers/app";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
// A tall cast, so its counts sit low and its Liked by list opens above them.
const tall = {
  hash: hex(1), author: alice, ts: 1792235681, likes: 10, replies: 0,
  text: Array.from({ length: 30 }, (_, i) => `Line ${i + 1}`).join("\n"),
};
const people = Array.from({ length: 10 }, (_, i) => ({ fid: 100 + i, username: `p${i}`, display_name: `Person ${i}` }));
const card = { bio: "Wallets and words.", followers: 120, following: 30, registered_at: 1759222921 };

test("at 880 px, a card from the lowest name of a list above its count stays level with the list", async ({ page }, testInfo: TestInfo) => {
  test.skip(Boolean(testInfo.project.use.hasTouch), "hover needs a mouse");
  await page.setViewportSize({ width: 880, height: 900 });
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [tall], notes: {}, meta } });
  });
  await page.route("**/api/v1/reactions?*", (route) => route.fulfill({ json: {
    hash: tall.hash, kind: "likes", users: people, more: false, meta,
    cards: Object.fromEntries(people.map((p) => [String(p.fid), card])),
  } }));
  await page.route("**/api/v1/vote?*", (route) => route.fulfill({ json: { fid: 109, onchain: "stay", meta } }));
  // Both wallet notes: the card's tallest wallet section.
  await page.route("**/api/v1/wallets?*", (route) => route.fulfill({ json: {
    fid: 109, addresses: [{ address: hex(9), kind: "evm", primary: true }, { address: "So1ana", kind: "solana", primary: false }],
    chains: [
      { chain: "base", name: "Base", kind: "evm", count: 0, capped: false, percent: 0 },
      { chain: "solana", name: "Solana", kind: "solana", count: 0, capped: false, percent: 0, error: "upstream returned HTTP 403" },
    ],
    total: 0, sources: [], fetched_at: Date.now(),
  } }));
  await page.goto("/");
  await appCame(page);
  const post = page.locator(".cast-card:not(.skeleton)").first();
  const likes = post.locator(".stat-likes");
  await likes.scrollIntoViewIfNeeded();
  await likes.hover();
  const list = page.getByRole("dialog", { name: "Liked by" });
  await expect(list).toBeVisible();
  await expect(list).not.toHaveClass(/\bbelow\b/);
  await list.getByRole("link", { name: /Person 9/ }).hover();
  const profile = page.locator(".popover-panel.profile-card");
  await expect(profile.locator(".profile-card-bio")).toHaveText("Wallets and words.");
  // One message; the Solana note folded into a word on the label's line.
  await expect(profile.locator(".profile-card-wallets .share-message")).toHaveText("No activity on the EVM chains counted.");
  await expect(profile.locator(".share-note .visually-hidden")).toHaveText(": Solana couldn't be read just now, so its wallets aren't counted.");
  expect(await profile.locator(".share-note").getAttribute("title")).toBeNull();
  const [listBox, cardBox, postBox] = [await list.boundingBox(), await profile.boundingBox(), await post.boundingBox()];
  // Level with the list's bottom, as the list opened above its count: not
  // hanging below the list, nor past the post the list belongs to.
  expect(Math.abs(cardBox!.y + cardBox!.height - (listBox!.y + listBox!.height))).toBeLessThan(2);
  expect(cardBox!.y + cardBox!.height).toBeLessThanOrEqual(postBox!.y + postBox!.height);
  expect(cardBox!.y).toBeGreaterThanOrEqual(0);
  // Beside it, not over it.
  expect(cardBox!.x >= listBox!.x + listBox!.width || cardBox!.x + cardBox!.width <= listBox!.x).toBe(true);
  // The note's word sits on the label's line, clear of its words.
  const label = await profile.locator(".profile-card-label").evaluate((el) => {
    const range = document.createRange();
    range.selectNodeContents(el);
    const { x, y, width, height } = range.getBoundingClientRect();
    return { x, y, width, height };
  });
  const note = (await profile.locator(".share-note").boundingBox())!;
  expect(Math.abs(note.y + note.height / 2 - (label.y + label.height / 2))).toBeLessThan(2);
  expect(note.x).toBeGreaterThanOrEqual(label.x + label.width + 8);
});
