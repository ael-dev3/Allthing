// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";

/*
 * A post page gives one count of its replies, in the post's row and over
 * the replies: its direct replies, as its card in the feeds says, never
 * fewer than it lists. Under the post and under each reply, the counts and
 * Reply, Quote and React keep to one line, with room to spare: on a phone
 * four levels of replies down (three in a busy thread), where Reply and
 * Quote are their icons, as React is, with their names for screen readers,
 * and a reply's counts are as the Farcaster app shows them (1.2k); on a
 * computer, five. A reply's time is its link, with no "permalink" beside
 * it.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const bob = { fid: 7, username: "bob", display_name: "Bob" };
type Node = { cast: object; children: Node[] };
type Counts = { likes: number; recasts: number; replies: number };
/** A reply's counts, and a busy thread's: "1.2k", "345", "12". */
const usual: Counts = { likes: 12, recasts: 3, replies: 2 };
const busy: Counts = { likes: 1234, recasts: 345, replies: 12 };
const reply = (counts: Counts, n: number, author: object, parent: string, text: string, children: Node[] = []): Node => ({
  cast: { hash: hex(n), author, text, ts: ts + n * 60, ...counts, replies: Math.max(children.length, counts.replies), parent_hash: parent },
  children,
});
/** Three direct replies, the first with a chain of four under it, five levels in all: seven replies. */
const thread = (root: string, counts: Counts): Node[] => {
  const r = (...args: [number, object, string, string, Node[]?]) => reply(counts, ...args);
  return [
    r(10, bob, root, "A first reply, with a chain of replies under it.", [
      r(11, alice, hex(10), "A reply to it.", [
        r(12, bob, hex(11), "Deeper.", [r(13, alice, hex(12), "Deeper still.", [r(14, bob, hex(13), "Four deep.")])]),
      ]),
    ]),
    r(15, alice, root, "A second reply."),
    r(16, bob, root, "A third reply."),
  ];
};
const post = { hash: hex(1), author: alice, text: "A post with a thread under it", ts, likes: 1234, recasts: 56, replies: 3 };
// A count that hasn't caught up: one reply, where three are listed.
const lagging = { ...post, hash: hex(2), text: "A post whose reply count lags", replies: 1 };

async function serve(page: Page, counts: Counts = usual) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/cast?*", (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash");
    const cast = hash === lagging.hash ? lagging : post;
    return route.fulfill({ json: { thread: { cast, children: thread(cast.hash, counts) }, notes: {}, meta } });
  });
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true,
  });
  return { context, page: await context.newPage() };
}

/** How much room a row has to spare on one line: its width, less its items' widths and the gaps between them. */
async function spare(row: Locator) {
  return row.evaluate((row) => {
    const items = [...row.children].filter((child) => child.getBoundingClientRect().width > 0);
    const gap = parseFloat(getComputedStyle(row).columnGap) || 0;
    const used = items.reduce((width, item) => width + item.getBoundingClientRect().width, 0) + gap * (items.length - 1);
    return row.clientWidth - used;
  });
}

/** How many lines a row's items take: their tops, a line apart or more. */
async function lines(row: Locator) {
  return row.evaluate((row) => {
    const tops = [...row.children]
      .map((child) => child.getBoundingClientRect())
      .filter((box) => box.width > 0 && box.height > 0)
      .map((box) => box.top + box.height / 2)
      .sort((a, b) => a - b);
    return tops.reduce((n, top, i) => (i > 0 && top - tops[i - 1] > 12 ? n + 1 : n), tops.length ? 1 : 0);
  });
}

async function oneCount(page: Page, hash: string, n: string) {
  await page.goto(`/p/${hash}`);
  await expect(page.locator(".comment")).toHaveCount(7);
  await expect(page.locator(".post .stat-replies .n")).toHaveText(n);
  await expect(page.locator(".replies-head h2 .num")).toHaveText(n);
  await expect(page.locator(".replies-head h2")).toHaveText(`Replies${n}`);
}

test("the post's row and the heading over its replies give one count: its direct replies", async ({ page }) => {
  await serve(page);
  await oneCount(page, post.hash, "3");
  // A count behind the replies listed counts those listed.
  await oneCount(page, lagging.hash, "3");
});

