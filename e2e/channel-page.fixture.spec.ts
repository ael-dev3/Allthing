// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { NOW } from "./fixture-time";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aP1cAAAAASUVORK5CYII=", "base64");
const user = (fid: number, username: string, display_name: string) =>
  ({ fid, username, display_name, pfp_url: `https://media.example/pfp-${fid}.png` });
const cast = (n: number, text: string) => ({
  hash: `0x${n.toString(16).padStart(40, "0")}`, author: user(100 + n, `caster${n}`, `Caster ${n}`), text,
  ts: (NOW - 19) - n * 60, likes: n,
});
const pinned = cast(1, "Welcome to the fixture channel: read the rules first");
// The pinned cast is also among the channel's recent casts.
const pool = [pinned, cast(2, "A cast in the feed"), cast(3, "Another cast in the feed")];
const parent_url = "https://warpcast.com/~/channel/fixture";

// The channel as the Worker serves it when the Farcaster app's channel API answered.
const rich = {
  id: "fixture", name: "Fixture Channel", parent_url,
  description: "Where @alice tests channel pages. Rules: https://example.com/rules",
  description_mentions: [{ username: "alice", fid: 2 }],
  image_url: "https://media.example/picture.png",
  header_image_url: "https://media.example/banner.png",
  member_count: 475, follower_count: 183421, casters: 15664, created_at: 1689121073,
  lead: user(191, "lead-fixture", "Lead Person"),
  moderators: [user(2172, "mod-one", "Mod One"), user(834, "mod-two", "Mod Two")],
  pinned,
  external_link: { url: "https://example.com/join", title: "Join" },
};

// The same channel when only Hypersnap answered (as in the static copy):
// its `member_count` counted distinct casters, served as `casters`.
const plain = {
  id: "fixture", name: "fixture", parent_url, description: null, image_url: null, member_count: null,
  casters: 15664, created_at: null,
};

async function serve(page: Page, channel: object) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/channel?*", (route) => route.fulfill({ json: { channel, pool, meta } }));
  await page.route("https://media.example/**", (route) => route.fulfill({ contentType: "image/png", body: png }));
}

async function expectNoSidewaysScroll(page: Page) {
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - innerWidth);
  expect(overflow, "the page scrolls sideways").toBeLessThanOrEqual(0);
}

const sizes = [
  { name: "desktop", use: { viewport: { width: 1280, height: 900 } } },
  { name: "phone", use: { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true } },
];

