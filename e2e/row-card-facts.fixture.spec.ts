// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";

/*
 * The people of a likes or replies preview have their votes and wallets
 * preloaded once the reader shows interest in the list, so a card opened
 * from a row shows them from its first visible frame: never loading, and
 * never growing when they come.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const person = (fid: number) => ({ fid, username: `user${fid}`, display_name: `Person ${fid}` });
const alice = { fid: 42, username: "alice", display_name: "Alice" };
// Three casts, each liked by ten people of its own.
const casts = [0, 1, 2].map((i) => ({ hash: hex(i + 1), author: alice, text: `Cast ${i + 1}`, ts: ts - i * 60, likes: 10, replies: 2 }));
const likers = (i: number) => Array.from({ length: 10 }, (_, j) => person(100 + i * 10 + j));
const cards = Object.fromEntries(
  Array.from({ length: 40 }, (_, k) => [String(100 + k), { bio: `Bio ${100 + k}`, followers: 10 + k, following: 3, registered_at: 1759222921 }]),
);
const bob = { fid: 7, username: "bob", display_name: "Bob" };
const carol = { fid: 8, username: "carol", display_name: "Carol" };
const replies = [
  { hash: hex(20), author: bob, text: "First reply", ts: ts - 30, likes: 2, parent_hash: casts[0].hash },
  { hash: hex(21), author: carol, text: "Second reply", ts: ts - 40, likes: 1, parent_hash: casts[0].hash },
  { hash: hex(22), author: bob, text: "Bob again", ts: ts - 50, likes: 0, parent_hash: casts[0].hash },
];
const replyCards = {
  "7": { bio: "Replies a lot.", followers: 5, following: 1, registered_at: 1759222921 },
  "8": { bio: "Replies less.", followers: 6, following: 2, registered_at: 1759222921 },
};

/**
 * The facts endpoints answer after `delay` ms, and not before `gate` opens.
 * Every read is logged by kind and fid as it is asked for, and in `served`
 * once answered.
 */
async function serve(page: Page, { delay = 120, gate = Promise.resolve() }: { delay?: number; gate?: Promise<void> } = {}) {
  const reads = Object.assign([] as string[], { served: [] as string[] });
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : casts, notes: {}, meta } });
  });
  await page.route("**/api/v1/reactions?*", (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash");
    const i = casts.findIndex((c) => c.hash === hash);
    return route.fulfill({ json: { hash, kind: "likes", users: likers(i), more: false, cards, meta } });
  });
  // Hovering a card prefetches its post, whose thread holds the replies.
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
    thread: { cast: casts[0], children: replies.map((reply) => ({ cast: reply })) }, notes: { users: replyCards }, meta,
  } }));
  await page.route("**/api/v1/replies?*", (route) =>
    route.fulfill({ json: { hash: casts[0].hash, replies, notes: { users: replyCards }, meta } }),
  );
  await page.route("**/api/v1/vote?*", async (route) => {
    const fid = Number(new URL(route.request().url()).searchParams.get("fid"));
    reads.push(`vote ${fid}`);
    await gate;
    await new Promise((r) => setTimeout(r, delay));
    await route.fulfill({ json: { fid, onchain: fid % 2 ? "stay" : "no_vote", app: "opted_in", meta } });
    reads.served.push(`vote ${fid}`);
  });
  await page.route("**/api/v1/wallets?*", async (route) => {
    const fid = Number(new URL(route.request().url()).searchParams.get("fid"));
    reads.push(`wallets ${fid}`);
    await gate;
    await new Promise((r) => setTimeout(r, delay));
    await route.fulfill({ json: {
      fid, addresses: [{ address: hex(fid), kind: "evm", primary: true }],
      chains: [
        { chain: "base", name: "Base", kind: "evm", count: 30, percent: 75 },
        { chain: "ethereum", name: "Ethereum", kind: "evm", count: 10, percent: 25 },
      ],
      total: 40, sources: [], fetched_at: Date.now(),
    } });
    reads.served.push(`wallets ${fid}`);
  });
  // Every frame: a visible card must not show a loading vote or wallet bar,
  // and must keep its height once visible. Each card's first moment is
  // noted too (whether it was out of sight, waiting), and when its facts
  // were in and when it came into sight, in the page's own time.
  await page.addInitScript(() => {
    const seen = ((window as any).__cards = { loading: 0, grew: 0, visibleFrames: 0, cards: {} as Record<string, Card> });
    type Card = { born: number; settling: boolean; factsIn: number | null; shown: number | null };
    const card = (panel: Element) =>
      (seen.cards[panel.getAttribute("aria-label") ?? ""] ??= {
        born: performance.now(), settling: !!panel.querySelector(".profile-card-body.settling"), factsIn: null, shown: null,
      });
    new MutationObserver(() => document.querySelectorAll(".popover-panel.profile-card").forEach(card))
      .observe(document, { childList: true, subtree: true });
    let height = new WeakMap<Element, number>();
    const frame = () => {
      for (const panel of document.querySelectorAll(".popover-panel.profile-card")) {
        const noted = card(panel);
        const now = performance.now();
        if (noted.factsIn === null && !panel.querySelector(".vote-loading, .share-bar.loading")) noted.factsIn = now;
        if (Number(getComputedStyle(panel).opacity) < 0.99) continue;
        noted.shown ??= now;
        seen.visibleFrames++;
        if (panel.querySelector(".vote-loading, .share-bar.loading")) seen.loading++;
        const h = panel.getBoundingClientRect().height;
        const before = height.get(panel);
        if (before !== undefined && Math.abs(h - before) > 0.5) seen.grew++;
        height.set(panel, h);
      }
      requestAnimationFrame(frame);
    };
    requestAnimationFrame(frame);
  });
  return reads;
}

