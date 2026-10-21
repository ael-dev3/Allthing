// SPDX-License-Identifier: AGPL-3.0-or-later
import axe from "axe-core";
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";
import { signIn } from "./helpers/farcaster";

/*
 * A reply's composer shows what the reply answers, as the Farcaster app's
 * does: the author's picture, name, @username and how long ago; the start
 * of the cast, a few lines of it; its first images and videos, as squares
 * of a fixed size; "Replying to @username"; and a line from the author's
 * picture down to the reader's, beside the text box, which has focus by the
 * frame after the tap. All of it comes from the cast on the page: opening
 * it asks for nothing. Opened from a post's Reply and from a reply's, at
 * 1280 and at 390.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no composer");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const now = Math.floor(Date.now() / 1000);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const pfp = (name: string) => `https://imagedelivery.net/fixture/${name}/original`;
const alice = { fid: 42, username: "alice", display_name: "Alice of the Very Long Reading Club Name", pfp_url: pfp("alice") };
const bob = { fid: 7, username: "bob", display_name: "Bob", pfp_url: pfp("bob") };
const me = { fid: 1234, username: "reader", display_name: "Reader", pfp_url: pfp("reader") };
const STREAM = "https://customer-fixture.cloudflarestream.com/token/manifest/video.m3u8";
const image = (n: number) => ({ kind: "image", url: `https://imagedelivery.net/fixture/photo-${n}/original` });
// Eight lines, more than the composer shows of it.
const LINES = Array.from({ length: 8 }, (_, i) => `Line ${i + 1}: what I'm reading this week :pepe_gm:`).join("\n");
const post = {
  hash: hex(1),
  author: alice,
  text: LINES,
  ts: now - 3 * 3600,
  likes: 3,
  replies: 1,
  // Five pictures, one of them a video: the composer shows the first four.
  embeds: [image(1), { kind: "video", url: STREAM }, image(2), image(3), image(4)],
};
const reply = { hash: hex(2), author: bob, text: "Hyperion, then the Fall", ts: now - 600, likes: 0, replies: 0, parent_hash: post.hash };
const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64");

/** The post page's payload and its pictures; every API call made is noted. */
async function serve(page: Page) {
  const asked: string[] = [];
  await page.route("**/api/v1/**", (route) => {
    asked.push(new URL(route.request().url()).pathname);
    return route.fulfill({ status: 404, json: { error: "Fixture endpoint" } });
  });
  await page.route("**/api/v1/cast?*", (route) => {
    asked.push("/api/v1/cast");
    return route.fulfill({ json: { thread: { cast: post, children: [{ cast: reply, children: [] }] }, notes: {}, meta } });
  });
  await page.route("https://imagedelivery.net/**", (route) => route.fulfill({ contentType: "image/png", body: png }));
  await page.route("https://customer-fixture.cloudflarestream.com/**", (route) =>
    route.request().url().includes("/thumbnails/") ? route.fulfill({ contentType: "image/png", body: png }) : route.fulfill({ status: 404, body: "" }),
  );
  await page.route("https://haatz.quilibrium.com/**", (route) =>
    route.fulfill({ headers: { "access-control-allow-origin": "*" }, json: { messages: [] } }),
  );
  return asked;
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  return { context, page: await context.newPage() };
}

/** Open the post page, signed in (or not), with layout shifts recorded from here on. */
async function openPost(page: Page, signedIn = true) {
  if (signedIn) await signIn(page, me);
  const asked = await serve(page);
  await page.goto(`/p/${post.hash}`);
  await expect(page.locator(".post")).toBeVisible();
  await expect(page.locator(".comment")).toHaveCount(1);
  await page.evaluate(() => {
    const w = window as unknown as { __shifts: { value: number; at: number; sources: string[] }[] };
    w.__shifts = [];
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries() as unknown as { value: number; startTime: number; sources?: { node?: Element }[] }[]) {
        w.__shifts.push({ value: entry.value, at: entry.startTime, sources: (entry.sources ?? []).map((s) => s.node?.className ?? "?") });
      }
    }).observe({ type: "layout-shift" });
  });
  return asked;
}

type Frame = {
  /** What has focus once the first frame after the press is drawn. */
  focus: string | null;
  /** Whether the composer's bar is there in that frame (not "Loading…"). */
  bar: boolean;
  /** That frame's time. */
  at: number;
  /** From the press to that frame, to the text box's focus, and to the bar, in ms. */
  frameMs: number;
  focusMs: number | null;
  barMs: number | null;
};