for (const size of sizes) {
  test.describe(size.name, () => {
    test.use(size.use);

    test("a channel shows its banner, picture, description, counts, hosts and pinned cast", async ({ page }) => {
      await serve(page, rich);
      await page.goto("/c/fixture");
      const hero = page.locator(".channel-hero");
      await expect(page.getByRole("heading", { level: 1 })).toHaveText("Fixture Channel");
      await expect(hero.locator(".channel-handle")).toHaveText("/fixture");
      await expect(page).toHaveTitle("Fixture Channel (/fixture) · Allthing");

      // A decorative 3:1 banner, and the named picture over its bottom edge.
      const banner = hero.locator(".channel-banner");
      await expect(banner.locator("img")).toHaveAttribute("alt", "");
      await expect(banner.locator("img")).toHaveAttribute("src", "https://media.example/banner.png");
      const picture = hero.getByRole("img", { name: "Picture of Fixture Channel" });
      await expect(picture).toBeVisible();
      await expect.poll(() => picture.evaluate((img) => (img as HTMLImageElement).naturalWidth > 0)).toBe(true);
      const [b, p] = await Promise.all([banner.boundingBox(), picture.boundingBox()]);
      expect(Math.abs(b!.width / b!.height - 3)).toBeLessThan(0.05);
      expect(p!.y).toBeLessThan(b!.y + b!.height);
      expect(p!.y + p!.height).toBeGreaterThan(b!.y + b!.height);
      expect(p!.x).toBeGreaterThanOrEqual(b!.x);

      // The description, with its mention and link.
      const description = hero.locator(".channel-description");
      await expect(description).toHaveText("Where @alice tests channel pages. Rules: example.com/rules");
      await expect(description.getByRole("link", { name: "@alice" })).toHaveAttribute("href", "/u/alice");
      await expect(description.getByRole("link", { name: "example.com/rules" })).toHaveAttribute("href", "https://example.com/rules");

      // True counts from the Farcaster app; Hypersnap's casters only as such.
      const stats = hero.locator(".channel-stats");
      await expect(stats.locator(".count")).toHaveText(["183,421 followers", "475 members"]);
      await expect(page.getByText(/15,664 members/)).toHaveCount(0);
      await expect(page.locator(".side")).toContainText("15,664 accounts have cast here");
      const join = stats.getByRole("link", { name: "Join" });
      await expect(join).toHaveAttribute("href", "https://example.com/join");
      await expect(join).toHaveAttribute("rel", /noopener/);

      // The lead and moderators link to their profiles by name.
      const hosts = hero.locator(".channel-hosts");
      await expect(hosts.getByText("Lead", { exact: true })).toBeVisible();
      await expect(hosts.getByRole("link", { name: "Lead Person", exact: true })).toHaveAttribute("href", "/u/lead-fixture");
      await expect(hosts.getByText("Moderators", { exact: true })).toBeVisible();
      await expect(hosts.locator(".host-list .host")).toHaveText(["Mod One", "Mod Two"]);
      await expect(hosts.getByRole("link", { name: "Mod One", exact: true })).toHaveAttribute("href", "/u/mod-one");
      await expect(hosts.getByRole("link", { name: "Mod Two", exact: true })).toHaveAttribute("href", "/u/mod-two");
      await expect(hosts.getByRole("button")).toHaveCount(0);

      // The pinned cast comes first, as a card, and isn't repeated in the feed.
      const pinnedRegion = page.getByRole("region", { name: "Pinned" });
      await expect(pinnedRegion.locator(`.cast-card[data-hash="${pinned.hash}"]`)).toContainText(pinned.text);
      await expect(page.locator(`.cast-card[data-hash="${pinned.hash}"]`)).toHaveCount(1);
      await expect(page.locator(".cast-list .cast-card")).toHaveCount(pool.length - 1);
      const [pinnedBox, feedBox] = await Promise.all([pinnedRegion.boundingBox(), page.locator(".cast-list").boundingBox()]);
      expect(pinnedBox!.y + pinnedBox!.height).toBeLessThanOrEqual(feedBox!.y);

      // Headings run in order from the channel's name.
      const levels = await page.locator("main :is(h1, h2, h3, h4)").evaluateAll((hs) => hs.map((h) => Number(h.tagName[1])));
      expect(levels[0]).toBe(1);
      expect(levels.every((level, i) => i === 0 || level <= levels[i - 1] + 1)).toBe(true);
      await expectNoSidewaysScroll(page);
    });

    test("a channel without the app's details leaves those parts out and claims no members", async ({ page }) => {
      await serve(page, plain);
      await page.goto("/c/fixture");
      await expect(page.getByRole("heading", { level: 1 })).toHaveText("/fixture");
      await expect(page.locator(".cast-list .cast-card")).toHaveCount(pool.length);
      const hero = page.locator(".channel-hero");
      await expect(hero.locator(".channel-banner")).toHaveCount(0);
      await expect(hero.locator("img")).toHaveCount(0);
      await expect(hero.locator(".channel-description, .channel-handle, .channel-hosts")).toHaveCount(0);
      await expect(page.getByText("Lead", { exact: true })).toHaveCount(0);
      await expect(page.getByText("Moderators", { exact: true })).toHaveCount(0);
      await expect(page.getByText("Pinned", { exact: true })).toHaveCount(0);
      // Hypersnap's number counts accounts that have cast, and says so.
      await expect(hero.locator(".channel-stats")).toHaveText("15,664 casters");
      await expect(page.locator("main")).not.toContainText(/\bmembers?\b/);
      await expectNoSidewaysScroll(page);
    });

    test("long moderator lists show six and a button for the rest", async ({ page }) => {
      const moderators = Array.from({ length: 9 }, (_, i) => user(1000 + i, `mod-${i}`, `Moderator with a long name ${i}`));
      await serve(page, { ...rich, moderators, pinned: undefined, header_image_url: undefined });
      await page.goto("/c/fixture");
      const hosts = page.locator(".channel-hosts");
      await expect(hosts.locator(".host-list .host")).toHaveCount(6);
      await expectNoSidewaysScroll(page);
      const more = hosts.getByRole("button", { name: "+3 more moderators", exact: true });
      await expect(more).toBeVisible();
      await more.click();
      await expect(hosts.locator(".host-list .host")).toHaveCount(9);
      await expect(more).toHaveCount(0);
      // Without a banner, the picture sits beside the name.
      await expect(page.locator(".channel-banner")).toHaveCount(0);
      await expect(page.getByRole("img", { name: "Picture of Fixture Channel" })).toBeVisible();
      await expectNoSidewaysScroll(page);
    });
  });
}
