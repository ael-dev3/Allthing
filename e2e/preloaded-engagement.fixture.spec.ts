// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const hash = (i: number) => `0x${i.toString(16).padStart(40, "0")}`;
const person = (fid: number, username: string, display_name: string) => ({ fid, username, display_name });
// Likes, recasts and replies of ten casts, newest first. Their text is long,
// so only cards of the first run of six (one batch) can be in view.
const counts = [[10, 2, 1], [7, 0, 0], [0, 0, 0], [1, 0, 0], [1, 0, 0], [1, 1, 0], [3, 3, 0], [3, 3, 0], [3, 3, 0], [3, 3, 0]];
const pool = counts.map(([likes, recasts, replies], i) => ({
  hash: hash(i),
  author: { fid: 100 + i, username: `author${i}` },
  text: Array.from({ length: 10 }, (_, p) => `Preloaded engagement fixture ${i}, paragraph ${p + 1}.`).join("\n\n"),
  ts: 1792235681 - i * 60,
  likes, recasts, replies,
}));
// Each preview is what /api/v1/reactions?…&limit=10 answers for its list.
const preview = (i: number, kind: "likes" | "recasts", users: object[], more = false) =>
  ({ hash: hash(i), kind, users, more, meta });
// The batch leaves out cast 1's likes, as when that read fails upstream.
const batch = {
  previews: [
    preview(0, "likes", [person(2, "batch-liker", "Batch Liker"), person(3, "early-bird", "Early Bird")], true),
    ...[3, 4, 5].map((i) => preview(i, "likes", [person(10 + i, `liker${i}`, `Liker ${i}`)])),
    preview(0, "recasts", [person(5, "re-caster", "Re Caster")]),
    preview(5, "recasts", [person(6, "other-caster", "Other Caster")]),
  ],
  meta,
};

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
        // A "Loading…" already replaced by the time this runs is still among the removed nodes.
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

test.beforeEach(async ({ page }) => {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  // "new" lists the payload's casts in its order.
  await page.route("**/api/v1/latest", (route) => route.fulfill({ json: { pool, meta } }));
  await page.route("**/api/v1/engagement?*", (route) => route.fulfill({ json: batch }));
  await page.route("**/api/v1/reactions?*", (route) => route.fulfill({ json: {
    hash: hash(1), kind: "likes", users: [person(9, "on-hover", "Fetched On Hover")], meta,
  } }));
});

// Preloading is for pointers that hover; touch screens load popovers on tap.
test.beforeEach(({}, testInfo) => {
  test.skip(Boolean(testInfo.project.use.hasTouch), "preloading needs a hovering pointer");
});

test("cards in view preload who liked and recasted them in one batch, and their popovers open with it", async ({ page }) => {
  await watchPopovers(page);
  const engagement = requests(page, "/api/v1/engagement");
  const reactions = requests(page, "/api/v1/reactions");
  await page.goto("/?sort=new");
  await expect(page.locator(".cast-card[data-hash]")).toHaveCount(10);

  // One batch for the run of six cards in view, with a list only for
  // counts above zero; the second run is out of view.
  await expect.poll(() => engagement.length).toBe(1);
  const asked = new URL(engagement[0]).searchParams;
  expect(asked.get("likes")?.split(",")).toEqual([0, 1, 3, 4, 5].map(hash));
  expect(asked.get("recasts")?.split(",")).toEqual([0, 5].map(hash));
  // Batches are at least 2 s apart, and the run in view has its batch: nothing follows.
  await page.waitForTimeout(2500);
  expect(engagement).toHaveLength(1);

  // The names are there in the frame the popover opens, without a request of its own.
  const card = page.locator(`.cast-card[data-hash="${hash(0)}"]`);
  await card.locator(".stat-likes").hover();
  const liked = card.getByRole("dialog", { name: "Liked by", exact: true });
  await expect(liked.locator(".who-list li")).toHaveCount(2);
  await expect(liked).toContainText("Batch Liker");
  await expect(liked.getByRole("button", { name: "See everyone →" })).toBeVisible();
  await card.locator(".stat-recasts").hover();
  const recasted = card.getByRole("dialog", { name: "Recasted by", exact: true });
  await expect(recasted).toContainText("Re Caster");
  expect(reactions).toEqual([]);
  const opened = await openings(page);
  expect(opened.length).toBeGreaterThanOrEqual(2);
  expect(opened[0]).toContain("Batch Liker");
  expect(opened.filter((text) => text.includes("Loading…"))).toEqual([]);
  expect(await loadingShown(page)).toBe(0);

  // A list the batch didn't bring loads when its popover opens, as before.
  const other = page.locator(`.cast-card[data-hash="${hash(1)}"]`);
  await other.locator(".stat-likes").hover();
  const fetched = other.getByRole("dialog", { name: "Liked by", exact: true });
  await expect(fetched).toContainText("Fetched On Hover");
  expect(reactions).toEqual([expect.stringContaining(`/api/v1/reactions?hash=${hash(1)}&type=likes&limit=10`)]);
  expect(engagement).toHaveLength(1);
});

test("the replies preview comes from the post that hovering its card prefetched", async ({ page }) => {
  await watchPopovers(page);
  const replies = requests(page, "/api/v1/replies");
  const reply = (n: number, text: string, likes: number) => ({
    hash: `0x${n.toString(16).padStart(40, "f")}`,
    author: { fid: 200 + n, username: `replier${n}`, display_name: `Replier ${n}` },
    text, ts: 1792235700 + n, parent_hash: hash(0), likes,
  });
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
    thread: {
      cast: pool[0],
      children: [
        { cast: reply(1, "A quiet reply from the post's own thread", 0) },
        { cast: reply(2, "The most liked reply comes first", 4) },
      ],
    },
    meta,
  } }));
  // Hovering the card prefetches its post (cards in view may also be
  // prefetched while idle): either way, its post payload is in.
  const post = page.waitForResponse((response) => response.url().includes(`/api/v1/cast?hash=${hash(0)}`));
  await page.goto("/?sort=new");
  await expect(page.locator(".cast-card[data-hash]")).toHaveCount(10);
  const card = page.locator(`.cast-card[data-hash="${hash(0)}"]`);
  await card.hover();
  await post;
  await card.locator(".stat-replies").hover();
  const panel = card.getByRole("dialog", { name: "Replies", exact: true });
  await expect(panel.locator(".reply-item")).toHaveCount(2);
  await expect(panel.locator(".reply-item").first()).toContainText("The most liked reply comes first");
  expect(replies).toEqual([]);
  expect(await loadingShown(page)).toBe(0);
});

test("with Save-Data nothing is preloaded, and popovers load when they open", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(Navigator.prototype, "connection", {
      configurable: true,
      get: () => ({ saveData: true, effectiveType: "4g" }),
    });
  });
  const engagement = requests(page, "/api/v1/engagement");
  const reactions = requests(page, "/api/v1/reactions");
  await page.goto("/?sort=new");
  await expect(page.locator(".cast-card[data-hash]")).toHaveCount(10);
  await page.waitForTimeout(1000);
  const card = page.locator(`.cast-card[data-hash="${hash(1)}"]`);
  await card.locator(".stat-likes").hover();
  await expect(card.getByRole("dialog", { name: "Liked by", exact: true })).toContainText("Fetched On Hover");
  expect(reactions).toHaveLength(1);
  expect(engagement).toEqual([]);
});
