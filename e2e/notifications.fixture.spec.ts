// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";
import { appCame } from "./helpers/app";
import { signIn } from "./helpers/farcaster";

/*
 * The header's bell and the notifications page, for a signed-in reader, at
 * 1280 and at 390 on a touch screen. `/api/v1/notifications` is a stand-in
 * answering grouped notifications as the Worker would (its grouping is the
 * Worker's, tested in Rust); the browser keeps what the reader has seen
 * (`allthing-notifications-seen`, by account). With E2E_SHOTS set to a
 * folder, the page and the bell are saved there as screenshots too.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in, and so no notifications");

const SHOTS = process.env.E2E_SHOTS;
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const at = Math.floor(Date.now() / 1000);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
/** Each person's picture: their initial on a colour of their own, so the screenshots tell them apart. */
const face = (url: string) => {
  const name = /\/fixture\/([^/]+)\//.exec(url)?.[1] ?? "?";
  const hue = [...name].reduce((h, c) => (h * 31 + c.charCodeAt(0)) % 360, 17);
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" fill="hsl(${hue} 50% 56%)"/>` +
    `<text x="32" y="43" font-family="Arial, sans-serif" font-size="30" font-weight="700" fill="#fff" text-anchor="middle">${name[0].toUpperCase()}</text></svg>`;
};
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" };
const SEEN = "allthing-notifications-seen";

const me = { fid: 1234, username: "reader", display_name: "Reader" };
const person = (fid: number, username: string, display_name: string) =>
  ({ fid, username, display_name, pfp_url: `https://imagedelivery.net/fixture/${username}/original` });
const alice = person(2, "alice", "Alice");
const bob = person(3, "bob", "Bob");
const carol = person(4, "carol", "Carol");
const dan = person(5, "dan", "Dan");
const erin = person(6, "erin", "Erin");
const frank = person(7, "frank", "Frank");
const grace = person(8, "grace", "Grace Hopper");

const mine = { hash: hex(1), excerpt: "Casting from https://allthing.social/ :pepe_smug:" };
/** The first page, as the Worker groups it: newest first. */
const first = {
  groups: [
    { kind: "like", actors: [alice, bob, carol, dan, erin], count: 6, ts: at - 60, cast: mine },
    { kind: "reply", actors: [frank], count: 1, ts: at - 300, cast: { hash: hex(2), excerpt: "Totally agree with this" } },
    { kind: "follow", actors: [grace, alice, bob, carol], count: 4, ts: at - 600 },
    { kind: "mention", actors: [bob], count: 1, ts: at - 3_600, cast: { hash: hex(3), excerpt: "gm @reader, look at this" } },
    { kind: "quote", actors: [carol], count: 1, ts: at - 7_200, mentions_you: true, cast: { hash: hex(4), excerpt: "Quoting @reader's take" } },
    { kind: "recast", actors: [dan, erin], count: 2, ts: at - 172_800, cast: mine },
    { kind: "quote", actors: [erin], count: 1, ts: at - 259_200, cast: { hash: hex(5), excerpt: "Quoted, without a mention" } },
  ],
  next: "eyJiZWZvcmVfdGltZXN0YW1wIjoxODMwMjk1MDJ9",
  newest: at - 60,
  notes: { users: { "2": { bio: "Reads conversations.", followers: 12, following: 3, registered_at: 1759222921 } } },
  meta,
};
/** The page after it, the last. */
const second = {
  groups: [
    { kind: "like", actors: [grace], count: 1, ts: at - 432_000, cast: { hash: hex(6), excerpt: "An older cast of mine" } },
    { kind: "reply", actors: [alice], count: 1, ts: at - 500_000, cast: { hash: hex(7), excerpt: "A reply from last week" } },
  ],
  newest: at - 432_000,
  meta,
};
/** Seen up to here before the visit: the like, the reply and the follows are new. */
const SEEN_BEFORE = at - 1_000;
const sentences = [
  "Alice and 5 others liked your cast",
  "Frank replied to your cast",
  "Grace Hopper and 3 others followed you",
  "Bob mentioned you",
  "Carol quoted your cast",
  "Dan and 1 other recasted your cast",
  "Erin quoted your cast",
];

