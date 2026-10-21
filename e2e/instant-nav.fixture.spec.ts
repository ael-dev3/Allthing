// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { appCame } from "./helpers/app";

/*
 * Opening a page from a link or a card (nav.rs `go`,
 * docs/PERFORMANCE.md#3-client-side-navigation):
 *   - a page the app already has something of (a post or a profile a feed
 *     showed, or data it keeps) opens in the frames after the press, and the
 *     placeholders of its part still to come show only if that part is slow
 *     (250 ms);
 *   - a page it has nothing of keeps the page left on screen, the link held
 *     pressed, until its data comes, or 250 ms at most, then opens complete,
 *     or with its placeholders; and a page whose code is its own (split.rs),
 *     until that has come, the first time;
 *   - a post's or a channel's data is fetched as its link is pressed or
 *     touched, or once a mouse has rested on it;
 *   - another press takes over from a page still waiting;
 *   - the replies' sort tabs take each other's place in the history;
 *   - the page opened is readable in its first frame: nothing on it comes in
 *     from transparent, and there's no view transition.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = Math.floor(Date.now() / 1000) - 600;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const bob = { fid: 43, username: "bob", display_name: "Bob" };
const feed = [
  { hash: hex(1), author: alice, text: "A cast in a channel", ts, channel: { id: "fixture", label: "fixture" }, replies: 2 },
  { hash: hex(2), author: bob, text: "A cast by Bob", ts: ts - 60, replies: 2 },
  { hash: hex(3), author: alice, text: "Another cast by Alice", ts: ts - 120 },
];
const reply = (parent: (typeof feed)[number], n: number) =>
  ({ hash: hex(100 + n), author: bob, text: `Reply ${n}`, ts: parent.ts + 60 * n, parent_hash: parent.hash, likes: n });
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** Serves the feed at once, and a post's thread and the channel after `delay` ms; says what was asked for. */
async function serve(page: Page, delay: { cast?: number; channel?: number } = {}) {
  const asked: string[] = [];
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : feed, notes: {}, meta } });
  });
  await page.route("**/api/v1/cast?*", async (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash");
    asked.push(`cast ${hash}`);
    await wait(delay.cast ?? 0);
    const cast = feed.find((c) => c.hash === hash) ?? feed[0];
    const thread = { cast, children: [1, 2].map((n) => ({ cast: reply(cast, n) })) };
    await route.fulfill({ json: { thread, notes: {}, meta } }).catch(() => {});
  });
  await page.route("**/api/v1/channel?*", async (route) => {
    const id = new URL(route.request().url()).searchParams.get("id");
    asked.push(id === "fixture" ? "channel" : `channel ${id}`);
    await wait(delay.channel ?? 0);
    const channel = { id: "fixture", name: "Fixture Channel", parent_url: "https://warpcast.com/~/channel/fixture", casters: 12 };
    await route.fulfill({ json: { channel, pool: feed.slice(0, 1), meta } }).catch(() => {});
  });
  return asked;
}

type Frame = { t: number; path: string; skeleton: boolean; held: boolean };

/**
 * The code of a page of its own (split.rs, `chunk`) in before the press, as
 * a few seconds on the feed bring it: focus on a link to another such page
 * (`link`) asks for it, and for that page's data, which nothing here waits
 * for.
 */
async function codeIn(page: Page, link: string, chunk: string) {
  await page.evaluate((href) => {
    const a = Object.assign(document.createElement("a"), { href });
    document.body.append(a);
    a.dispatchEvent(new FocusEvent("focusin", { bubbles: true }));
    a.remove();
  }, link);
  await expect.poll(() => page.evaluate((name) => performance.getEntriesByType("resource").some((e) => e.name.includes(name)), chunk)).toBe(true);
  // Compiled and in, a moment after it's fetched.
  await page.waitForTimeout(200);
}

/**
 * Records, in each frame from `__watch(placeholders)` on, the page's path,
 * whether one of `placeholders` shows and whether a press is held, and when
 * the next click came.
 */
