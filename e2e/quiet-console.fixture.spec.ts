// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type ConsoleMessage, type Page } from "@playwright/test";
import { appCame } from "./helpers/app";
import { signIn } from "./helpers/farcaster";

/*
 * Nothing a reader does fails where it didn't have to, so the console stays
 * quiet: signed out, pointing at Cast, Reply or Quote asks for no GIF key
 * (the Worker gives one only to a reader signed in); a preview batch the
 * Worker couldn't read comes as an empty batch, kept nowhere, and is asked
 * for again; and a post's like and recast are settled by the reader's
 * latest, when they reach back to the cast, without asking the node about
 * a reaction that isn't there (which it answers with a 400). No native
 * title tooltips are left, nor a help cursor with nothing to show.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const now = Math.floor(Date.now() / 1000);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const NODE = "https://haatz.quilibrium.com/v1";
const cors = { "access-control-allow-origin": "*" };
const EPOCH = 1_609_459_200;
// A recent cast and one from a year ago, each liked twice.
const recent = { hash: hex(1), author: alice, text: "A recent cast", ts: now - 3600, likes: 2, recasts: 1, replies: 0 };
const old = { hash: hex(2), author: alice, text: "A cast from a year ago", ts: now - 365 * 86_400, likes: 2, recasts: 1, replies: 0 };
const people = [{ fid: 7, username: "bob", display_name: "Bob" }, { fid: 8, username: "carol", display_name: "Carol" }];

/**
 * Errors in the console, but those the stand-ins' own 404s cause, and a
 * page's code still coming when a test loads the next page: a reader's
 * navigation stays in the app, which aborts nothing.
 */
function errors(page: Page) {
  const seen: string[] = [];
  page.on("console", (message: ConsoleMessage) => {
    const text = message.text();
    if (message.type() !== "error" || /Fixture endpoint|404/.test(text)) return;
    if (/WebAssembly compilation aborted|Failed to fetch[\s\S]*allthing-split\.js/.test(text)) return;
    seen.push(text);
  });
  return seen;
}

async function serve(page: Page, { liked = 2 }: { liked?: number } = {}) {
  const calls = { gifKey: 0, engagement: 0, reactionById: [] as string[] };
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [recent, old], notes: {}, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash");
    return route.fulfill({ json: { thread: { cast: hash === old.hash ? old : recent, children: [] }, notes: {}, meta } });
  });
  await page.route("**/api/v1/gifs/key", (route) => {
    calls.gifKey += 1;
    return route.fulfill({ json: { key: "fixture-key" } });
  });
  // As the Worker answers a batch it couldn't read: empty, kept nowhere.
  await page.route("**/api/v1/engagement?*", (route) => {
    calls.engagement += 1;
    return route.fulfill({ headers: { "cache-control": "no-store", "x-allthing-cache": "skipped" }, json: { previews: [], meta } });
  });
  await page.route("**/api/v1/reactions?*", (route) => route.fulfill({ json: { hash: recent.hash, kind: "likes", users: people, more: false, meta } }));
  await page.route("**/api/v1/signer", (route) => route.fulfill({ status: 404, json: { error: "Liking and recasting from Allthing isn't set up on this server" } }));
  // The reader's latest likes: `liked` of them, an hour apart, the newest an hour ago.
  await page.route(`${NODE}/reactionsByFid?*`, (route) => {
    const url = new URL(route.request().url());
    const size = Number(url.searchParams.get("pageSize"));
    const messages = url.searchParams.get("reaction_type") === "Like"
      ? Array.from({ length: liked }, (_, i) => ({
        data: {
          type: "MESSAGE_TYPE_REACTION_ADD", fid: me.fid, timestamp: now - EPOCH - 3600 * (i + 1),
          reactionBody: { type: "REACTION_TYPE_LIKE", targetCastId: { fid: 9, hash: hex(100 + i) } },
        },
      }))
      : [];
    expect(size).toBe(100);
    return route.fulfill({ headers: cors, json: { messages } });
  });
  await page.route(`${NODE}/reactionById?*`, (route) => {
    calls.reactionById.push(new URL(route.request().url()).searchParams.get("target_hash")!);
    return route.fulfill({ status: 400, headers: cors, json: { error: "Failed to get reaction", error_detail: 'status: NotFound, message: "not found"' } });
  });
  return calls;
}