type Calls = { first: number; more: string[] };

async function serve(page: Page, opts: { failFirst?: number } = {}) {
  const calls: Calls = { first: 0, more: [] };
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    const cast = { hash: hex(9), author: alice, text: "A cast on the front page", ts: at - 30, likes: 1 };
    return route.fulfill({ json: { pool: later ? [] : [cast], notes: {}, meta } });
  });
  await page.route("**/api/v1/notifications**", (route) => {
    expect(route.request().method()).toBe("GET");
    const cursor = new URL(route.request().url()).searchParams.get("cursor");
    if (cursor) {
      calls.more.push(cursor);
      return route.fulfill({ json: second, headers: { "cache-control": "private, no-store" } });
    }
    calls.first += 1;
    if (calls.first <= (opts.failFirst ?? 0)) {
      return route.fulfill({ status: 503, json: { error: "Farcaster data source is busy; backing off", retry_after: 5 } });
    }
    return route.fulfill({ json: first, headers: { "cache-control": "private, no-store" } });
  });
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
    thread: { cast: { hash: hex(1), author: me, text: "Casting from https://allthing.social/ :pepe_smug:", ts: at - 4_000, likes: 6 } },
    notes: {}, meta,
  } }));
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: {
    user: { ...grace, bio: "Programs", follower_count: 3, following_count: 1 }, pool: [], notes: {}, meta,
  } }));
  // Signed in where writes go to the Farcaster app: the browser reads the
  // reader's likes from the node itself.
  await page.route("https://haatz.quilibrium.com/**", (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  await page.route("https://imagedelivery.net/**", (route) => route.fulfill({ contentType: "image/svg+xml", body: face(route.request().url()) }));
  return calls;
}

/** Signed in, having seen notifications up to `seen` in this browser. */
async function ready(page: Page, seen = SEEN_BEFORE, opts: { failFirst?: number } = {}) {
  await signIn(page, me);
  await page.addInitScript(({ key, fid, seen }) => {
    if (localStorage.getItem(key) === null) localStorage.setItem(key, JSON.stringify({ [fid]: seen }));
  }, { key: SEEN, fid: me.fid, seen });
  return serve(page, opts);
}

const bell = (page: Page) => page.getByRole("link", { name: /^Notifications, \d+ unread$/ });
const rows = (page: Page) => page.locator(".notif-list > li.notif:not(.skeleton)");
const shot = async (target: Page | Locator, name: string, testInfo: TestInfo) => {
  const path = SHOTS ? `${SHOTS}/${name}.png` : testInfo.outputPath(`${name}.png`);
  await target.screenshot({ path, animations: "disabled" });
};

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true,
  });
  return { context, page: await context.newPage() };
}