function frames() {
  const w = window as any;
  w.__frames = [] as Frame[];
  w.__clickAt = -1;
  document.addEventListener("click", () => (w.__clickAt = w.__clickAt < 0 ? performance.now() : w.__clickAt), true);
  w.__watch = (placeholders: string) => {
    w.__frames = [];
    w.__clickAt = -1;
    const since = performance.now();
    const shows = (el: Element) => {
      const r = el.getBoundingClientRect();
      return r.width > 0 && r.bottom > 0 && r.top < innerHeight && getComputedStyle(el).visibility === "visible";
    };
    const tick = () => {
      w.__frames.push({
        t: performance.now(),
        path: location.pathname,
        skeleton: [...document.querySelectorAll(placeholders)].some(shows),
        held: document.querySelector("[data-press=held]") !== null,
      });
      if (performance.now() - since < 4000) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  };
}

/** The front page, its frames watched for `placeholders`. */
async function watched(page: Page, placeholders: string) {
  await page.addInitScript(frames);
  await page.goto("/");
  await appCame(page);
  await expect(page.locator("#app .cast-card")).toHaveCount(feed.length);
  await page.evaluate((selector) => (window as any).__watch(selector), placeholders);
}

/** A post's replies' placeholders, and a channel's. */
const REPLIES = "#app .replies .cast-card.skeleton";
const CHANNEL = "#app .channel-hero[aria-busy=true], #app .cast-list[aria-busy=true] .cast-card.skeleton";

const recorded = (page: Page) => page.evaluate(() => ({ frames: (window as any).__frames as Frame[], clickAt: (window as any).__clickAt as number }));

const sizes = [
  { name: "at 1280", use: { viewport: { width: 1280, height: 900 } } },
  { name: "at 390", use: { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true } },
];

for (const size of sizes) {
  test.describe(size.name, () => {
    test.use(size.use);

    test("a post opened from its card shows in the frames after the press, its replies with it when they come in time", async ({ page }) => {
      await serve(page, { cast: 80 });
      await watched(page, REPLIES);
      await codeIn(page, `/p/${hex(99)}`, "split_post.wasm");
      await page.evaluate((selector) => (window as any).__watch(selector), REPLIES);
      const card = page.locator(`#app .cast-card[data-hash="${feed[1].hash}"]`);
      await card.locator(".cast-text").click();
      await expect(page).toHaveURL(new RegExp(`/p/${feed[1].hash}$`));
      await expect(page.locator(".replies .comment")).toHaveCount(2);
      await page.waitForTimeout(400);
      const { frames, clickAt } = await recorded(page);
      const opened = frames.findIndex((f) => f.path.startsWith("/p/"));
      // The card shows pressed until the post shows, a frame or two later.
      expect(frames.slice(0, opened).some((f) => f.held && f.t > clickAt)).toBe(true);
      expect(frames[opened].t - clickAt).toBeLessThan(200);
      // Its replies came within 250 ms: no placeholder showed for them.
      expect(frames.slice(opened).filter((f) => f.skeleton)).toEqual([]);
      expect(frames.slice(opened).some((f) => f.held)).toBe(false);
    });

    test("a post whose replies are slow shows them as placeholders only after 250 ms", async ({ page }) => {
      await serve(page, { cast: 1500 });
      await watched(page, REPLIES);
      await page.locator(`#app .cast-card[data-hash="${feed[1].hash}"] .cast-text`).click();
      await expect(page.locator(".post")).toBeVisible();
      await expect(page.locator(".replies .cast-card.skeleton").first()).toBeVisible();
      const { frames } = await recorded(page);
      const opened = frames.find((f) => f.path.startsWith("/p/"))!;
      const placeholders = frames.find((f) => f.skeleton)!;
      expect(placeholders.t - opened.t).toBeGreaterThan(200);
      await expect(page.locator(".replies .comment")).toHaveCount(2);
    });

    test("a channel keeps the feed on screen, its link held pressed, until its data comes, then shows complete", async ({ page }) => {
      const asked = await serve(page, { channel: 60 });
      await watched(page, CHANNEL);
      await codeIn(page, "/c/elsewhere", "split_channel.wasm");
      await page.evaluate((selector) => (window as any).__watch(selector), CHANNEL);
      await page.locator(`#app .cast-card[data-hash="${feed[0].hash}"] a[href="/c/fixture"]`).first().click();
      await expect(page).toHaveURL(/\/c\/fixture$/);
      await expect(page.getByRole("heading", { level: 1 })).toHaveText("Fixture Channel");
      await page.waitForTimeout(300);
      const { frames, clickAt } = await recorded(page);
      const opened = frames.findIndex((f) => f.path === "/c/fixture");
      const before = frames.slice(0, opened).filter((f) => f.t > clickAt);
      expect(before.length).toBeGreaterThan(0);
      expect(before.every((f) => f.path === "/")).toBe(true);
      expect(before.some((f) => f.held)).toBe(true);
      expect(frames.slice(opened).filter((f) => f.skeleton)).toEqual([]);
      // Asked for once: the page took the answer the press asked for.
      expect(asked.filter((a) => a === "channel")).toHaveLength(1);
    });

    test("a channel whose code hasn't come keeps the feed on screen, its link held pressed, until it comes, then shows complete", async ({ page }) => {
      // Its data at once, its code 150 ms after it's asked for, which the
      // mouse coming to rest on the link does, 50 ms before the click.
      await serve(page);
      await page.route("**/split_channel.wasm", async (route) => {
        await wait(150);
        await route.continue().catch(() => {});
      });
      await watched(page, CHANNEL);
      const link = page.locator(`#app .cast-card[data-hash="${feed[0].hash}"] a[href="/c/fixture"]`).first();
      const asked = page.waitForRequest("**/split_channel.wasm");
      await link.hover();
      await asked;
      await page.waitForTimeout(50);
      await link.click();
      await expect(page).toHaveURL(/\/c\/fixture$/);
      await expect(page.getByRole("heading", { level: 1 })).toHaveText("Fixture Channel");
      await page.waitForTimeout(300);
      const { frames, clickAt } = await recorded(page);
      const opened = frames.findIndex((f) => f.path === "/c/fixture");
      expect(frames.slice(0, opened).some((f) => f.held && f.t > clickAt)).toBe(true);
      // Not on the placeholder of a page whose code hasn't come, nor on its
      // own: opened once a frame had painted, it showed that until its code came.
      expect(frames.slice(opened).filter((f) => f.skeleton)).toEqual([]);
    });

    test("a page slow to come opens 250 ms after the press, with its placeholders", async ({ page }) => {
      const asked = await serve(page, { channel: 2500 });
      await watched(page, CHANNEL);
      await page.locator(`#app .cast-card[data-hash="${feed[0].hash}"] a[href="/c/fixture"]`).first().click();
      await expect(page).toHaveURL(/\/c\/fixture$/);
      await expect(page.locator(".channel-hero[aria-busy=true]")).toBeVisible();
      const { frames, clickAt } = await recorded(page);
      const opened = frames.find((f) => f.path === "/c/fixture")!;
      expect(opened.t - clickAt).toBeGreaterThan(200);
      expect(opened.t - clickAt).toBeLessThan(1000);
      await expect(page.getByRole("heading", { level: 1 })).toHaveText("Fixture Channel", { timeout: 6000 });
      expect(asked.filter((a) => a === "channel")).toHaveLength(1);
    });

    test("another press takes over from a page still waiting", async ({ page }) => {
      await serve(page, { channel: 2500 });
      await watched(page, CHANNEL);
      // The channel's link, then at once a card: the card's post opens, and the channel never does.
      await page.evaluate(([channel, post]) => {
        (document.querySelector(channel) as HTMLElement).click();
        (document.querySelector(post) as HTMLElement).click();
      }, [`#app .cast-card[data-hash="${feed[0].hash}"] a[href="/c/fixture"]`, `#app .cast-card[data-hash="${feed[2].hash}"] .cast-text`]);
      await expect(page).toHaveURL(new RegExp(`/p/${feed[2].hash}$`));
      await page.waitForTimeout(600);
      await expect(page).toHaveURL(new RegExp(`/p/${feed[2].hash}$`));
      const { frames } = await recorded(page);
      expect(frames.filter((f) => f.path === "/c/fixture")).toEqual([]);
      expect(await page.locator("[data-press=held]").count()).toBe(0);
    });
  });
}

test("a page still waiting gives up when the address changes another way, as a search does", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await serve(page, { channel: 2500 });
  await page.route("**/api/v1/search?*", (route) =>
    route.fulfill({ json: { query: "fixture", users: [], channels: [], casts: [], meta } }));
  await watched(page, CHANNEL);
  // The channel's link, then at once a search from the header's box.
  await page.evaluate((channel) => {
    (document.querySelector(channel) as HTMLElement).click();
    const input = document.querySelector("#app .search-box input") as HTMLInputElement;
    input.value = "fixture";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    (input.closest("form") as HTMLFormElement).requestSubmit();
  }, `#app .cast-card[data-hash="${feed[0].hash}"] a[href="/c/fixture"]`);
  await expect(page).toHaveURL(/\/search\?q=fixture$/);
  await page.waitForTimeout(600);
  await expect(page).toHaveURL(/\/search\?q=fixture$/);
  const { frames } = await recorded(page);
  expect(frames.filter((f) => f.path === "/c/fixture")).toEqual([]);
  expect(await page.locator("[data-press=held]").count()).toBe(0);
});

