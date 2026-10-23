// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { appCame } from "./helpers/app";
import { NOW, isoAgo } from "./fixture-time";

/*
 * A feed card's replies come with its run's popover batch
 * (`/api/v1/engagement?…&replies=…`), filed under the card's own
 * `/api/v1/replies?hash=…`, so its replies popover opens with its rows,
 * hovered or tapped, without a request of its own. The Worker builds each
 * as `/replies` does, by the same code, leaving out spam and mini apps
 * (`pages.rs`, `hidden_replies_are_left_out_of_a_batch_as_replies_leaves_them_out`);
 * a card the batch has no replies for loads them when its popover opens.
 * The static copy, which builds payloads in the browser and preloads no
 * batches, shows that code leaving them out on a page.
 */

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const hash = (i: number) => `0x${i.toString(16).padStart(40, "0")}`;
const ts = (NOW - 19);
// Likes, recasts and replies of eight casts, newest first. Their text is
// long, so only cards of the first run of six (one batch) can be in view.
// Cast 0 counts a third reply, spam, which the Worker leaves out; cast 2
// has too many replies for a batch to read them; cast 3's replies the
// batch leaves out, as when they don't fit or their read fails.
const counts = [[4, 1, 3], [2, 0, 0], [1, 0, 150], [1, 1, 1], [1, 0, 2], [1, 0, 1], [3, 3, 2], [3, 3, 2]];
const pool = counts.map(([likes, recasts, replies], i) => ({
  hash: hash(i),
  author: { fid: 100 + i, username: `author${i}` },
  text: Array.from({ length: 10 }, (_, p) => `Preloaded replies fixture ${i}, paragraph ${p + 1}.`).join("\n\n"),
  ts: ts - i * 60,
  likes, recasts, replies,
}));
const replier = (fid: number) => ({ fid, username: `replier${fid}`, display_name: `Replier ${fid}` });
const reply = (n: number, of: number, text: string, likes: number) => ({
  hash: `0x${n.toString(16).padStart(40, "f")}`,
  author: replier(200 + n), text, ts: ts + n * 60, parent_hash: hash(of), likes,
});
// What the Worker sends for each card it has the replies of: as /replies
// answers, the most engaged first, without the spam reply.
const batchReplies: Record<string, object> = {
  [hash(0)]: {
    hash: hash(0),
    replies: [reply(1, 0, "The batch's most liked reply", 5), reply(2, 0, "A quieter reply from the batch", 1)],
    notes: { users: { "201": { bio: "Replies first.", followers: 12 } } },
    meta,
  },
  [hash(4)]: { hash: hash(4), replies: [reply(3, 4, "Card four's reply", 2), reply(4, 4, "Another of card four", 0)], meta },
  [hash(5)]: { hash: hash(5), replies: [reply(5, 5, "Card five's only reply", 0)], meta },
};
const SPAM = "Just unlocked my DEGEN rewards";

type Watched = Window & { popoverOpenings: string[]; popoverLoading: number };

/** Record each popover panel's text as it is added, and every "Loading…" shown, however briefly. */
async function watchPopovers(page: Page) {
  await page.addInitScript(() => {
    const watched = window as unknown as Watched;
    watched.popoverOpenings = [];
    watched.popoverLoading = 0;
    const loading = (node: Node) =>
      node instanceof Element && (node.matches(".popover-loading") || node.querySelector(".popover-loading") !== null);
    new MutationObserver((records) => {
      for (const record of records) {
        for (const node of Array.from(record.addedNodes)) {
          if (!(node instanceof Element)) continue;
          const panels = node.matches(".popover-panel") ? [node] : Array.from(node.querySelectorAll(".popover-panel"));
          for (const panel of panels) watched.popoverOpenings.push(panel.textContent ?? "");
        }
        for (const node of [...Array.from(record.addedNodes), ...Array.from(record.removedNodes)]) {
          if (loading(node)) watched.popoverLoading += 1;
        }
      }
    }).observe(document, { childList: true, subtree: true });
  });
}

const openings = (page: Page) => page.evaluate(() => (window as unknown as Watched).popoverOpenings);
const loadingShown = (page: Page) => page.evaluate(() => (window as unknown as Watched).popoverLoading);

/** URLs of the requests whose URL contains `part`, as they are sent. */
function requests(page: Page, part: string): string[] {
  const seen: string[] = [];
  page.on("request", (request) => {
    if (request.url().includes(part)) seen.push(request.url());
  });
  return seen;
}

