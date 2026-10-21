// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Locator, type Page } from "@playwright/test";
import { appCame } from "./helpers/app";

/*
 * A page keeps its layout as its data comes, and as the reader switches
 * between lists. A channel's page has its sidebar's place while it loads,
 * so the feed beside it doesn't move over when the channel comes (it moved
 * 165 px left at 1280 px). And new shows the apps its casts came from, as
 * hot shows its trending casts', so switching between them doesn't move
 * the list (it moved about 45 px up, and back down).
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const now = Math.floor(Date.now() / 1000);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const apps = {
  9152: { fid: 9152, username: "farcaster", display_name: "Farcaster" },
  2904: { fid: 2904, username: "wake", display_name: "wake" },
};
const list = (base: number, label: string) =>
  Array.from({ length: 30 }, (_, i) => ({
    hash: hex(base + i),
    author: { fid: 10 + (i % 5), username: `reader${i % 5}`, display_name: `Reader ${i % 5}` },
    text: `${label} ${i}`,
    ts: now - 600 - i * 60,
    likes: 100 - i,
    recasts: 0,
    replies: 0,
  }));
/** Each cast's app: two in three from the Farcaster app, one from wake. */
const origins = (casts: { hash: string }[]) =>
  Object.fromEntries(casts.map((c, i) => [c.hash, { signer: `0x${"ab".repeat(32)}`, app_fid: i % 3 === 2 ? 2904 : 9152 }]));
const hot = list(100, "Hot");
const latest = list(200, "New");

/** The front page's lists, at once; a channel once `release` is called. */
async function serve(page: Page) {
  let release: () => void = () => {};
  const released = new Promise<void>((done) => (release = done));
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const more = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: more ? [] : hot, notes: { origins: origins(hot), apps }, meta } });
  });
  await page.route("**/api/v1/latest**", (route) => route.fulfill({ json: { pool: latest, notes: { origins: origins(latest), apps }, meta } }));
  await page.route("**/api/v1/channel?*", async (route) => {
    await released;
    const channel = { id: "fixture", name: "Fixture Channel", parent_url: "https://warpcast.com/~/channel/fixture", casters: 1200 };
    return route.fulfill({ json: { channel, pool: list(300, "Channel"), notes: {}, meta } });
  });
  return { release };
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

const top = (locator: Locator) => locator.evaluate((el) => Math.round(el.getBoundingClientRect().top + scrollY));

const sizes = [
  { name: "1280", phone: false, use: { viewport: { width: 1280, height: 900 } } },
  { name: "390", phone: true, use: { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true } },
];

for (const size of sizes) {
  test.describe(`at ${size.name}`, () => {
    test.use(size.use);
    const press = (target: Locator) => (size.phone ? target.tap() : target.click());

    test("switching between hot and new doesn't move the list: new shows its casts' apps as hot does", async ({ page }) => {
      await serve(page);
      await page.goto("/");
      await appCame(page);
      const first = page.locator(".cast-list .cast-card[data-hash]").first();
      await expect(first).toContainText("Hot 0");
      await expect(page.getByRole("region", { name: "Apps posting today's trending casts", exact: true })).toBeVisible();
      await settled(page);
      const where = await top(first);

      await press(page.locator(".sort-tabs a", { hasText: /^new$/ }));
      await expect(first).toContainText("New 0");
      const shares = page.getByRole("region", { name: "Apps posting the newest casts", exact: true });
      await expect(shares).toBeVisible();
      await expect(shares.locator(".client-legend li")).toHaveText([/^Farcaster\s*67%$/, /^wake\s*33%$/]);
      // Its apps filter new, as hot's do.
      await expect(shares.getByRole("link", { name: /^wake/ })).toHaveAttribute("href", "/?sort=new&client=2904");
      await settled(page);
      expect(await top(first)).toBe(where);

      await press(page.locator(".sort-tabs a", { hasText: /^hot$/ }));
      await expect(first).toContainText("Hot 0");
      await settled(page);
      expect(await top(first)).toBe(where);
    });

    test("a channel's page has its sidebar's place while it loads, so the feed doesn't move over when it comes", async ({ page }) => {
      const { release } = await serve(page);
      await page.goto("/c/fixture");
      /** The main column's box, and whether the page is still loading and has a sidebar, at one moment. */
      const columns = () =>
        page.evaluate(() => {
          const box = document.querySelector(".columns > .content")!.getBoundingClientRect();
          return {
            loading: document.querySelector(".channel-hero[aria-busy=true]") !== null,
            side: document.querySelectorAll(".columns > .side .side-box").length,
            x: Math.round(box.x),
            width: Math.round(box.width),
          };
        });
      // The channel is on its way.
      await expect(page.locator(".channel-hero[aria-busy=true]")).toBeVisible({ timeout: 15_000 });
      const loading = await columns();
      expect(loading.loading).toBe(true);
      expect(loading.side).toBe(1);
      release();
      await expect(page.locator(".cast-list .cast-card[data-hash]").first()).toBeVisible({ timeout: 10_000 });
      await expect(page.locator(".columns > .side")).toContainText("Feed: https://warpcast.com/~/channel/fixture");
      const loaded = await columns();
      expect(loaded.loading).toBe(false);
      expect(Math.abs(loaded.x - loading.x)).toBeLessThanOrEqual(1);
      expect(Math.abs(loaded.width - loading.width)).toBeLessThanOrEqual(1);
    });
  });
}