/**
 * Press `button` as a reader does (a tap on a phone), and say what has focus
 * and whether the bar is there once the first frame after the press is
 * drawn, and how long the box's focus and the bar took.
 */
async function press(page: Page, button: Locator, touch: boolean): Promise<Frame> {
  await page.evaluate(() => {
    const w = window as unknown as { __frame?: Frame };
    delete w.__frame;
    const ready = () => Boolean(document.querySelector(".compose-modal .compose-post:not(.compose-loading)"));
    addEventListener(
      "click",
      (event) => {
        const t0 = event.timeStamp;
        const state: Frame = { focus: null, bar: false, at: 0, frameMs: 0, focusMs: null, barMs: ready() ? 0 : null };
        const focused = (e: FocusEvent) => {
          if ((e.target as Element).classList?.contains("compose-text") && state.focusMs === null) state.focusMs = performance.now() - t0;
        };
        addEventListener("focusin", focused, true);
        const watch = new MutationObserver(() => {
          if (state.barMs === null && ready()) state.barMs = performance.now() - t0;
        });
        watch.observe(document.body, { childList: true, subtree: true });
        requestAnimationFrame((at) =>
          setTimeout(() => {
            Object.assign(state, { focus: document.activeElement?.getAttribute("aria-label") ?? null, bar: ready(), at, frameMs: at - t0 });
            w.__frame = state;
          }, 0),
        );
        setTimeout(() => {
          removeEventListener("focusin", focused, true);
          watch.disconnect();
        }, 5000);
      },
      { capture: true, once: true },
    );
  });
  await (touch ? button.tap() : button.click());
  await expect.poll(() => page.evaluate(() => Boolean((window as unknown as { __frame?: Frame }).__frame))).toBe(true);
  return page.evaluate(() => (window as unknown as { __frame: Frame }).__frame);
}

/** How long the bar took to come, once it has (`press` watches for 5 s). */
async function barMs(page: Page) {
  await expect.poll(() => page.evaluate(() => (window as unknown as { __frame: Frame }).__frame.barMs)).not.toBeNull();
  return page.evaluate(() => (window as unknown as { __frame: Frame }).__frame.barMs!);
}

/** How much moved after `at` (a frame's time), in a second's watching: CLS's sum, input or not. */
async function shiftAfter(page: Page, at: number) {
  await page.waitForTimeout(1000);
  return page.evaluate(
    (at) =>
      (window as unknown as { __shifts: { value: number; at: number; sources: string[] }[] }).__shifts
        .filter((s) => s.at > at)
        .reduce((sum, s) => sum + s.value, 0),
    at,
  );
}