test("the bell counts what's new, and the page groups it as Farcaster does and marks it seen", async ({ page }, testInfo) => {
  const calls = await ready(page);
  await page.goto("/");
  await appCame(page);
  await expect(bell(page)).toHaveAccessibleName("Notifications, 3 unread");
  await expect(bell(page).locator(".bell-badge")).toHaveText("3");
  await expect(bell(page)).toHaveAttribute("href", "/notifications");
  // Beside the account's picture, in the header.
  const header = page.locator(".header-actions");
  await expect(header.locator(":scope > *")).toHaveCount(2);
  await shot(page.locator(".site-header"), "bell-1280", testInfo);

  await page.mouse.move(5, 400);
  await bell(page).click();
  await expect(page).toHaveURL(/\/notifications$/);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Notifications");
  await expect(rows(page)).toHaveCount(7);
  // Each group in a sentence, its first person's name a link to them.
  for (const [i, sentence] of sentences.entries()) {
    await expect(rows(page).nth(i).locator(".notif-line")).toHaveText(sentence);
  }
  const like = rows(page).first();
  await expect(like.locator(".notif-name")).toHaveAttribute("href", "/u/alice");
  await expect(like.locator(".notif-face")).toHaveCount(5);
  await expect(like.getByRole("link", { name: "Dan", exact: true })).toHaveAttribute("href", "/u/dan");
  // The cast it's about, with its Pepe as cards show it, and when.
  await expect(like.locator(".notif-excerpt")).toContainText("Casting from https://allthing.social/");
  await expect(like.locator('.notif-excerpt img[alt=":pepe_smug:"]')).toBeVisible();
  await expect(like.locator("time")).toHaveText(/^\dm ago$/);
  // What came since the last visit is marked new, for screen readers too.
  expect(await rows(page).evaluateAll((els) => els.map((el) => el.classList.contains("new")))).toEqual([true, true, true, false, false, false, false]);
  await expect(like.locator(".notif-open")).toHaveAccessibleName(
    "New: Alice and 5 others liked your cast: Casting from https://allthing.social/ :pepe_smug:",
  );
  await expect(rows(page).nth(3).locator(".notif-open")).toHaveAccessibleName("Bob mentioned you: gm @reader, look at this");
  // Opening the page marked it seen: the bell clears, the rows stay marked.
  await expect(bell(page)).toHaveAccessibleName("Notifications, 0 unread");
  await expect(bell(page).locator(".bell-badge")).toHaveCount(0);
  await expect(bell(page)).toHaveAttribute("aria-current", "page");
  expect(await page.evaluate((key) => JSON.parse(localStorage.getItem(key)!), SEEN)).toEqual({ [me.fid]: first.newest });
  // One request: the bell's, reused by the page.
  expect(calls.first).toBe(1);
  await page.mouse.move(5, 5);
  await shot(page, "notifications-all-1280", testInfo);

  // Load more: the next page, after; the last has no more.
  await page.getByRole("button", { name: "Load more" }).click();
  await expect(rows(page)).toHaveCount(9);
  await expect(rows(page).nth(7).locator(".notif-line")).toHaveText("Grace Hopper liked your cast");
  await expect(rows(page).nth(8).locator(".notif-line")).toHaveText("Alice replied to your cast");
  await expect(page.getByRole("button", { name: "Load more" })).toHaveCount(0);
  expect(calls.more).toEqual([first.next]);

  // After a reload, nothing is new.
  await page.reload();
  await expect(rows(page)).toHaveCount(7);
  await expect(page.locator(".notif.new")).toHaveCount(0);
  await expect(bell(page)).toHaveAccessibleName("Notifications, 0 unread");
});

