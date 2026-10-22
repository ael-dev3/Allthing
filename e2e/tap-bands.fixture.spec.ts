// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";
import { appCame } from "./helpers/app";

/*
 * On a phone, the smallest targets take a fingertip too, as the others do
 * (e2e/tap-targets.fixture.spec.ts): a reply's collapse toggle (22 px), the
 * "?" beside a profile's facts (14 px) and the apps under the front page's
 * bar (19 px tall) each take a tap in a band 44 px tall, centred on them,
 * and no band meets another target's: room is made where there was none.
 * With a mouse, they are as they were.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const bob = { fid: 7, username: "bob", display_name: "Bob" };
const post = { hash: hex(1), author: alice, text: "A cast with replies", ts, likes: 3, recasts: 1, replies: 4 };
// Replies with no replies of their own, one after another (each one's
// actions right above the next one's toggle), and one answered, whose
// answer's toggle is right under its actions.
const reply = (n: number, author: object, text: string, children: object[] = [], replies = 0) =>
  ({ cast: { hash: hex(n), author, text, ts: ts + n * 60, likes: 1, recasts: 0, replies, parent_hash: post.hash }, children });
const thread = {
  cast: post,
  children: [
    reply(2, bob, "A first reply, with none of its own."),
    reply(3, alice, "A second, with none either."),
    reply(4, bob, "A third, answered.", [reply(5, alice, "The answer.", [reply(6, bob, "And its answer.")], 1)], 1),
  ],
};
// The front page's apps: 15 casts from the Farcaster app, 5 from Base App,
// 2 from another, and one each from three small ones, which make up "Other apps".
const pool = Array.from({ length: 25 }, (_, i) => ({ hash: hex(100 + i), author: i % 2 ? alice : bob, text: `Cast ${i}`, ts: ts - i * 60, likes: 25 - i, recasts: 0, replies: 0 }));
const appOf = (i: number) => (i < 15 ? 9152 : i < 20 ? 309857 : i < 22 ? 1234 : i < 23 ? 5555 : i < 24 ? 6666 : 7777);
const apps = {
  "9152": { fid: 9152, username: "warpcast", display_name: "Warpcast" },
  "309857": { fid: 309857, username: "baseapp.base.eth", display_name: "Base App" },
  "1234": { fid: 1234, username: "otherclient", display_name: "Other client" },
  "5555": { fid: 5555, username: "fourth", display_name: "Fourth app" },
  "6666": { fid: 6666, username: "small", display_name: "Small app" },
  "7777": { fid: 7777, username: "tiny", display_name: "Tiny app" },
};
const to = Math.floor(Date.now() / 3_600_000) * 3_600;
const apps24h = {
  to, counted: to + 120, active: 6174, unknown: 0, none: 14,
  apps: [{ app: 9152, active: 4812 }, { app: 309857, active: 1203 }, { app: 5555, active: 120 }, { app: 6666, active: 37 }, { app: 1234, active: 1 }],
};
const address = "0x76d0e7a13248945ee9f808b4a472262b28778942";

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    const origins = Object.fromEntries(pool.map((c, i) => [c.hash, { signer: "0x01", app_fid: appOf(i) }]));
    return route.fulfill({ json: { pool: later ? [] : pool, notes: { origins, apps, users: { "42": { bio: "Alice's bio", followers: 9, following: 2 } } }, meta, ...(later ? {} : { apps_24h: apps24h }) } });
  });
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: { thread, notes: {}, meta } }));
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: {
    user: { ...alice, bio: "Reads conversations.", follower_count: 12, following_count: 3, registered_at: 1700000000, eth_addresses: [address] },
    pool: [post], notes: {}, meta,
  } }));
  await page.route("**/api/v1/vote?*", (route) => route.fulfill({ json: { fid: 42, onchain: "stay", meta } }));
  await page.route("**/api/v1/wallets?*", (route) => route.fulfill({ json: {
    fid: 42, addresses: [{ address, kind: "evm", primary: true }],
    chains: [{ chain: "base", name: "Base", kind: "evm", count: 120, capped: false, percent: 70 }, { chain: "ethereum", name: "Ethereum", kind: "evm", count: 50, capped: false, percent: 30 }],
    total: 170, sources: [], fetched_at: Date.now(),
  } }));
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true,
  });
  return { context, page: await context.newPage() };
}

/** For the screenshots only: each tap band, outlined. */
const outlined = [".comment-head .toggle", ".fact-help", ".client-legend a", ".client-other", ".seg-pop .seg", ".sort-tabs a", ".engagement .act", ".engagement .popover-trigger", ".reply-button"]
  .map((target) => `${target}::after`).join(", ") + " { outline: 1px dashed #d6247a; outline-offset: -1px; }";