test("signed out, Cast, Reply and Quote ask for no GIF key, and nothing fails", async ({ page }) => {
  const seen = errors(page);
  const calls = await serve(page);
  await page.goto("/");
  // Cast is the app's, with its handlers.
  await appCame(page);
  const cast = page.getByRole("button", { name: "Write a cast" }).first();
  await cast.hover();
  await cast.focus();
  // Pointing at Cast fetches the composer's code: it comes before the page is left.
  await page.waitForLoadState("networkidle");
  await page.goto(`/p/${recent.hash}`);
  for (const name of ["Reply to @alice", "Quote @alice's cast"]) {
    const button = page.getByRole("button", { name });
    await button.hover();
    await button.focus();
  }
  // The composer, signed out: no GIF button, and no key asked for.
  await page.getByRole("button", { name: "Reply to @alice" }).click();
  await expect(page.getByRole("dialog", { name: "Reply" })).toBeVisible();
  await page.waitForTimeout(500);
  expect(calls.gifKey).toBe(0);
  await expect(page.getByRole("button", { name: "Add a GIF" })).toHaveCount(0);
  expect(seen).toEqual([]);
});

test("signed in, pointing at Cast asks for the GIF key, and the composer has its GIF button", async ({ page }) => {
  await signIn(page, me);
  const calls = await serve(page);
  await page.goto("/");
  await appCame(page);
  const cast = page.getByRole("button", { name: "Write a cast" }).first();
  // Pointed at (again, until the page knows who is signed in).
  await expect.poll(async () => {
    await page.mouse.move(0, 0);
    await cast.hover();
    return calls.gifKey;
  }, { timeout: 15_000 }).toBeGreaterThan(0);
  await cast.click();
  await expect(page.getByRole("dialog", { name: "New cast" }).getByRole("button", { name: "Add a GIF" })).toBeVisible();
});

test("a preview batch the Worker couldn't read is empty, logs nothing, and is asked for again", async ({ page }) => {
  const seen = errors(page);
  const calls = await serve(page);
  await page.goto("/");
  await appCame(page);
  await expect.poll(() => calls.engagement).toBe(1);
  // Its previews ask for themselves as they open.
  await page.locator(`.cast-card[data-hash="${recent.hash}"] .stat-likes .popover-trigger`).hover();
  await expect(page.getByRole("dialog", { name: "Liked by" })).toContainText("@bob");
  // Back on the front page, the batch is asked for again: it wasn't kept.
  // (The post page loads first: leaving it while its module compiles would
  // log the abort.)
  await page.goto(`/p/${recent.hash}`);
  await expect(page.locator(".post .post-text")).toHaveText(recent.text);
  await page.goBack();
  await expect.poll(() => calls.engagement).toBe(2);
  expect(seen).toEqual([]);
});

test("signed in, a post's like is settled by the reader's latest when they reach back to it", async ({ page }) => {
  const seen = errors(page);
  await signIn(page, me);
  // Two likes in all: every like the reader made is among them.
  const calls = await serve(page, { liked: 2 });
  await page.goto(`/p/${recent.hash}`);
  await expect(page.locator(".post .post-text")).toHaveText(recent.text);
  await page.waitForTimeout(1500);
  await page.goto(`/p/${old.hash}`);
  await expect(page.locator(".post .post-text")).toHaveText(old.text);
  await page.waitForTimeout(1500);
  expect(calls.reactionById).toEqual([]);
  expect(seen).toEqual([]);
});