test("a channel's data is fetched as its link is touched, before the tap ends", async ({ browser }, info) => {
  const context = await browser.newContext({ baseURL: info.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  const page = await context.newPage();
  const asked = await serve(page, { channel: 2500 });
  await page.goto("/");
  await appCame(page);
  const link = page.locator(`#app .cast-card[data-hash="${feed[0].hash}"] a[href="/c/fixture"]`).first();
  await link.dispatchEvent("pointerdown", { pointerType: "touch", isPrimary: true, button: 0, bubbles: true });
  await expect.poll(() => asked.filter((a) => a === "channel").length).toBe(1);
  await expect(page).toHaveURL(/\/$/);
  await context.close();
});

test("with a mouse, a channel's data is fetched once the pointer rests on its link, not as it passes", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  const asked = await serve(page, { channel: 2500 });
  await page.goto("/");
  await appCame(page);
  const link = page.locator(`#app .cast-card[data-hash="${feed[0].hash}"] a[href="/c/fixture"]`).first();
  const box = (await link.boundingBox())!;
  // Across it, without stopping.
  await page.mouse.move(box.x - 40, box.y + box.height / 2);
  await page.mouse.move(box.x + box.width + 40, box.y + box.height / 2, { steps: 4 });
  await page.waitForTimeout(300);
  expect(asked.filter((a) => a === "channel")).toEqual([]);
  // Resting on it.
  await link.hover();
  await expect.poll(() => asked.filter((a) => a === "channel").length).toBe(1);
});

test("the replies' sort tabs take each other's place in the history, so Back leaves the post", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  await appCame(page);
  await page.locator(`#app .cast-card[data-hash="${feed[1].hash}"] .cast-text`).click();
  await expect(page.locator(".replies .comment")).toHaveCount(2);
  const tabs = page.getByRole("navigation", { name: "Sort replies" });
  await tabs.getByRole("link", { name: "new" }).click();
  await expect(page).toHaveURL(/sort=new/);
  await expect(page.locator(".replies .comment").first()).toContainText("Reply 2");
  await tabs.getByRole("link", { name: "old" }).click();
  await expect(page).toHaveURL(/sort=old/);
  await expect(page.locator(".replies .comment").first()).toContainText("Reply 1");
  await page.goBack();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.locator(`#app .cast-card[data-hash="${feed[1].hash}"]`)).toBeVisible();
});

