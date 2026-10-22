// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Locator, type Page } from "@playwright/test";
import { appCame } from "./helpers/app";

/*
 * The usage bar under the front page's tabs, while the apps of most of its
 * casts are still being read (the Worker reads them in the background after
 * a list is built) and once they all are. Each known app is a share of all
 * the casts, not of the few known, and those not known yet are a light grey
 * segment at the bar's end, counted in the legend. The legend is one line
 * however much it lists, scrolling sideways where it doesn't fit, so the
 * row is as tall either way and nothing under it moves as the apps come.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const now = Math.floor(Date.now() / 1000);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const apps = {
  9152: { fid: 9152, username: "warpcast", display_name: "Warpcast" },
  2904: { fid: 2904, username: "wake", display_name: "wake" },
  309857: { fid: 309857, username: "baseapp.base.eth", display_name: "Base App" },
  6131: { fid: 6131, username: "fireflyapp", display_name: "Firefly App" },
  1325: { fid: 1325, username: "recaster", display_name: "Recaster" },
  4242: { fid: 4242, username: "herocast", display_name: "herocast" },
};
const pool = Array.from({ length: 30 }, (_, i) => ({
  hash: hex(100 + i),
  author: { fid: 10 + (i % 5), username: `reader${i % 5}`, display_name: `Reader ${i % 5}` },
  text: `Cast ${i}`,
  ts: now - 600 - i * 60,
  likes: 100 - i,
  recasts: 0,
  replies: 0,
}));
/** Each of the first `known` casts' apps, from `fids` in turn; the rest still being read. */
const notes = (known: number, fids: number[]) => ({
  origins: Object.fromEntries(pool.slice(0, known).map((c, i) => [c.hash, { signer: `0x${"ab".repeat(32)}`, app_fid: fids[i % fids.length] }])),
  apps,
  pending: pool.length - known,
});
// 7 in 10 from the Farcaster app, 3 from wake.
const twoApps = [9152, 9152, 9152, 2904, 9152, 9152, 2904, 9152, 9152, 2904];
const sixApps = [9152, 9152, 2904, 309857, 6131, 1325, 4242, 9152];

async function serve(page: Page, body: object) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const more = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: more ? { pool: [], notes: {}, meta } : { pool, notes: body, meta } });
  });
}

/** Once the entrances on the page have run: their transforms move what's measured. */
const settled = (page: Page) =>
  expect
    .poll(() =>
      page.evaluate(() =>
        document.getAnimations().filter((a) => a.effect?.getTiming().iterations !== Infinity).every((a) => a.playState === "finished"),
      ),
    )
    .toBe(true);

/** The usage bar's row and the first card, as laid out. */
const layout = (page: Page) =>
  page.evaluate(() => {
    const box = (s: string) => document.querySelector(s)!.getBoundingClientRect();
    return { row: Math.round(box("#app .client-share").height), card: Math.round(box("#app .cast-list .cast-card[data-hash]").top + scrollY) };
  });

const region = (page: Page) => page.getByRole("region", { name: "Apps posting today's trending casts", exact: true });
const legend = (bar: Locator) => bar.locator(".client-legend li");

const sizes = [
  { name: "1280", use: { viewport: { width: 1280, height: 900 } } },
  { name: "390", use: { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true } },
];

for (const size of sizes) {
  test.describe(`at ${size.name}`, () => {
    test.use(size.use);

    test("while most apps are being read, the known are shares of all the casts, and the rest show in grey", async ({ page }) => {
      await serve(page, notes(10, twoApps));
      await page.goto("/");
      await appCame(page);
      const bar = region(page);
      // 7 and 3 of 30, not 70% and 30% of the 10 known.
      await expect(legend(bar)).toHaveText([/^Farcaster\s*23%$/, /^wake\s*10%$/, /^20 not known yet$/]);
      await expect(bar.getByRole("img")).toHaveAttribute("aria-label", "Farcaster 23%, wake 10%, 20 not known yet");
      const unknown = bar.locator(".share-bar > .seg-unknown");
      await expect(unknown).toHaveAttribute("style", /flex-grow: 67/);
      // The grey is the bar's last segment, and two thirds of it.
      expect(await unknown.evaluate((seg) => seg === seg.parentElement!.lastElementChild)).toBe(true);
      const [whole, part] = await Promise.all([bar.locator(".share-bar").boundingBox(), unknown.boundingBox()]);
      expect(part!.width / whole!.width).toBeGreaterThan(0.6);
      expect(part!.width / whole!.width).toBeLessThan(0.72);
      // It names no app: it is no link.
      await expect(bar.locator(".client-unknown a")).toHaveCount(0);
    });

    test("the row is as tall, and the feed where it was, whether the apps are being read or all known, with a long legend too", async ({ page }) => {
      const seen: { row: number; card: number }[] = [];
      for (const body of [notes(10, twoApps), notes(30, twoApps), notes(16, sixApps), notes(30, sixApps)]) {
        await serve(page, body);
        await page.goto("/");
        await appCame(page);
        await expect(region(page)).toBeVisible();
        await settled(page);
        seen.push(await layout(page));
        await page.unrouteAll({ behavior: "ignoreErrors" });
      }
      // All known: no grey, no count.
      expect(seen.map((s) => s.row)).toEqual(seen.map(() => seen[0].row));
      expect(seen.map((s) => s.card)).toEqual(seen.map(() => seen[0].card));
    });

    test("a legend longer than its line stays one line, and scrolls sideways to the rest", async ({ page }) => {
      await serve(page, notes(16, sixApps));
      await page.goto("/");
      await appCame(page);
      const bar = region(page);
      const items = legend(bar);
      // Four apps by name, the other two together, and those not known yet.
      await expect(items).toHaveCount(6);
      await expect(items.last()).toHaveText("14 not known yet");
      const tops = await items.evaluateAll((lis) => lis.map((li) => Math.round(li.getBoundingClientRect().top)));
      expect(new Set(tops).size).toBe(1);
      const list = bar.locator(".client-legend");
      if (size.name === "390") {
        // More than a phone's line: the rest is a swipe away, not cut off.
        expect(await list.evaluate((el) => el.scrollWidth > el.clientWidth)).toBe(true);
        await items.last().scrollIntoViewIfNeeded();
        const [line, last] = await Promise.all([list.boundingBox(), items.last().boundingBox()]);
        expect(last!.x + last!.width).toBeLessThanOrEqual(line!.x + line!.width + 1);
      }
      // An app's link still filters the feed.
      await expect(items.first().getByRole("link")).toHaveAttribute("href", "/?client=9152");
    });
  });
}