test("signed in, a post older than the reader's latest hundred likes is asked about", async ({ page }) => {
  await signIn(page, me);
  // A hundred likes, the oldest a hundred hours ago.
  const calls = await serve(page, { liked: 100 });
  await page.goto(`/p/${recent.hash}`);
  await expect(page.locator(".post .post-text")).toHaveText(recent.text);
  await page.waitForTimeout(1500);
  // The recent cast is settled by the hundred; the year-old one is asked about, like and recast.
  expect(calls.reactionById).toEqual([]);
  await page.goto(`/p/${old.hash}`);
  await expect.poll(() => calls.reactionById.length).toBe(1);
  // Recasts: none in the latest, so every recast is known there.
  await page.waitForTimeout(800);
  expect(calls.reactionById).toEqual([old.hash]);
});

test("no native tooltips, and no help cursor with nothing to show", async ({ page }) => {
  await signIn(page, me);
  await serve(page);
  await page.route("**/api/v1/channel?*", (route) => route.fulfill({ json: {
    channel: {
      id: "fixtures", name: "Fixtures", parent_url: "https://farcaster.xyz/~/channel/fixtures", follower_count: 1200, member_count: 30,
      external_link: { url: "https://example.com/join", title: "Join" },
    },
    pool: [recent], notes: {}, meta,
  } }));
  await page.route("**/api/v1/channels*", (route) => route.fulfill({ json: { channels: [{ id: "fixtures", name: "Fixtures", parent_url: "https://farcaster.xyz/~/channel/fixtures", follower_count: 1200 }], meta } }));
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: {
    user: { ...alice, follower_count: 12, following_count: 3, eth_addresses: ["0x76d0e7a13248945ee9f808b4a472262b28778942"] }, pool: [recent], notes: {}, meta,
  } }));
  await page.route("**/api/v1/wallets?*", (route) => route.fulfill({ json: {
    fid: 42, addresses: [{ address: "0x76d0e7a13248945ee9f808b4a472262b28778942", kind: "evm", primary: true }],
    chains: [{ chain: "base", name: "Base", kind: "evm", count: 12, capped: false, percent: 100 }], total: 12, sources: [], fetched_at: Date.now(),
  } }));
  const titles = () => page.evaluate(() => [...document.querySelectorAll("[title]")].filter((el) => el.tagName !== "IFRAME").map((el) => el.outerHTML.slice(0, 80)));
  for (const path of ["/", "/c/fixtures", "/channels", "/u/alice", `/p/${recent.hash}`]) {
    await page.goto(path);
    await page.locator("main").first().waitFor();
    await page.waitForTimeout(800);
    expect(await titles(), path).toEqual([]);
  }
  // The profile's wallets, listed.
  await page.goto("/u/alice");
  const wallets = page.getByRole("button", { name: "Show 1 verified wallet", exact: true });
  await wallets.hover();
  await expect(page.getByRole("dialog", { name: "Verified wallets" })).toBeVisible();
  expect(await titles()).toEqual([]);
  // The sort tabs still say what each does, read out.
  await page.goto("/");
  await expect(page.locator(".sort-tabs a").first()).toHaveAttribute("aria-description", "engagement, decaying with age");
  // The composer's clock, signed in where the composer can schedule.
  await page.getByRole("button", { name: "Write a cast" }).first().click();
  await expect(page.getByRole("dialog", { name: "New cast" })).toBeVisible();
  await page.waitForTimeout(500);
  expect(await titles()).toEqual([]);
  await page.keyboard.press("Escape");
  // No help cursor on a card's "via".
  const via = page.locator(".cast-card .origin").first();
  if (await via.count()) expect(await via.evaluate((el) => getComputedStyle(el).cursor)).not.toBe("help");
  expect(await page.evaluate(() => [...document.querySelectorAll("*")].filter((el) => getComputedStyle(el).cursor === "help").length)).toBe(0);
});