/**
 * Each of `targets`' tap bands (its box, with its `::after` band), and
 * every other target's near it, on the same layer (a popover's panel is
 * over the page, the sticky header over what scrolls under it): the pairs
 * whose bands meet.
 */
async function meeting(page: Page, targets: string) {
  return page.evaluate((targets) => {
    const band = (el: Element) => {
      const r = el.getBoundingClientRect();
      let [x, y, right, bottom] = [r.left, r.top, r.right, r.bottom];
      const after = getComputedStyle(el, "::after");
      if (after.content !== "none" && after.position === "absolute") {
        const own = getComputedStyle(el);
        const m = after.transform && after.transform !== "none" ? new DOMMatrixReadOnly(after.transform) : new DOMMatrixReadOnly();
        const left = r.left + (parseFloat(own.borderLeftWidth) || 0) + parseFloat(after.left) + m.e;
        const top = r.top + (parseFloat(own.borderTopWidth) || 0) + parseFloat(after.top) + m.f;
        [x, y, right, bottom] = [Math.min(x, left), Math.min(y, top), Math.max(right, left + parseFloat(after.width)), Math.max(bottom, top + parseFloat(after.height))];
      }
      return { x, y, right, bottom };
    };
    const layer = (el: Element) => el.closest(".popover-panel") ?? el.closest(".site-header") ?? document.body;
    const shown = (el: Element) => (el as HTMLElement).offsetParent !== null || getComputedStyle(el).position === "fixed";
    const all = [...document.querySelectorAll("a[href], button, select, .seg-pop .seg")].filter(shown);
    const mine = [...document.querySelectorAll(targets)].filter(shown);
    const name = (el: Element) => `${el.tagName.toLowerCase()}.${[...el.classList].join(".")} "${(el.textContent ?? "").trim().slice(0, 20)}"`;
    const met: string[] = [];
    for (const a of mine) {
      const A = band(a);
      for (const b of all) {
        if (a === b || a.contains(b) || b.contains(a) || layer(a) !== layer(b)) continue;
        const B = band(b);
        const [w, h] = [Math.min(A.right, B.right) - Math.max(A.x, B.x), Math.min(A.bottom, B.bottom) - Math.max(A.y, B.y)];
        if (w > 0.5 && h > 0.5) met.push(`${name(a)} meets ${name(b)} (${w.toFixed(1)} × ${h.toFixed(1)})`);
      }
    }
    return met;
  }, targets);
}

/** A target's tap band, and whether taps at its middle, above, below and to each side land on it. */
async function takes(target: Locator, { dx = 0 }: { dx?: number } = {}) {
  return target.evaluate((el, dx) => {
    const after = getComputedStyle(el, "::after");
    const box = el.getBoundingClientRect();
    const [x, y] = [box.x + box.width / 2, box.y + box.height / 2];
    const lands = (px: number, py: number) => {
      const hit = document.elementFromPoint(px, py);
      return hit === el || el.contains(hit);
    };
    return {
      band: [parseFloat(after.width), parseFloat(after.height)],
      middle: lands(x, y),
      above: lands(x, y - 20),
      below: lands(x, y + 20),
      sides: dx ? lands(x - dx, y) && lands(x + dx, y) : true,
    };
  }, dx);
}

test("at 390 on a touch screen, a reply's toggle takes a 44 px tap, clear of the actions above it", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await serve(page);
  await page.goto(`/p/${post.hash}`);
  const toggles = page.locator(".comment-head .toggle");
  await expect(toggles).toHaveCount(5);
  for (let i = 0; i < 5; i++) {
    const toggle = toggles.nth(i);
    // In the middle of the screen, so its band, above and below, is on it.
    await toggle.evaluate((el) => el.scrollIntoView({ block: "center" }));
    const t = await takes(toggle);
    expect(t.band[1], `toggle ${i}: band height`).toBeGreaterThanOrEqual(44);
    expect(t.band[0], `toggle ${i}: band as wide as it is`).toBeGreaterThanOrEqual(20);
    expect([t.middle, t.above, t.below], `toggle ${i}: a tap on it, above and below`).toEqual([true, true, true]);
  }
  await page.locator(".comments").scrollIntoViewIfNeeded();
  expect(await meeting(page, ".comment-head .toggle")).toEqual([]);
  await page.evaluate(() => window.scrollBy(0, 400));
  expect(await meeting(page, ".comment-head .toggle")).toEqual([]);
  // A tap above a toggle, in its band, collapses its reply.
  const last = toggles.nth(4);
  const box = (await last.boundingBox())!;
  await page.touchscreen.tap(box.x + box.width / 2, box.y + box.height / 2 - 18);
  await expect(last).toHaveAttribute("aria-expanded", "false");
  await page.screenshot({ path: testInfo.outputPath("tap-bands-390-replies.png"), style: outlined });
  await context.close();
});