const people = (reads: string[], fids: number[]) => reads.filter((r) => fids.includes(Number(r.split(" ")[1])));
const votes = (reads: string[], fids: number[]) =>
  people(reads, fids).filter((r) => r.startsWith("vote")).map((r) => Number(r.split(" ")[1]));
/** A gate the facts wait at until the test opens it. */
const held = () => {
  let open!: () => void;
  const gate = new Promise<void>((resolve) => (open = resolve));
  return { gate, open };
};
type Seen = {
  loading: number; grew: number; visibleFrames: number;
  cards: Record<string, { born: number; settling: boolean; factsIn: number | null; shown: number | null }>;
};
const cardSeen = (page: Page) => page.evaluate(() => (window as any).__cards as Seen);

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true,
  });
  return { context, page: await context.newPage() };
}

test("a card opened from Liked by is complete from its first frame, its people preloaded in order", async ({ page }) => {
  const reads = await serve(page);
  await page.goto("/");
  const first = page.locator(".cast-card:not(.skeleton)").first();
  await first.locator(".stat-likes").hover();
  const list = page.getByRole("dialog", { name: "Liked by" });
  await expect(list.locator(".who-list li")).toHaveCount(10);
  const fids = likers(0).map((p) => p.fid);
  // Only showing the list loads nothing of its people.
  await page.waitForTimeout(400);
  expect(people(reads, fids)).toEqual([]);
  // The pointer comes into the list: every row's vote and wallets load, top
  // to bottom, once.
  await list.locator(".who-list").hover({ position: { x: 4, y: 4 } });
  await expect.poll(() => people(reads.served, fids).length, { timeout: 15_000 }).toBe(20);
  expect(votes(reads, fids)).toEqual(fids);
  expect(new Set(people(reads, fids)).size).toBe(20);
  // A row's card is complete from the frame it shows.
  for (const name of ["Person 102", "Person 108"]) {
    await list.getByRole("link", { name: new RegExp(name) }).hover();
    const card = page.getByRole("dialog", { name: new RegExp(`Profile: ${name}`) });
    await expect(card.locator(".vote")).toHaveText(/Didn't vote|Voted/);
    await expect(card.locator(".share-list")).toContainText("Ethereum");
    await expect(card).toHaveCSS("opacity", "1");
  }
  await page.waitForTimeout(300);
  const seen = await cardSeen(page);
  expect(seen.visibleFrames).toBeGreaterThan(0);
  expect(seen.loading, "a visible card showed a loading vote or wallet bar").toBe(0);
  expect(seen.grew, "a visible card changed height").toBe(0);
  expect(new Set(people(reads, fids)).size).toBe(20);
});

test("a card opened before its facts are in waits for them out of sight", async ({ page }) => {
  const { gate, open } = held();
  const reads = await serve(page, { gate });
  await page.goto("/");
  await page.locator(".cast-card:not(.skeleton)").first().locator(".stat-likes").hover();
  const list = page.getByRole("dialog", { name: "Liked by" });
  await expect(list.locator(".who-list li")).toHaveCount(10);
  // Straight onto a name: its card opens while the facts are held.
  const name = "Profile: Person 103 (@user103)";
  await list.getByRole("link", { name: /Person 103/ }).hover();
  await page.waitForFunction((name) => (window as any).__cards.cards[name], name);
  open();
  const card = page.getByRole("dialog", { name });
  await expect(card.locator(".vote")).toHaveText("Voted yes");
  await expect(card.locator(".share-list")).toContainText("Base");
  await expect(card).toHaveCSS("opacity", "1");
  const seen = await cardSeen(page);
  const noted = seen.cards[name];
  expect(noted.settling, "the card was in sight before its facts came").toBe(true);
  // It came into sight once its facts were in, or after the half second it
  // waits for them, whichever came first (a slow machine can take longer
  // than that to open the gate).
  expect(noted.shown!).toBeGreaterThanOrEqual(Math.min(noted.factsIn!, noted.born + 450));
  if (noted.factsIn! - noted.born < 450) {
    expect(seen.loading, "a visible card showed a loading vote or wallet bar").toBe(0);
    expect(seen.grew, "a visible card changed height").toBe(0);
  }
  // The opened row's reads went ahead of the queue; the rest kept its order.
  const fids = likers(0).map((p) => p.fid);
  await expect.poll(() => people(reads.served, fids).length, { timeout: 15_000 }).toBe(20);
  expect(votes(reads, fids).filter((fid) => fid !== 103)).toEqual(fids.filter((fid) => fid !== 103));
  expect(votes(reads, fids).indexOf(103)).toBeLessThan(4);
  expect(people(reads, fids)).toHaveLength(20);
  expect(new Set(people(reads, fids)).size).toBe(20);
});

test("a card whose facts are slow still shows after half a second, then fills in", async ({ page }) => {
  const { gate, open } = held();
  await serve(page, { gate });
  await page.goto("/");
  await page.locator(".cast-card:not(.skeleton)").first().locator(".stat-likes").hover();
  const list = page.getByRole("dialog", { name: "Liked by" });
  await list.getByRole("link", { name: /Person 100/ }).hover();
  const card = page.getByRole("dialog", { name: "Profile: Person 100 (@user100)" });
  // The facts never came: the card shows anyway, saying they're loading.
  await expect(card).toHaveCSS("opacity", "1", { timeout: 2_000 });
  await expect(card.locator(".share-bar.loading")).toHaveCount(1);
  open();
  await expect(card.locator(".share-list")).toContainText("Base");
  await expect(card.locator(".vote")).toHaveText("Didn't vote");
});

test("a list the pointer only passes, without coming into it, preloads nothing", async ({ page }) => {
  const reads = await serve(page);
  await page.goto("/");
  const first = page.locator(".cast-card:not(.skeleton)").first();
  await first.locator(".stat-likes").hover();
  await expect(page.getByRole("dialog", { name: "Liked by" }).locator(".who-list li")).toHaveCount(10);
  await page.mouse.move(5, 600);
  await expect(page.getByRole("dialog", { name: "Liked by" })).toHaveCount(0);
  await page.waitForTimeout(500);
  expect(people(reads, likers(0).map((p) => p.fid))).toEqual([]);
});

test("at most 20 people of opened lists are preloaded per five minutes", async ({ page }) => {
  const reads = await serve(page, { delay: 20 });
  await page.goto("/");
  for (const i of [0, 1, 2]) {
    const card = page.locator(`.cast-card[data-hash="${casts[i].hash}"]`);
    await card.scrollIntoViewIfNeeded();
    await card.locator(".stat-likes").hover();
    const list = page.getByRole("dialog", { name: "Liked by" });
    await expect(list.locator(".who-list li")).toHaveCount(10);
    // Into the list, onto its title, then away.
    await list.locator(".who-list").hover({ position: { x: 4, y: 4 } });
    await page.waitForTimeout(800);
    await page.mouse.move(5, 5);
    await expect(list).toHaveCount(0);
  }
  const all = [0, 1, 2].flatMap((i) => likers(i).map((p) => p.fid));
  await expect.poll(() => new Set(people(reads, all).map((r) => r.split(" ")[1])).size).toBe(20);
  await page.waitForTimeout(800);
  expect(new Set(people(reads, all).map((r) => r.split(" ")[1])).size).toBe(20);
  // The first two lists, whole; the third, none.
  expect(people(reads, likers(2).map((p) => p.fid))).toEqual([]);
});

test("a replies preview preloads its authors, each once", async ({ page }) => {
  const reads = await serve(page);
  await page.goto("/");
  await page.locator(".cast-card:not(.skeleton)").first().locator(".stat-replies").hover();
  const panel = page.getByRole("dialog", { name: "Replies", exact: true });
  await expect(panel.locator(".reply-items > li")).toHaveCount(3);
  await panel.locator(".reply-item").first().hover();
  await expect.poll(() => people(reads.served, [7, 8]).length).toBe(4);
  await panel.locator(".reply-author-link").nth(1).hover();
  const card = page.getByRole("dialog", { name: "Profile: Carol (@carol)" });
  await expect(card.locator(".share-list")).toContainText("Base");
  await expect(card).toHaveCSS("opacity", "1");
  await page.waitForTimeout(300);
  expect(people(reads, [7, 8]).sort()).toEqual(["vote 7", "vote 8", "wallets 7", "wallets 8"]);
  const seen = await cardSeen(page);
  expect(seen.visibleFrames).toBeGreaterThan(0);
  expect(seen.loading).toBe(0);
  expect(seen.grew).toBe(0);
});

test("on a phone, a tapped list preloads its people, and a tapped name's card is complete", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  const reads = await serve(page);
  await page.goto("/");
  await page.locator(".cast-card:not(.skeleton)").first().locator(".stat-likes").tap();
  const list = page.getByRole("dialog", { name: "Liked by" });
  await expect(list.locator(".who-list li")).toHaveCount(10);
  const fids = likers(0).map((p) => p.fid);
  // A tap is interest: the people load at once, before any name is tapped.
  await expect.poll(() => people(reads.served, fids).length, { timeout: 15_000 }).toBe(20);
  await list.getByRole("link", { name: /Person 104/ }).tap();
  const card = page.getByRole("dialog", { name: "Profile: Person 104 (@user104)" });
  await expect(card.locator(".vote")).toBeVisible();
  await expect(card.locator(".share-list")).toContainText("Base");
  await expect(card).toBeInViewport({ ratio: 1 });
  await expect(page).toHaveURL(/\/$/);
  await page.waitForTimeout(500);
  const seen = await cardSeen(page);
  expect(seen.visibleFrames).toBeGreaterThan(0);
  expect(seen.loading).toBe(0);
  expect(seen.grew).toBe(0);
  await context.close();
});
