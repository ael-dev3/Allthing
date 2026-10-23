// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Locator, type Page } from "@playwright/test";
import { appCame } from "./helpers/app";

/*
 * An image a cast shows alone has its box before it comes, so nothing moves
 * when it does: on a post's page loaded cold, and in the front page's lists
 * after another time range or the new tab. Cloudflare Images describes an
 * image's size in a small JSON answer, asked for first; for an image hosted
 * elsewhere, the card waits for the image's own first bytes. Here the
 * images come late, as on a slow line: those on Cloudflare Images 1.5 s
 * after they are asked for, the other 0.4 s. Layout shift is read from the
 * browser's own record of it (a buffered PerformanceObserver), with the
 * positions of what is below the images.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const now = Math.floor(Date.now() / 1000);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const author = { fid: 7, username: "lens", display_name: "Lens" };
/** Each fixture image's size, as its 720 px variant has it. */
const SIZES: Record<string, [number, number]> = {
  wide: [720, 405],
  tall: [720, 960],
  square: [720, 720],
  panorama: [720, 240],
  photo: [1200, 800],
};
const image = (name: string, size: string) => ({ kind: "image", url: `https://imagedelivery.net/fixture/${name}-${size}/original` });
const elsewhere = (name: string, size: string) => ({ kind: "image", url: `https://media.example/${name}-${size}.svg` });
const card = (n: number, text: string, embeds: object[] = []) => ({
  hash: hex(n),
  author,
  text,
  ts: now - 600 - (n % 100) * 60,
  // Hot and new both list them in this order.
  likes: 1000 - n,
  recasts: 0,
  replies: 0,
  embeds,
});
const list = (base: number, label: string) => [
  card(base + 1, `${label}: a wide photo`, [image(`${label}-1`, "wide")]),
  card(base + 2, `${label}: words only`),
  card(base + 3, `${label}: a tall one`, [image(`${label}-3`, "tall")]),
  card(base + 4, `${label}: hosted elsewhere`, [elsewhere(`${label}-4`, "photo")]),
  card(base + 5, `${label}: a square`, [image(`${label}-5`, "square")]),
  card(base + 6, `${label}: more words`),
  card(base + 7, `${label}: a panorama`, [image(`${label}-7`, "panorama")]),
  card(base + 8, `${label}: the last words`),
];
const pools: Record<string, object[]> = {
  "": Array.from({ length: 8 }, (_, i) => card(1 + i, `Today ${i}: words only`)),
  "1h": list(100, "hour"),
  "7d": list(200, "week"),
};
const latest = list(300, "new");
const post = card(400, "A walk in the park");
post.embeds = [image("post", "tall")];
const replies = [
  card(401, "What a view"),
  card(402, "Mine from this morning", [image("reply", "wide")]),
  ...Array.from({ length: 6 }, (_, i) => card(403 + i, `Reply ${i}`)),
].map((reply) => ({ cast: { ...reply, parent_hash: post.hash } }));

const svg = ([w, h]: [number, number]) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}"><rect width="${w}" height="${h}" fill="#4f43d4"/></svg>`;
const later = (ms: number) => new Promise((done) => setTimeout(done, ms));
const sizeOf = (name: string) => SIZES[name.split("-").pop()!];

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const url = new URL(route.request().url());
    const pool = url.searchParams.has("offset") ? [] : (pools[url.searchParams.get("window") ?? ""] ?? []);
    return route.fulfill({ json: { pool, notes: {}, meta } });
  });
  await page.route("**/api/v1/latest**", (route) => route.fulfill({ json: { pool: latest, notes: {}, meta } }));
  await page.route("**/api/v1/cast?*", (route) =>
    route.fulfill({ json: { thread: { cast: post, children: replies }, quotes: [], notes: {}, meta } }),
  );
  const cors = { "access-control-allow-origin": "*" };
  await page.route("https://imagedelivery.net/**", async (route) => {
    const [, , name, variant] = new URL(route.request().url()).pathname.split("/");
    const [width, height] = sizeOf(name);
    if (variant === "fit=scale-down,w=720,format=json") {
      await later(50);
      return route.fulfill({ json: { width, height, original: { width: width * 2, height: height * 2 } }, headers: cors });
    }
    await later(1500);
    return route.fulfill({ body: svg([width, height]), contentType: "image/svg+xml", headers: cors });
  });
  await page.route("https://media.example/**", async (route) => {
    const name = new URL(route.request().url()).pathname.slice(1).replace(/\.svg$/, "");
    await later(400);
    return route.fulfill({ body: svg(sizeOf(name)), contentType: "image/svg+xml" });
  });
}

/** Records layout shifts from the start, with whether each moved only the footer. */
async function watchShifts(page: Page) {
  await page.addInitScript(() => {
    const shifts: { value: number; input: boolean; footer: boolean }[] = [];
    (window as unknown as { __shifts: typeof shifts }).__shifts = shifts;
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries() as unknown as { value: number; hadRecentInput: boolean; sources: { node?: Node }[] }[]) {
        const footer = entry.sources.length > 0 && entry.sources.every((s) => s.node instanceof Element && s.node.closest(".site-footer") !== null);
        shifts.push({ value: entry.value, input: entry.hadRecentInput, footer });
      }
    }).observe({ type: "layout-shift", buffered: true });
  });
}

const shiftCount = (page: Page) => page.evaluate(() => (window as unknown as { __shifts: unknown[] }).__shifts.length);

/** Layout shift since the `from`th entry, as CLS counts it (not just after
 * input). The footer's own move as a page's content takes its placeholder's
 * place is the page's height while loading, not an image's: not counted. */