/** Serve "new" with the pool, and each batch for what it asks; the batches asked, as their query. */
async function serve(page: Page) {
  const batches: URLSearchParams[] = [];
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/latest", (route) => route.fulfill({ json: { pool, meta } }));
  await page.route("**/api/v1/engagement?*", (route) => {
    const asked = new URL(route.request().url()).searchParams;
    batches.push(asked);
    const list = (key: string) => asked.get(key)?.split(",") ?? [];
    const users = (h: string) => [{ fid: 9, username: `liker${parseInt(h, 16)}`, display_name: `Liker ${parseInt(h, 16)}` }];
    return route.fulfill({ json: {
      previews: [
        ...list("likes").map((h) => ({ hash: h, kind: "likes", users: users(h), more: false, meta })),
        ...list("recasts").map((h) => ({ hash: h, kind: "recasts", users: users(h), more: false, meta })),
      ],
      replies: list("replies").filter((h) => h in batchReplies).map((h) => batchReplies[h]),
      meta,
    } });
  });
  // What a popover would show had the batch not brought it: the post that
  // hovering or touching its card prefetches, and the replies on their own.
  await page.route("**/api/v1/cast?*", (route) => {
    const h = new URL(route.request().url()).searchParams.get("hash")!;
    const cast = pool.find((c) => c.hash === h) ?? pool[0];
    return route.fulfill({ json: {
      thread: { cast, children: [{ cast: reply(90, 0, "A reply from the post's own thread", 9) }] },
      meta,
    } });
  });
  await page.route("**/api/v1/replies?*", (route) => {
    const h = new URL(route.request().url()).searchParams.get("hash")!;
    return route.fulfill({ json: { hash: h, replies: [reply(91, 3, "Fetched when the popover opened", 1)], meta } });
  });
  // The repliers' and authors' card facts.
  await page.route("**/api/v1/vote?*", (route) => {
    const fid = Number(new URL(route.request().url()).searchParams.get("fid"));
    return route.fulfill({ json: { fid, onchain: "stay", app: "opted_in", meta } });
  });
  await page.route("**/api/v1/wallets?*", (route) => {
    const fid = Number(new URL(route.request().url()).searchParams.get("fid"));
    return route.fulfill({ json: { fid, addresses: [], chains: [], total: 0, sources: [], fetched_at: Date.now() } });
  });
  return batches;
}