/** Accessibility problems axe finds in `scope`, once what animates in has. */
async function axeViolations(page: Page, scope: string) {
  await page.evaluate(() =>
    Promise.all(
      document
        .getAnimations()
        .filter((a) => a.effect?.getComputedTiming().endTime !== Infinity)
        .map((a) => a.finished.catch(() => undefined)),
    ),
  );
  await page.evaluate(axe.source);
  return page.evaluate(async (scope) => {
    // @ts-expect-error axe is the injected script's global
    const result = await window.axe.run(scope, { resultTypes: ["violations"] });
    return result.violations.map((v: { id: string; nodes: { target: string[] }[] }) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`);
  }, scope);
}

type Box = { x: number; y: number; width: number; height: number };

/** The block's parts, as laid out: the thread line (its `::before`) and the pictures around it. */
async function layout(dialog: Locator) {
  return dialog.evaluate((root) => {
    const rect = (el: Element | null) => {
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return { x: r.x, y: r.y, width: r.width, height: r.height };
    };
    const block = root.querySelector(".reply-parent")!;
    const style = getComputedStyle(block, "::before");
    const b = block.getBoundingClientRect();
    const top = b.top + parseFloat(style.top);
    const line = { x: b.left + parseFloat(style.left), y: top, width: parseFloat(style.width), height: b.bottom - parseFloat(style.bottom) - top, content: style.content };
    const text = root.querySelector(".reply-parent-text") as HTMLElement | null;
    return {
      line,
      author: rect(block.querySelector(".avatar")),
      reader: rect(root.querySelector(".reply-row .avatar")),
      box: rect(root.querySelector(".reply-row .compose-text")),
      thumbs: Array.from(root.querySelectorAll(".reply-thumb")).map((t) => rect(t) as Box),
      lines: text ? Math.round(text.clientHeight / parseFloat(getComputedStyle(text).lineHeight)) : 0,
      clamped: text ? text.scrollHeight > text.clientHeight + 4 : false,
      dialog: rect(root),
    };
  });
}

/** What the composer shows of the post, the same at both widths but for sizes. */
async function showsThePost(page: Page, size: { avatar: number; thumb: number; lines: number }) {
  const dialog = page.getByRole("dialog", { name: "Reply" });
  const block = dialog.locator(".reply-parent");
  await expect(block).toHaveAttribute("aria-hidden", "true");
  await expect(block.locator(".reply-parent-name")).toHaveText(alice.display_name);
  await expect(block.locator(".reply-parent-handle")).toHaveText("@alice");
  await expect(block.locator(".reply-parent-meta time")).toHaveText("3h ago");
  await expect(block.locator(".reply-parent-text")).toContainText("Line 1: what I'm reading this week");
  await expect(block.locator(".reply-parent-text img").first()).toHaveAttribute("alt", ":pepe_gm:");
  await expect(block.locator(".reply-parent-to")).toHaveText("Replying to @alice");
  // Four of its five pictures, the video's with its poster and a play sign.
  await expect(block.locator(".reply-thumb")).toHaveCount(4);
  await expect(block.locator(".reply-thumb").nth(0).locator("img")).toHaveAttribute("src", "https://imagedelivery.net/fixture/photo-1/fit=scale-down,f=auto,w=720");
  const video = block.locator(".reply-thumb").nth(1);
  await expect(video).toHaveClass(/reply-thumb-video/);
  await expect(video.locator("img")).toHaveAttribute("src", /\/thumbnails\/thumbnail\.jpg\?height=720$/);
  await expect(video.locator("svg")).toBeVisible();
  // The reader's picture beside the box, which says what it answers to a
  // screen reader.
  await expect(dialog.locator(".reply-row .avatar")).toHaveAttribute("src", /fixture\/reader\//);
  const box = dialog.getByRole("textbox", { name: "Cast your reply" });
  await expect(box).toHaveAttribute("aria-describedby", "compose-replying");
  await expect(dialog.locator("#compose-replying")).toHaveText(/^Replying to @alice: Line 1: what I'm reading this week :pepe_gm: Line 2: /);
  await expect(dialog.locator("#compose-replying")).toHaveText(/…$/);

  const laid = await layout(dialog);
  // Its first lines, then cut.
  expect(laid.lines).toBe(size.lines);
  expect(laid.clamped).toBe(true);
  // Pictures of a fixed size, which nothing moves as they load.
  for (const thumb of laid.thumbs) expect([Math.round(thumb.width), Math.round(thumb.height)]).toEqual([size.thumb, size.thumb]);
  expect(Math.round(laid.author!.width)).toBe(size.avatar);
  expect(Math.round(laid.reader!.width)).toBe(size.avatar);
  // A 2 px line from under the author's picture down to just above the
  // reader's, down both pictures' middles; the box beside the reader's.
  expect(laid.line.content).not.toBe("none");
  expect(laid.line.width).toBe(2);
  expect(laid.line.y).toBeGreaterThan(laid.author!.y + laid.author!.height);
  expect(laid.line.y + laid.line.height).toBeLessThanOrEqual(laid.reader!.y);
  expect(laid.line.y + laid.line.height).toBeGreaterThan(laid.reader!.y - 8);
  for (const avatar of [laid.author!, laid.reader!]) expect(Math.abs(laid.line.x + 1 - (avatar.x + avatar.width / 2))).toBeLessThanOrEqual(1);
  expect(laid.box!.x).toBeGreaterThan(laid.reader!.x + laid.reader!.width);
  return laid;
}

test("at 1280, Reply on a post shows what it answers, as Farcaster does, with the box focused at once and nothing moving", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  const asked = await openPost(page);
  const before = asked.length;
  const frame = await press(page, page.locator(".post").getByRole("button", { name: "Reply to @alice" }), false);
  // The box has focus in the first frame after the click.
  expect(frame.focus).toBe("Cast your reply");
  const dialog = page.getByRole("dialog", { name: "Reply" });
  await showsThePost(page, { avatar: 36, thumb: 96, lines: 5 });
  expect(await shiftAfter(page, frame.at)).toBeLessThan(0.01);
  // Nothing about the post was asked for.
  expect(asked.slice(before).filter((path) => path.startsWith("/api/v1/cast"))).toEqual([]);
  await expect(dialog.locator(".compose-loading")).toHaveCount(0);
  expect(await axeViolations(page, ".compose-modal")).toEqual([]);
  await page.screenshot({ path: testInfo.outputPath("reply-1280.png") });
});

test("at 1280, a reply's Reply shows that reply, and Escape takes focus back to it", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await openPost(page);
  const button = page.locator(".comment").getByRole("button", { name: "Reply to @bob" });
  const frame = await press(page, button, false);
  expect(frame.focus).toBe("Cast your reply");
  const dialog = page.getByRole("dialog", { name: "Reply" });
  const block = dialog.locator(".reply-parent");
  await expect(block.locator(".reply-parent-name")).toHaveText("Bob");
  await expect(block.locator(".reply-parent-handle")).toHaveText("@bob");
  await expect(block.locator(".reply-parent-meta time")).toHaveText("10m ago");
  await expect(block.locator(".reply-parent-text")).toHaveText("Hyperion, then the Fall");
  await expect(block.locator(".reply-parent-media")).toHaveCount(0);
  await expect(block.locator(".reply-parent-to")).toHaveText("Replying to @bob");
  await expect(dialog.locator("#compose-replying")).toHaveText("Replying to @bob: Hyperion, then the Fall");
  const laid = await layout(dialog);
  expect([laid.lines, laid.clamped]).toEqual([1, false]);
  expect(laid.line.y + laid.line.height).toBeLessThanOrEqual(laid.reader!.y);
  expect(await shiftAfter(page, frame.at)).toBeLessThan(0.01);
  await page.screenshot({ path: testInfo.outputPath("reply-to-reply-1280.png") });
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(button).toBeFocused();
});

test("at 390, a tap on Reply shows the post in the sheet, smaller, with the box focused by the next frame and nothing moving", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await openPost(page);
  const frame = await press(page, page.locator(".post").getByRole("button", { name: "Reply to @alice" }), true);
  expect(frame.focus).toBe("Cast your reply");
  const laid = await showsThePost(page, { avatar: 32, thumb: 64, lines: 3 });
  // The sheet is the screen's width, and nothing in it runs off it.
  expect(Math.round(laid.dialog!.x)).toBe(0);
  expect(Math.round(laid.dialog!.width)).toBe(390);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  const meta = await page.locator(".reply-parent-meta").evaluate((el) => ({ scroll: el.scrollWidth, client: el.clientWidth }));
  expect(meta.scroll).toBeLessThanOrEqual(meta.client + 1);
  expect(await shiftAfter(page, frame.at)).toBeLessThan(0.01);
  expect(await axeViolations(page, ".compose-modal")).toEqual([]);
  await page.screenshot({ path: testInfo.outputPath("reply-390.png") });

  // A reply's Reply, the same way.
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog", { name: "Reply" })).toHaveCount(0);
  const again = await press(page, page.locator(".comment").getByRole("button", { name: "Reply to @bob" }), true);
  expect(again.focus).toBe("Cast your reply");
  await expect(page.locator(".reply-parent-text")).toHaveText("Hyperion, then the Fall");
  await page.screenshot({ path: testInfo.outputPath("reply-to-reply-390.png") });
  await context.close();
});

test("signed out, the reader's place beside the box is an empty picture, and a draft kept for the same cast comes back", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await openPost(page, false);
  await page.locator(".post").getByRole("button", { name: "Reply to @alice" }).click();
  const dialog = page.getByRole("dialog", { name: "Reply" });
  await expect(dialog.locator(".reply-row .avatar-empty")).toBeVisible();
  await expect(dialog.locator(".reply-parent-handle")).toHaveText("@alice");
  await page.keyboard.type("Kept for later");
  await page.keyboard.press("Escape");
  await page.locator(".post").getByRole("button", { name: "Reply to @alice" }).click();
  await expect(dialog.getByRole("textbox", { name: "Cast your reply" })).toHaveText("Kept for later");
  await expect(dialog.locator(".reply-parent")).toBeVisible();
});

/** Whether the page has fetched the composer's part of the app. */
const composerCame = (page: Page) =>
  page.evaluate(() => performance.getEntriesByType("resource").some((e) => e.name.includes("split_composer")));

for (const width of [1280, 390] as const) {
  test(`at ${width}, signed in, the composer's code comes while the page is idle: Reply opens with its bar and the box focused in its first frame, the second time too`, async ({ browser }, testInfo) => {
    const touch = width === 390;
    const context = await browser.newContext({
      baseURL: testInfo.project.use.baseURL,
      viewport: { width, height: touch ? 844 : 900 },
      isMobile: touch,
      hasTouch: touch,
    });
    const page = await context.newPage();
    await openPost(page);
    // A phone's slower CPU, as the smoothness budgets take it.
    if (touch) await (await context.newCDPSession(page)).send("Emulation.setCPUThrottlingRate", { rate: 2 });
    // Fetched once the page is idle, before any Reply is pointed at.
    await expect.poll(() => composerCame(page), { timeout: 10_000 }).toBe(true);
    await page.waitForTimeout(300);
    const button = page.locator(".post").getByRole("button", { name: "Reply to @alice" });
    const first = await press(page, button, touch);
    expect([first.focus, first.bar]).toEqual(["Cast your reply", true]);
    const dialog = page.getByRole("dialog", { name: "Reply" });
    await expect(dialog.locator(".compose-loading")).toHaveCount(0);
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    const second = await press(page, button, touch);
    expect([second.focus, second.bar]).toEqual(["Cast your reply", true]);
    const numbers = `first open: box focused ${first.focusMs?.toFixed(1)} ms, bar ${(await barMs(page)).toFixed(1)} ms after the click, first frame ${first.frameMs.toFixed(1)} ms; second: focused ${second.focusMs?.toFixed(1)} ms, frame ${second.frameMs.toFixed(1)} ms`;
    testInfo.annotations.push({ type: `open latency at ${width}`, description: numbers });
    console.log(`open latency at ${width}${touch ? " (CPU 2×)" : ""}: ${numbers}`);
    await context.close();
  });
}

test("signed out, nothing is fetched ahead: the composer's code comes with the first Reply", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await openPost(page, false);
  await page.waitForTimeout(2500);
  expect(await composerCame(page)).toBe(false);
  await page.locator(".post").getByRole("button", { name: "Reply to @alice" }).click();
  await expect(page.getByRole("dialog", { name: "Reply" }).locator(".compose-post:not(.compose-loading)")).toBeVisible();
  expect(await composerCame(page)).toBe(true);
});