/** The rows of counts and buttons, the post's and each reply's, in order, with each reply's depth (0 for a direct reply). */
async function feet(page: Page) {
  const rows = [page.locator(".post .cast-foot"), ...(await page.locator(".comment-foot").all())];
  expect(rows).toHaveLength(8);
  const depths = [-1, ...(await page.locator(".comment-foot").evaluateAll((all) => all.map((foot) => {
    let depth = 0;
    for (let p = foot.closest(".comment")?.parentElement; p; p = p.parentElement) if (p.classList.contains("comment")) depth += 1;
    return depth;
  })))];
  return rows.map((row, i) => ({ row, depth: depths[i] }));
}

/** Each row of counts and buttons on one line, the post's and each reply's at depth `deepest` or less; and what Reply and Quote show. */
async function rows(page: Page, { icons, deepest }: { icons: boolean; deepest: number }) {
  for (const [i, { row: foot, depth }] of (await feet(page)).entries()) {
    await foot.scrollIntoViewIfNeeded();
    if (depth <= deepest) {
      expect(await lines(foot), `row ${i}, at depth ${depth}: one line`).toBe(1);
      // With room to spare: a font a little wider elsewhere keeps it on one line.
      expect(await spare(foot), `row ${i}, at depth ${depth}: room to spare`).toBeGreaterThanOrEqual(8);
    }
    const reply = foot.getByRole("button", { name: /^Reply to @/ });
    const quote = foot.getByRole("button", { name: /^Quote @/ });
    await expect(reply).toBeVisible();
    await expect(quote).toBeVisible();
    await expect(foot.getByRole("button", { name: /^React to @/ })).toBeVisible();
    for (const [button, word] of [[reply, "Reply"], [quote, "Quote"]] as const) {
      if (icons) await expect(button.getByText(word, { exact: true }), `row ${i}: ${word} is its icon`).toBeHidden();
      else await expect(button.getByText(word, { exact: true }), `row ${i}: ${word} has its word`).toBeVisible();
    }
  }
  // No "permalink": a reply's time is its link, and says so.
  await expect(page.getByRole("link", { name: "permalink" })).toHaveCount(0);
  const when = page.locator(".comment-head a.when").first();
  await expect(when).toHaveAttribute("href", `/p/${hex(10)}`);
  await expect(when).toHaveAccessibleName(/^Open reply, /);
}

test("on a phone, each row of counts and buttons keeps to one line four levels of replies down, Reply and Quote as icons", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await serve(page);
  await page.goto(`/p/${post.hash}`);
  await expect(page.locator(".comment")).toHaveCount(7);
  // A reply's counts: the post's row and the replies' four levels down (of five) on one line.
  await rows(page, { icons: true, deepest: 3 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await page.locator(".replies").scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath("post-replies-390.png"), fullPage: true });
  await context.close();
});

test("on a phone, a busy thread's rows keep to one line three levels down", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await serve(page, busy);
  await page.goto(`/p/${post.hash}`);
  await expect(page.locator(".comment")).toHaveCount(7);
  await rows(page, { icons: true, deepest: 2 });
  // A reply's counts are as the Farcaster app shows them, and read out in full; the post's are whole.
  const likes = page.locator(".comment-foot .stat-likes .popover-trigger").first();
  await expect(likes.locator(".n")).toHaveText("1.2k");
  await expect(likes).toHaveAccessibleName("1,234 likes: show who");
  await expect(page.locator(".post .stat-likes .n")).toHaveText("1,234");
  await context.close();
});

test("on a computer, Reply and Quote keep their words, and each row keeps to one line, five levels down with a busy thread's counts", async ({ page }, testInfo) => {
  test.skip(Boolean(testInfo.project.use.hasTouch), "a computer's width");
  await page.setViewportSize({ width: 1280, height: 900 });
  await serve(page, busy);
  await page.goto(`/p/${post.hash}`);
  await expect(page.locator(".comment")).toHaveCount(7);
  await rows(page, { icons: false, deepest: 4 });
  await page.screenshot({ path: testInfo.outputPath("post-replies-1280.png"), fullPage: true });
});