test("a page opened is readable in its first frame: nothing on it fades in, and there's no view transition", async ({ page }) => {
  await page.addInitScript(() => {
    const w = window as any;
    w.__transitions = 0;
    const start = document.startViewTransition?.bind(document);
    if (start) {
      (document as any).startViewTransition = (update: any) => {
        w.__transitions++;
        return start(update);
      };
    }
  });
  await serve(page, { cast: 80 });
  await page.goto("/");
  await appCame(page);
  await page.locator(`#app .cast-card[data-hash="${feed[1].hash}"] .cast-text`).click();
  await expect(page.locator(".post")).toBeVisible();
  // While the post and its replies rise into place, nothing on the page
  // comes in from transparent.
  const fading = () => page.evaluate(() => document.getAnimations()
    .filter((a) => {
      const effect = a.effect as KeyframeEffect | null;
      const target = effect?.target as Element | null | undefined;
      return target?.closest("#main") && effect!.getComputedTiming().iterations !== Infinity
        && effect!.getKeyframes().some((k) => k.opacity !== undefined && Number(k.opacity) < 1);
    })
    .map((a) => (a as CSSAnimation).animationName ?? "?"));
  expect(await fading()).toEqual([]);
  await expect(page.locator(".replies .comment")).toHaveCount(2);
  expect(await fading()).toEqual([]);
  expect(await page.evaluate(() => (window as any).__transitions)).toBe(0);
});