const shiftSince = (page: Page, from = 0) =>
  page.evaluate(
    (from) =>
      (window as unknown as { __shifts: { value: number; input: boolean; footer: boolean }[] }).__shifts
        .slice(from)
        .filter((s) => !s.input && !s.footer)
        .reduce((sum, s) => sum + s.value, 0),
    from,
  );

/** Notes, for each lone image's tile added from now on, whether it had its
 * box as it was added, and whether it was in view then. */
const watchTiles = (page: Page) =>
  page.evaluate(() => {
    const seen = new Map<Element, { src: string; sized: boolean; inView: boolean }>();
    (window as unknown as { __sizedAtInsert: typeof seen }).__sizedAtInsert = seen;
    new MutationObserver((records) => {
      for (const node of records.flatMap((r) => [...r.addedNodes])) {
        if (!(node instanceof Element)) continue;
        for (const tile of [node, ...node.querySelectorAll(".media-grid.n1 .tile-image")]) {
          if (!tile.matches(".media-grid.n1 .tile-image") || seen.has(tile)) continue;
          const src = tile.querySelector("img")?.getAttribute("src") ?? "";
          seen.set(tile, { src, sized: tile.hasAttribute("data-sized"), inView: tile.getBoundingClientRect().top < innerHeight });
        }
      }
    }).observe(document.body, { childList: true, subtree: true });
  });

const tilesAdded = (page: Page) =>
  page.evaluate(() => [...(window as unknown as { __sizedAtInsert: Map<Element, { src: string; sized: boolean; inView: boolean }> }).__sizedAtInsert.values()]);

/** Once the entrances on the page have run. */
const settled = (page: Page) =>
  expect
    .poll(() =>
      page.evaluate(() =>
        document
          .getAnimations()
          .filter((a) => a.effect?.getTiming().iterations !== Infinity)
          .every((a) => a.playState === "finished"),
      ),
    )
    .toBe(true);

/** Once every lone image in `scope` that is in view, or near enough to load, has come. */
const imagesIn = (scope: Locator) =>
  expect
    .poll(
      () =>
        scope.locator(".media-grid.n1 .tile-image img").evaluateAll((imgs) => {
          const near = imgs.filter((img) => img.getBoundingClientRect().top < innerHeight + 800);
          return near.length > 0 && near.every((img) => (img as HTMLImageElement).naturalWidth > 0);
        }),
      { timeout: 15_000 },
    )
    .toBe(true);

for (const width of [1280, 390]) {
  test.describe(`at width ${width}`, () => {
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 900 });
      await watchShifts(page);
      await serve(page);
    });

    test("a post loaded cold has its image's box before the image, and its replies stay put as it comes", async ({ page }) => {
      await page.goto(`/p/${post.hash}`);
      await expect(page.locator(".post .post-text")).toBeVisible();
      const tile = page.locator(".post .tile-image");
      // The image is still on its way, and its tile already has its box.
      expect(await tile.locator("img").evaluate((img) => (img as HTMLImageElement).naturalWidth)).toBe(0);
      expect(await tile.getAttribute("data-sized")).toBe("");
      const [w, h] = SIZES.tall;
      const box = (await tile.boundingBox())!;
      expect(Math.abs(box.height - (box.width * h) / w)).toBeLessThanOrEqual(2);
      // Where its replies start, once the page's entrance has run (its
      // transforms move what's measured), and the image still hasn't come.
      await settled(page);
      expect(await tile.locator("img").evaluate((img) => (img as HTMLImageElement).naturalWidth)).toBe(0);
      const below = () => page.evaluate(() => document.querySelector("section.replies")!.getBoundingClientRect().top + scrollY);
      const before = await below();
      await imagesIn(page.locator(".post"));
      await expect(page.locator("section.replies .comment").first()).toBeVisible();
      await imagesIn(page.locator("section.replies"));
      await page.waitForTimeout(300);
      expect(await below()).toBeCloseTo(before, 0);
      expect(await shiftSince(page)).toBeLessThan(0.05);
    });

    test("the front page's cards stay put as their images come, after another time range and the new tab", async ({ page }) => {
      await page.goto("/");
      await appCame(page);
      await expect(page.locator(".cast-card[data-hash]").first()).toContainText("Today 0");
      await watchTiles(page);
      const feed = page.locator(".cast-list").first();
      const range = page.getByRole("combobox", { name: "Time range" });
      for (const [value, label] of [["hour", "hour"], ["week", "week"]]) {
        const from = await shiftCount(page);
        await range.selectOption(value);
        await expect(page.locator(".cast-card[data-hash]").first()).toContainText(`${label}: a wide photo`);
        await imagesIn(feed);
        await page.waitForTimeout(300);
        expect(await shiftSince(page, from), `after ${value}`).toBeLessThan(0.05);
      }
      const from = await shiftCount(page);
      await page.locator(".sort-tabs a", { hasText: /^new$/ }).click();
      await expect(page.locator(".cast-card[data-hash]").first()).toContainText("new: a wide photo");
      await imagesIn(feed);
      await page.waitForTimeout(300);
      expect(await shiftSince(page, from), "after new").toBeLessThan(0.05);
      // Each lone image's tile drawn in view had its box as it was drawn.
      const tiles = await tilesAdded(page);
      expect(tiles.filter((t) => t.inView).length).toBeGreaterThanOrEqual(6);
      expect(tiles.filter((t) => t.inView && !t.sized)).toEqual([]);
    });
  });
}