test("the bar: 32 px tools on the left, a 40 px tap on a phone; the ring and Reply on the right", async ({ browser }, testInfo) => {
  for (const [width, tool] of [
    [1280, 32],
    [390, 40],
  ] as const) {
    const touch = width === 390;
    const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width, height: 844 }, isMobile: touch, hasTouch: touch });
    const page = await context.newPage();
    await openPost(page);
    const button = page.locator(".post").getByRole("button", { name: "Reply to @alice" });
    await (touch ? button.tap() : button.click());
    const dialog = page.getByRole("dialog", { name: "Reply" });
    const bar = dialog.locator(".compose-bar");
    await expect(bar.getByRole("button", { name: "Add an emoji" })).toBeVisible();
    const laid = await bar.evaluate((bar) => {
      const box = (el: Element) => el.getBoundingClientRect();
      const tools = [...bar.querySelectorAll(".compose-tool")].map((el) => [Math.round(box(el).width), Math.round(box(el).height)]);
      const ring = box(bar.querySelector(".length")!);
      const post = box(bar.querySelector(".compose-post")!);
      const last = box([...bar.querySelectorAll(".compose-tool")].pop()!);
      return { tools, ringBeforePost: ring.right <= post.left, gap: ring.left - last.right, end: Math.abs(Math.round(box(bar).right - post.right)) };
    });
    for (const size of laid.tools) expect(size).toEqual([tool, tool]);
    // The ring sits right before Reply, away from the tools, Reply at the end.
    expect(laid.ringBeforePost).toBe(true);
    expect(laid.gap).toBeGreaterThan(40);
    expect(laid.end).toBe(0);
    await expect(dialog.locator(".length-ring")).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath(`bar-${width}.png`) });
    await context.close();
  }
});