test("someone else signing in, in another tab, sees none of the last reader's notifications", async ({ page }) => {
  await ready(page);
  const other = { fid: 5678, username: "other", display_name: "Other" };
  const theirs = {
    groups: [{ kind: "like", actors: [person(9, "heidi", "Heidi")], count: 1, ts: at - 120, cast: { hash: hex(8), excerpt: "The other reader's cast" } }],
    newest: at - 120,
    meta,
  };
  // The Worker answers for the session's account: here, the user cookie's.
  let asked = 0;
  await page.route("**/api/v1/notifications**", async (route) => {
    const cookie = (await route.request().headerValue("cookie")) ?? "";
    if (!cookie.includes(encodeURIComponent(`"fid":${other.fid}`))) return route.fallback();
    asked += 1;
    await new Promise((resolve) => setTimeout(resolve, 1_500));
    return route.fulfill({ json: theirs, headers: { "cache-control": "private, no-store" } });
  });
  await page.goto("/notifications");
  await expect(rows(page)).toHaveCount(7);

  // Signed in as someone else in another tab: this one follows.
  await page.evaluate((user) => {
    document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax`;
    new BroadcastChannel("allthing-session").postMessage("changed");
  }, other);
  // While theirs come, placeholders, and nothing of the last reader's.
  await expect(page.locator(".notif.skeleton").first()).toBeVisible();
  await expect(rows(page)).toHaveCount(0);
  await expect(page.getByText("Alice and 5 others liked your cast")).toHaveCount(0);
  await expect(rows(page)).toHaveCount(1);
  await expect(rows(page).first().locator(".notif-line")).toHaveText("Heidi liked your cast");
  // Never seen in this browser: new, and seen now, each account by itself.
  await expect(rows(page).first()).toHaveClass(/\bnew\b/);
  await expect(bell(page)).toHaveAccessibleName("Notifications, 0 unread");
  expect(await page.evaluate((key) => JSON.parse(localStorage.getItem(key)!), SEEN)).toEqual({
    [me.fid]: first.newest,
    [other.fid]: theirs.newest,
  });
  expect(asked).toBe(1);
});

test("in view, the bell asks every 3 minutes and when back in view; an open page adds what's new above its rows", async ({ page }) => {
  await page.addInitScript(() => {
    let hidden = false;
    Object.defineProperty(Document.prototype, "hidden", { configurable: true, get: () => hidden });
    Object.defineProperty(Document.prototype, "visibilityState", { configurable: true, get: () => (hidden ? "hidden" : "visible") });
    (window as any).__setHidden = (value: boolean) => {
      hidden = value;
      document.dispatchEvent(new Event("visibilitychange", { bubbles: true }));
    };
  });
  await page.clock.install();
  await ready(page);
  let answer = first;
  let asked = 0;
  await page.route("**/api/v1/notifications", (route) => {
    asked += 1;
    return route.fulfill({ json: answer, headers: { "cache-control": "private, no-store" } });
  });
  await page.goto("/notifications");
  await expect(rows(page)).toHaveCount(7);
  expect(asked).toBe(1);
  // A row already there, marked to tell whether it is drawn anew.
  await rows(page).first().evaluate((el) => el.setAttribute("data-was-here", ""));

  // Three minutes on, the bell asks again, and someone has replied since.
  const reply = { kind: "reply", actors: [person(9, "heidi", "Heidi")], count: 1, ts: at, cast: { hash: hex(10), excerpt: "Just now" } };
  answer = { ...first, groups: [reply, ...first.groups], newest: at };
  await page.clock.fastForward("03:01");
  await expect(rows(page)).toHaveCount(8);
  await expect(rows(page).first().locator(".notif-line")).toHaveText("Heidi replied to your cast");
  await expect(rows(page).first()).toHaveClass(/\bnew\b/);
  await expect(rows(page).nth(1)).toHaveAttribute("data-was-here", "");
  // The page is open: seen at once.
  await expect(bell(page)).toHaveAccessibleName("Notifications, 0 unread");
  expect(asked).toBe(2);

  // Hidden, it asks nothing; back in view, it asks at once.
  await page.evaluate(() => (window as any).__setHidden(true));
  await page.clock.fastForward("06:01");
  await page.waitForTimeout(300);
  expect(asked).toBe(2);
  await page.evaluate(() => (window as any).__setHidden(false));
  await expect.poll(() => asked).toBe(3);
});

test("the tabs show all, mentions (and quotes that mention you), and replies, from the URL", async ({ page }, testInfo) => {
  await ready(page);
  await page.goto("/notifications");
  const tabs = page.getByRole("tablist", { name: "Notifications" });
  const tab = (name: string) => tabs.getByRole("tab", { name });
  await expect(tab("All")).toHaveAttribute("aria-selected", "true");
  await expect(tabs.getByRole("tab")).toHaveText(["All", "Mentions", "Replies"]);
  const panel = page.getByRole("tabpanel", { name: "All" });
  await expect(panel).toBeVisible();
  await expect(rows(page)).toHaveCount(7);

  await tab("Mentions").click();
  await expect(page).toHaveURL(/\/notifications\?tab=mentions$/);
  await expect(tab("Mentions")).toHaveAttribute("aria-selected", "true");
  await expect(tab("All")).toHaveAttribute("aria-selected", "false");
  await expect(page.getByRole("tabpanel", { name: "Mentions" })).toBeVisible();
  await expect(rows(page).locator(".notif-line")).toHaveText(["Bob mentioned you", "Carol quoted your cast"]);
  await page.mouse.move(5, 5);
  await shot(page, "notifications-mentions-1280", testInfo);

  await tab("Replies").click();
  await expect(page).toHaveURL(/\?tab=replies$/);
  await expect(rows(page).locator(".notif-line")).toHaveText(["Frank replied to your cast"]);

  // The arrow keys move between them, and the tab follows, focused.
  await tab("Replies").focus();
  await page.keyboard.press("ArrowLeft");
  await expect(page).toHaveURL(/\?tab=mentions$/);
  await expect(tab("Mentions")).toBeFocused();
  await page.keyboard.press("Home");
  await expect(page).toHaveURL(/\/notifications$/);
  await expect(tab("All")).toBeFocused();
  await expect(tab("All")).toHaveAttribute("tabindex", "0");
  await expect(tab("Mentions")).toHaveAttribute("tabindex", "-1");

  // A tab visited first comes from the URL.
  await page.goto("/notifications?tab=replies");
  await expect(tab("Replies")).toHaveAttribute("aria-selected", "true");
  await expect(rows(page)).toHaveCount(1);
});

test("a row opens its cast, a follow its follower, and a name or face its profile card", async ({ page }) => {
  await ready(page);
  await page.goto("/notifications");
  await expect(rows(page)).toHaveCount(7);
  // Anywhere on a row that isn't a link: its cast.
  await rows(page).first().locator(".notif-excerpt").click();
  await expect(page).toHaveURL(new RegExp(`/p/${hex(1)}$`));
  await page.goBack();
  await expect(rows(page)).toHaveCount(7);
  // The sentence is a link there too.
  await expect(rows(page).nth(1).locator(".notif-open")).toHaveAttribute("href", `/p/${hex(2)}`);
  // Follows: the newest follower's profile.
  await expect(rows(page).nth(2).locator(".notif-open")).toHaveAttribute("href", "/u/grace");
  await rows(page).nth(2).locator(".notif-icon").click();
  await expect(page).toHaveURL(/\/u\/grace$/);
  await page.goBack();
  // A face, pointed at: that person's card, with what the payload says.
  await page.mouse.move(5, 400);
  await rows(page).first().locator(".notif-face").first().hover();
  const card = page.getByRole("dialog", { name: "Profile: Alice (@alice)", exact: true });
  await expect(card).toBeVisible();
  await expect(card).toContainText("Reads conversations.");
  await page.mouse.move(5, 400);
  await expect(card).toBeHidden();
  await rows(page).nth(3).locator(".notif-name").hover();
  await expect(page.getByRole("dialog", { name: "Profile: Bob (@bob)", exact: true })).toBeVisible();
});

test("signed out, there's no bell, and the page asks to sign in", async ({ page }) => {
  const calls = await serve(page);
  await page.goto("/notifications");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Notifications");
  await expect(page.getByText("Sign in to see your notifications.")).toBeVisible();
  await expect(page.getByRole("tablist")).toHaveCount(0);
  await expect(page.locator(".bell")).toHaveCount(0);
  await page.locator(".notif-empty").getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByRole("dialog", { name: "Sign in with Farcaster" })).toBeVisible();
  expect(calls.first).toBe(0);
});

test("a read that fails says why, and Try again reads again", async ({ page }) => {
  const calls = await ready(page, SEEN_BEFORE, { failFirst: 2 });
  await page.goto("/notifications");
  const failed = page.locator(".notif-error[role=alert]");
  await expect(failed).toContainText("Farcaster data source is busy");
  await expect(page.locator(".bell-badge")).toHaveCount(0);
  await failed.getByRole("button", { name: "Try again" }).click();
  await expect(rows(page)).toHaveCount(7);
  expect(calls.first).toBeGreaterThanOrEqual(3);
});

test("nothing yet, per tab", async ({ page }) => {
  await signIn(page, me);
  await serve(page);
  await page.route("**/api/v1/notifications**", (route) => route.fulfill({ json: { groups: [], newest: 0, meta } }));
  await page.goto("/notifications?tab=mentions");
  await expect(page.locator(".notif-empty")).toHaveText(/^No mentions yet\./);
  await expect(page.getByRole("button", { name: "Load more" })).toHaveCount(0);
  await expect(bell(page)).toHaveAccessibleName("Notifications, 0 unread");
});

test("the page is accessible: one heading, a tab list, named links, readable text", async ({ page }) => {
  await ready(page);
  await page.goto("/notifications");
  await expect(rows(page)).toHaveCount(7);
  await expect(page.locator("h1")).toHaveCount(1);
  // Tabs control the panel, which is named by the tab chosen.
  const tabs = await page.getByRole("tab").evaluateAll((els) => els.map((el) => [el.getAttribute("aria-controls"), el.id]));
  expect(tabs).toEqual([
    ["notifications-panel", "notifications-tab-all"],
    ["notifications-panel", "notifications-tab-mentions"],
    ["notifications-panel", "notifications-tab-replies"],
  ]);
  await expect(page.locator("#notifications-panel")).toHaveAttribute("aria-labelledby", "notifications-tab-all");
  // Every link and button in the list has a name.
  const unnamed = await page.locator(".notif-list a, .notif-list button, .notif-more button").evaluateAll((els) =>
    els.filter((el) => !(el.getAttribute("aria-label") || el.textContent || "").trim()).length,
  );
  expect(unnamed).toBe(0);
  // The icons are decoration; the sentence says what happened.
  await expect(page.locator(".notif-icon[aria-hidden=true]")).toHaveCount(7);
  // Muted and plain text stay readable on a new row's tint.
  const ratios = await page.locator(".notif.new").first().evaluate((row) => {
    const rgb = (css: string) => css.match(/\d+(\.\d+)?/g)!.slice(0, 3).map(Number);
    const lum = (css: string) => {
      const [r, g, b] = rgb(css).map((v) => v / 255).map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
      return 0.2126 * r + 0.7152 * g + 0.0722 * b;
    };
    const ratio = (a: string, b: string) => {
      const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x);
      return (hi + 0.05) / (lo + 0.05);
    };
    const bg = getComputedStyle(row).backgroundColor;
    const color = (selector: string) => getComputedStyle(row.querySelector(selector)!).color;
    return [ratio(color(".notif-excerpt"), bg), ratio(color(".notif-open"), bg), ratio(color(".notif-when"), bg)];
  });
  for (const ratio of ratios) expect(ratio).toBeGreaterThanOrEqual(4.5);
  // The bell's badge is white on the accent.
  await page.goto("/");
  await page.evaluate((key) => localStorage.setItem(key, "{}"), SEEN);
  await page.reload();
  await expect(page.locator(".bell-badge")).toHaveText("7");
  const badge = await page.locator(".bell-badge").evaluate((el) => [getComputedStyle(el).color, getComputedStyle(el).backgroundColor]);
  expect(badge).toEqual(["rgb(255, 255, 255)", "rgb(79, 67, 212)"]);
});

test("at 390 on a touch screen it fits, and its targets take a fingertip", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await ready(page, SEEN_BEFORE - 600_000);
  await page.goto("/");
  await appCame(page);
  // Past nine, the badge says 9+; screen readers hear the count.
  await page.route("**/api/v1/notifications**", (route) => route.fulfill({ json: {
    ...first, groups: Array.from({ length: 12 }, (_, i) => ({ ...first.groups[i % 7], ts: at - 60 * (i + 1), cast: { hash: hex(100 + i), excerpt: "x" } })),
  } }));
  await page.evaluate((key) => localStorage.setItem(key, "{}"), SEEN);
  await page.reload();
  await appCame(page);
  await expect(bell(page)).toHaveAccessibleName("Notifications, 12 unread");
  await expect(page.locator(".bell-badge")).toHaveText("9+");
  // The header holds the wordmark, Home, search, the bell and the picture.
  for (const el of [".wordmark", ".main-nav a", ".search-box", ".bell", ".account-button"]) {
    await expect(page.locator(el).first(), el).toBeInViewport({ ratio: 1 });
  }
  // Search keeps room for its whole placeholder, with 8 px to spare (CI's
  // text renders a pixel or two wider than a laptop's).
  const room = await page.locator(".search-box input").evaluate((input: HTMLInputElement) => {
    const css = getComputedStyle(input);
    const pen = document.createElement("canvas").getContext("2d")!;
    pen.font = `${css.fontWeight} ${css.fontSize} ${css.fontFamily}`;
    const inside = input.clientWidth - parseFloat(css.paddingLeft) - parseFloat(css.paddingRight);
    return inside - pen.measureText(input.placeholder).width;
  });
  expect(room).toBeGreaterThanOrEqual(8);
  await shot(page.locator(".site-header"), "bell-390", testInfo);
  // The bell and the picture each take a 44 px tap, neither the other's.
  const box = (await bell(page).boundingBox())!;
  const band = await bell(page).evaluate((el) => {
    const after = getComputedStyle(el, "::after");
    return [parseFloat(after.width), parseFloat(after.height)];
  });
  expect(Math.min(...band)).toBeGreaterThanOrEqual(44);
  // Fixed sizes (36 px, a 12 px gap, 32 px), so no font moves them: the
  // bell's 44 px band ends before the picture's begins.
  const picture = (await page.locator(".account-button").boundingBox())!;
  const bellRight = box.x + box.width / 2 + band[0] / 2;
  const pictureLeft = picture.x + picture.width / 2 - 22;
  expect(bellRight).toBeLessThanOrEqual(pictureLeft);

  // Seen up to before the like, the reply and the follows, as elsewhere.
  await page.unroute("**/api/v1/notifications**");
  await serve(page);
  await page.evaluate(([key, fid, seen]) => localStorage.setItem(key as string, JSON.stringify({ [fid as number]: seen })), [SEEN, me.fid, SEEN_BEFORE]);
  await page.reload();
  await appCame(page);
  await expect(bell(page)).toHaveAccessibleName("Notifications, 3 unread");
  await bell(page).tap();
  await expect(page).toHaveURL(/\/notifications$/);
  await expect(rows(page)).toHaveCount(7);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await expect(page.getByRole("tab", { name: "Mentions" })).toBeInViewport({ ratio: 1 });
  await shot(page, "notifications-all-390", testInfo);

  // Faces and names: each a 44 px tap of its own; a tap there opens the
  // person's card, not the row's cast.
  const like = rows(page).first();
  const faces = await like.locator(".notif-face").all();
  const targets: [Locator, boolean][] = [...faces.map((face): [Locator, boolean] => [face, true]), [like.locator(".notif-name"), false]];
  for (const [target, wide] of targets) {
    const hit = await target.evaluate((el, wide) => {
      const after = getComputedStyle(el, "::after");
      const b = el.getBoundingClientRect();
      const lands = (dx: number, dy: number) => {
        const at = document.elementFromPoint(b.x + b.width / 2 + dx, b.y + b.height / 2 + dy);
        return at === el || el.contains(at);
      };
      const around = [lands(0, 0), lands(0, -20), lands(0, 20)];
      return { w: parseFloat(after.width), h: parseFloat(after.height), lands: wide ? [...around, lands(-20, 0), lands(20, 0)] : around };
    }, wide);
    expect(hit.h, "band height").toBeGreaterThanOrEqual(44);
    if (wide) expect(hit.w, "band width").toBeGreaterThanOrEqual(44);
    expect(hit.lands.every(Boolean), `a tap 20 px around its middle: ${hit.lands}`).toBe(true);
  }
  await like.locator(".notif-face").nth(1).tap();
  await expect(page.getByRole("dialog", { name: "Profile: Bob (@bob)", exact: true })).toBeInViewport();
  await expect(page).toHaveURL(/\/notifications$/);
  await page.keyboard.press("Escape");
  // The tabs and Load more take a fingertip.
  for (const tab of await page.getByRole("tab").all()) {
    const after = await tab.evaluate((el) => parseFloat(getComputedStyle(el, "::after").height));
    expect(after).toBeGreaterThanOrEqual(44);
  }
  const more = page.getByRole("button", { name: "Load more" });
  await more.scrollIntoViewIfNeeded();
  expect((await more.boundingBox())!.height).toBeGreaterThanOrEqual(44);
  // A tap on a row opens its cast.
  await rows(page).nth(1).locator(".notif-excerpt").tap();
  await expect(page).toHaveURL(new RegExp(`/p/${hex(2)}$`));
  await page.goBack();

  await page.goto("/notifications?tab=mentions");
  await expect(rows(page)).toHaveCount(2);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await shot(page, "notifications-mentions-390", testInfo);
  await context.close();
});