test.describe("with the Worker", () => {
  test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

  for (const phone of [false, true]) {
    const width = phone ? 390 : 1280;
    test(`at ${width}, ${phone ? "a tap on" : "hovering"} a card's replies count opens the replies its batch brought, asking for nothing`, async ({ browser }, testInfo) => {
      const context = await browser.newContext({
        baseURL: testInfo.project.use.baseURL,
        ...(phone ? { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true } : { viewport: { width: 1280, height: 900 } }),
      });
      const page = await context.newPage();
      await watchPopovers(page);
      const batches = await serve(page);
      const replies = requests(page, "/api/v1/replies");
      await page.goto("/?sort=new");
      await appCame(page);
      await expect(page.locator(".cast-card[data-hash]")).toHaveCount(8);

      // One batch for the run of six in view, with the replies of the cards
      // that have some, short of a hundred.
      await expect.poll(() => batches.length).toBe(1);
      expect(batches[0].get("replies")?.split(",")).toEqual([0, 3, 4, 5].map(hash));
      expect(batches[0].get("likes")?.split(",")).toEqual([0, 1, 2, 3, 4, 5].map(hash));

      // The rows are there in the frame the popover opens, without a request of its own.
      const card = page.locator(`.cast-card[data-hash="${hash(0)}"]`);
      const count = card.getByRole("button", { name: "3 replies: preview", exact: true });
      if (phone) await count.tap();
      else await count.hover();
      const panel = card.getByRole("dialog", { name: "Replies", exact: true });
      const rows = panel.locator(".reply-items > li");
      await expect(rows).toHaveCount(2);
      await expect(rows.nth(0)).toContainText("The batch's most liked reply");
      await expect(rows.nth(0)).toContainText("Replier 201");
      await expect(rows.nth(1)).toContainText("A quieter reply from the batch");
      // The spam reply the cast counts shows nowhere, nor do the post's own
      // rows or those of /replies.
      await expect(panel).not.toContainText(SPAM);
      await expect(panel).not.toContainText("A reply from the post's own thread");
      expect(replies).toEqual([]);
      expect(await loadingShown(page)).toBe(0);
      const opened = (await openings(page)).filter((text) => text.startsWith("Replies"));
      expect(opened[0]).toContain("The batch's most liked reply");
      await page.screenshot({ path: testInfo.outputPath(`preloaded-replies-${width}.png`) });

      // The batch had no replies for cast 3: its popover reads them when it opens.
      if (phone) await page.keyboard.press("Escape");
      const other = page.locator(`.cast-card[data-hash="${hash(3)}"]`);
      await other.scrollIntoViewIfNeeded();
      const its = other.getByRole("button", { name: "1 reply: preview", exact: true });
      if (phone) await its.tap();
      else await its.hover();
      await expect(other.getByRole("dialog", { name: "Replies", exact: true })).toContainText(/Fetched when the popover opened|A reply from the post's own thread/);
      expect(replies.every((url) => url.includes(`hash=${hash(3)}`))).toBe(true);
      await context.close();
    });
  }
});

test.describe("in the static copy", () => {
  test.skip(!process.env.E2E_PAGES, "the static copy builds payloads in the browser");

  test("a replies preview built in the browser, by the code a batch builds replies with, leaves out spam and mini apps", async ({ page }) => {
    const cast = `0x${"b".repeat(40)}`;
    const cors = { "access-control-allow-origin": "*" };
    // Hypersnap's conversation of a cast with five direct replies: the most
    // liked are spam, a mini app by its address and one by its page.
    const raw = (n: number, likes: number, text: string, embeds: string[] = []) => ({
      hash: `0x${n.toString(16).padStart(40, "c")}`, parent_hash: cast, text, timestamp: isoAgo(3_600),
      author: { fid: 300 + n, username: `replier${n}`, display_name: `Replier ${n}` },
      embeds: embeds.map((url) => ({ url })), reactions: { likes_count: likes, recasts_count: 0 }, replies: { count: 0 },
    });
    const direct = [
      raw(1, 50, `${SPAM}, claim yours`),
      raw(2, 40, "Play my game", ["https://farcaster.xyz/miniapps/abc/hidden-game"]),
      raw(3, 30, "A frame on its own site", ["https://game.static-replies.example/play"]),
      raw(4, 9, "The most liked reply shown"),
      raw(5, 1, "A quieter reply, with a link", ["https://blog.static-replies.example/post"]),
    ];
    const conversations: string[] = [];
    await page.route("https://haatz.quilibrium.com/**", (route) => {
      const url = new URL(route.request().url());
      if (url.pathname.endsWith("/feed/trending")) {
        return route.fulfill({ json: { casts: url.searchParams.has("cursor") ? [] : [{
          hash: cast, author: { fid: 1, username: "fixture" }, text: "A cast with replies",
          timestamp: isoAgo(3_600), embeds: [], replies: { count: 5 }, reactions: { likes_count: 1, recasts_count: 0 },
        }] }, headers: cors });
      }
      if (url.pathname.endsWith("/cast/conversation") && url.searchParams.get("reply_depth") === "1") {
        conversations.push(url.searchParams.get("identifier") ?? "");
        return route.fulfill({ json: { conversation: { cast: { hash: cast, text: "A cast with replies", direct_replies: direct } } }, headers: cors });
      }
      return route.fulfill({ status: 404, json: {}, headers: cors });
    });
    await page.route("https://game.static-replies.example/**", (route) =>
      route.fulfill({ contentType: "text/html", body: `<html><head><meta name="fc:miniapp" content="{}"></head></html>`, headers: cors }));
    await page.route("https://blog.static-replies.example/**", (route) =>
      route.fulfill({ contentType: "text/html", body: "<html><head><title>A post</title></head></html>", headers: cors }));
    await page.goto("/Allthing/");
    await expect(page.getByText("A cast with replies", { exact: true })).toBeVisible();
    // Keyboard focus opens the preview without hovering the card, which
    // would build its post instead.
    const count = page.getByRole("button", { name: "5 replies: preview", exact: true });
    await page.keyboard.press("Tab");
    await count.focus();
    const panel = page.getByRole("dialog", { name: "Replies", exact: true });
    const rows = panel.locator(".reply-items > li");
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0)).toContainText("The most liked reply shown");
    await expect(rows.nth(1)).toContainText("A quieter reply, with a link");
    for (const hidden of [SPAM, "Play my game", "A frame on its own site"]) await expect(panel).not.toContainText(hidden);
    expect(conversations).toEqual([cast]);
  });
});