test("at 390 on a touch screen, the \"?\" beside a fact takes a tap 44 px tall and 36 wide", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await serve(page);
  await page.goto("/u/alice");
  const helps = page.locator(".profile .fact-help");
  await expect(helps).toHaveCount(2);
  for (let i = 0; i < 2; i++) {
    const help = helps.nth(i);
    await help.evaluate((el) => el.scrollIntoView({ block: "center" }));
    const t = await takes(help, { dx: 15 });
    expect(t.band, `"?" ${i}: its band`).toEqual([36, 44]);
    expect([t.middle, t.above, t.below, t.sides], `"?" ${i}: a tap on it, above, below and beside`).toEqual([true, true, true, true]);
    expect(await meeting(page, ".fact-help")).toEqual([]);
  }
  await page.screenshot({ path: testInfo.outputPath("tap-bands-390-profile.png"), style: outlined });
  // A profile card has two, a line apart, whose bands don't meet.
  await page.goto("/");
  await page.locator("#app .cast-card .user-chip .author").first().tap();
  const card = page.locator(".popover-panel.profile-card");
  await expect(card.locator(".fact-help")).toHaveCount(2);
  for (let i = 0; i < 2; i++) {
    const t = await takes(card.locator(".fact-help").nth(i), { dx: 15 });
    expect([t.middle, t.above, t.below, t.sides], `card "?" ${i}`).toEqual([true, true, true, true]);
  }
  expect(await meeting(page, ".profile-card .fact-help")).toEqual([]);
  await page.screenshot({ path: testInfo.outputPath("tap-bands-390-card.png"), style: outlined });
  await context.close();
});

test("at 390 on a touch screen, the apps under the front page's bar take a 44 px tap, clear of the bar and of each other", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await serve(page);
  await page.goto("/");
  // The app's legend, not the boot screen's, which it takes the place of.
  await appCame(page);
  const legend = page.locator(".client-legend").locator("a, .client-other");
  await expect(legend).toHaveCount(5);
  // Two rows of them on a phone.
  const rows = new Set(await legend.evaluateAll((items) => items.map((i) => Math.round(i.getBoundingClientRect().top))));
  expect(rows.size).toBe(2);
  for (let i = 0; i < 5; i++) {
    const t = await takes(legend.nth(i));
    expect(t.band[1], `app ${i}: band height`).toBeGreaterThanOrEqual(44);
    expect([t.middle, t.above, t.below], `app ${i}: a tap on it, above and below`).toEqual([true, true, true]);
  }
  expect(await meeting(page, ".client-legend a, .client-other")).toEqual([]);
  // The bar's segments still take their own taps.
  const segment = page.locator(".seg-pop .seg").first();
  const s = (await segment.boundingBox())!;
  expect(await page.evaluate(([x, y]) => !!document.elementFromPoint(x, y)?.closest(".seg-pop"), [s.x + s.width / 2, s.y + s.height + 4])).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("tap-bands-390-apps.png"), style: outlined });
  await context.close();
});

test("with a mouse, the toggle, the \"?\" and the apps are as they were", async ({ page }, testInfo) => {
  test.skip(Boolean(testInfo.project.use.hasTouch), "a mouse");
  await serve(page);
  await page.goto("/");
  await appCame(page);
  const app = page.locator(".client-legend a").first();
  await expect(app).toBeVisible();
  expect(await app.evaluate((el) => getComputedStyle(el, "::after").content)).toBe("none");
  expect(await page.locator(".client-legend").evaluate((el) => [getComputedStyle(el).marginTop, getComputedStyle(el).rowGap])).toEqual(["7px", "4px"]);
  await page.goto(`/p/${post.hash}`);
  const toggle = page.locator(".comment-head .toggle").first();
  await expect(toggle).toBeVisible();
  expect(await toggle.evaluate((el) => getComputedStyle(el, "::after").content)).toBe("none");
  expect(await page.locator(".comment").first().evaluate((el) => getComputedStyle(el).paddingTop)).toBe("10px");
  await page.goto("/u/alice");
  const help = page.locator(".profile .fact-help").first();
  await expect(help).toBeVisible();
  expect(await help.evaluate((el) => getComputedStyle(el, "::after").content)).toBe("none");
});
