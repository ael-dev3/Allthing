// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

/*
 * Show mini app casts: payloads carry mini app casts, marked, and the app
 * leaves them out unless the reader turns the switch on (over a feed, or in
 * the account menu). Then each shows with a link to open it in Farcaster.
 * The choice survives a reload; spam never shows.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const user = (fid: number, username: string) => ({ fid, username, display_name: username });
const hash = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const cast = (n: number, text: string, extra: object = {}) => ({
  hash: hash(n), author: user(100 + n, `caster${n}`), text, ts: 1792235681 - n * 60, likes: 10 - n, recasts: 0, replies: 0, ...extra,
});
const app = cast(2, "Play my game", { mini_app: true, embeds: [{ kind: "link", url: "https://game.example/" }] });
const pool = [cast(1, "An ordinary cast"), app, cast(3, "Another ordinary cast")];
const channel = { id: "fixture", name: "fixture", parent_url: "https://warpcast.com/~/channel/fixture" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : pool, notes: {}, meta } });
  });
  await page.route("**/api/v1/channel?*", (route) => route.fulfill({ json: { channel, pool, meta } }));
  const thread = { cast: cast(1, "A post"), children: [{ cast: app, children: [] }, { cast: cast(3, "A plain reply"), children: [] }] };
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: { thread, quotes: [], notes: {}, meta } }));
}

const sizes = [
  { name: "1280", use: { viewport: { width: 1280, height: 900 } } },
  { name: "390", use: { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true } },
];

for (const size of sizes) {
  test.describe(size.name, () => {
    test.use(size.use);

    test("feeds hide mini app casts by default, show them on the switch, and keep the choice", async ({ page }, info) => {
      await serve(page);
      await page.goto("/");
      const cards = page.locator(".cast-card");
      await expect(cards.filter({ hasText: "An ordinary cast" })).toBeVisible();
      await expect(cards.filter({ hasText: "Play my game" })).toHaveCount(0);
      const note = page.locator(".mini-apps-note");
      await expect(note).toContainText("1 mini app cast hidden");
      const toggle = note.getByRole("switch", { name: "Show mini app casts" });
      await expect(toggle).toHaveAttribute("aria-checked", "false");
      if (size.name === "390") expect((await toggle.boundingBox())!.height).toBeGreaterThanOrEqual(44);

      // Keyboard: focus and Space.
      await toggle.focus();
      await page.keyboard.press("Space");
      await expect(toggle).toHaveAttribute("aria-checked", "true");
      const shown = cards.filter({ hasText: "Play my game" });
      await expect(shown).toBeVisible();
      await expect(note).toContainText("1 mini app cast shown");
      const open = shown.getByRole("link", { name: /Open in Farcaster/ });
      await expect(open).toHaveAttribute("href", `https://farcaster.xyz/caster2/${hash(2).slice(0, 10)}`);
      await expect(open).toHaveAttribute("target", "_blank");
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(size.use.viewport.width);
      await page.screenshot({ path: info.outputPath(`mini-apps-${size.name}.png`) });

      // Kept across a reload, and on other feeds.
      await page.reload();
      await expect(cards.filter({ hasText: "Play my game" })).toBeVisible();
      await page.goto("/c/fixture");
      await expect(cards.filter({ hasText: "Play my game" })).toBeVisible();

      // Off again: gone, and still gone after a reload.
      await page.locator(".mini-apps-note").getByRole("switch").click();
      await expect(cards.filter({ hasText: "Play my game" })).toHaveCount(0);
      await page.reload();
      await expect(cards.filter({ hasText: "An ordinary cast" })).toBeVisible();
      await expect(cards.filter({ hasText: "Play my game" })).toHaveCount(0);
    });

    test("a thread leaves out mini app replies unless they are shown", async ({ page }) => {
      await serve(page);
      await page.goto(`/p/${hash(1)}`);
      await expect(page.getByText("A plain reply")).toBeVisible();
      await expect(page.getByText("Play my game")).toHaveCount(0);
      await page.evaluate(() => localStorage.setItem("allthing.show-mini-apps", "1"));
      await page.reload();
      await expect(page.getByText("Play my game")).toBeVisible();
      await expect(page.locator(".comment").filter({ hasText: "Play my game" }).getByRole("link", { name: /Open in Farcaster/ })).toBeVisible();
    });
  });
}

test("signed in, the account menu has the switch", async ({ page }) => {
  await serve(page);
  await page.addInitScript((user) => {
    document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax`;
  }, me);
  await page.goto("/");
  // The account menu opens on hover, or on a tap on a phone.
  const avatar = page.getByRole("link", { name: "Signed in as @reader: your profile" });
  await (test.info().project.name === "phone" ? avatar.tap() : avatar.hover());
  const menu = page.getByRole("dialog", { name: "Your account" });
  const toggle = menu.getByRole("switch", { name: "Show mini app casts" });
  await expect(toggle).toHaveAttribute("aria-checked", "false");
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-checked", "true");
  await expect(page.locator(".cast-card").filter({ hasText: "Play my game" })).toBeVisible();
  expect(await page.evaluate(() => localStorage.getItem("allthing.show-mini-apps"))).toBe("1");
});
