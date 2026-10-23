// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { NOW } from "./fixture-time";

/*
 * Show mini app casts: payloads carry mini app casts, marked, and the app
 * leaves them out unless the reader turns the switch on in the header's
 * Settings panel. No mini app count appears over the feed or in Settings.
 * Shown, a mini app cast (or a quoted one) carries a link to open it in Farcaster; no other cast carries any mini app chrome. The choice
 * survives a reload; spam never shows.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const user = (fid: number, username: string) => ({ fid, username, display_name: username });
const hash = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const cast = (n: number, text: string, extra: object = {}) => ({
  hash: hash(n), author: user(100 + n, `caster${n}`), text, ts: (NOW - 19) - n * 60, likes: 10 - n, recasts: 0, replies: 0, ...extra,
});
const app = cast(2, "Play my game", { mini_app: true, embeds: [{ kind: "link", url: "https://game.example/" }] });
const quotedApp = cast(9, "Quoted game", { mini_app: true, embeds: [{ kind: "link", url: "https://quoted.example/" }] });
const plainQuoted = cast(8, "A plain quoted cast");
const quotingApp = cast(4, "Look at this game", { embeds: [{ kind: "cast", hash: quotedApp.hash, fid: 109 }] });
const quotingPlain = cast(5, "Look at this cast", { embeds: [{ kind: "cast", hash: plainQuoted.hash, fid: 108 }] });
const pool = [cast(1, "An ordinary cast"), app, cast(3, "Another ordinary cast"), quotingApp, quotingPlain];
const notes = { quotes: { [quotedApp.hash]: quotedApp, [plainQuoted.hash]: plainQuoted } };
const channel = { id: "fixture", name: "fixture", parent_url: "https://warpcast.com/~/channel/fixture" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : pool, notes: later ? {} : notes, meta } });
  });
  await page.route("**/api/v1/channel?*", (route) => route.fulfill({ json: { channel, pool, notes, meta } }));
  const thread = { cast: cast(1, "A post"), children: [{ cast: app, children: [] }, { cast: cast(3, "A plain reply"), children: [] }] };
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: { thread, quotes: [], notes: {}, meta } }));
}

/** Open the header's Settings panel; give it and its switch. */
async function settings(page: Page) {
  await page.getByRole("button", { name: "Settings" }).click();
  const panel = page.getByRole("dialog", { name: "Settings" });
  await expect(panel).toBeVisible();
  return { panel, toggle: panel.getByRole("switch", { name: "Show mini app casts" }) };
}

const sizes = [
  { name: "1280", use: { viewport: { width: 1280, height: 900 } } },
  { name: "390", use: { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true } },
];

for (const size of sizes) {
  test.describe(size.name, () => {
    test.use(size.use);

    test("the switch lives in Settings, not the feed, and keeps the choice", async ({ page }, info) => {
      await serve(page);
      await page.goto("/");
      const cards = page.locator(".cast-card");
      await expect(cards.filter({ hasText: "An ordinary cast" })).toBeVisible();
      await expect(cards.filter({ hasText: "Play my game" })).toHaveCount(0);
      // Nothing about mini apps in the feed itself.
      await expect(page.locator("main .mini-apps-note, main .mini-apps-switch")).toHaveCount(0);

      const quote = cards.filter({ hasText: "Look at this game" }).locator("blockquote.quote");
      await expect(quote).toContainText("Quoted cast hidden: mini app");
      const { toggle } = await settings(page);
      await expect(page.locator(".mini-apps-note")).toHaveCount(0);
      await expect(toggle).toHaveAttribute("aria-checked", "false");
      await toggle.click();
      await expect(toggle).toHaveAttribute("aria-checked", "true");
      await expect(page.locator(".mini-apps-note")).toHaveCount(0);
      const shown = cards.filter({ hasText: "Play my game" });
      await expect(shown).toBeVisible();
      await expect(quote).toContainText("Quoted game");
      await expect(quote.locator(".mini-app-link")).toHaveCount(1);
      const open = shown.getByRole("link", { name: /Open in Farcaster/ });
      await expect(open).toHaveAttribute("href", `https://farcaster.xyz/caster2/${hash(2).slice(0, 10)}`);
      await expect(open).toHaveAttribute("target", "_blank");
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(size.use.viewport.width);
      await page.screenshot({ path: info.outputPath(`mini-apps-${size.name}.png`) });
      expect(await page.evaluate(() => localStorage.getItem("allthing.show-mini-apps"))).toBe("1");

      // Kept across a reload, and on other feeds.
      await page.reload();
      await expect(cards.filter({ hasText: "Play my game" })).toBeVisible();
      await page.goto("/c/fixture");
      await expect(cards.filter({ hasText: "Play my game" })).toBeVisible();

      // Off again: gone, and still gone after a reload.
      await (await settings(page)).toggle.click();
      await expect(cards.filter({ hasText: "Play my game" })).toHaveCount(0);
      await expect(quote).toContainText("Quoted cast hidden: mini app");
      await expect(quote.locator(".mini-app-link")).toHaveCount(0);
      await page.reload();
      await expect(cards.filter({ hasText: "An ordinary cast" })).toBeVisible();
      await expect(cards.filter({ hasText: "Play my game" })).toHaveCount(0);
    });

    test("mini app chrome only on casts and quotes that have a mini app", async ({ page }) => {
      await page.addInitScript(() => localStorage.setItem("allthing.show-mini-apps", "1"));
      await serve(page);
      await page.goto("/");
      const cards = page.locator(".cast-card");
      const plain = cards.filter({ hasText: "An ordinary cast" });
      await expect(plain).toBeVisible();
      await expect(plain.locator(".mini-app-link, .mini-app-badge")).toHaveCount(0);
      await expect(cards.filter({ hasText: "Play my game" }).locator(".mini-app-link")).toHaveCount(1);

      // A quote of a mini app cast carries the link, inside the quote card.
      const withApp = cards.filter({ hasText: "Look at this game" });
      const quote = withApp.locator("blockquote.quote");
      await expect(quote).toContainText("Quoted game");
      const link = quote.locator(".mini-app-link").getByRole("link", { name: /Open in Farcaster/ });
      await expect(link).toHaveAttribute("href", `https://farcaster.xyz/caster9/${hash(9).slice(0, 10)}`);
      await expect(withApp.locator(".mini-app-link")).toHaveCount(1);

      // A quote of a plain cast carries none.
      const withPlain = cards.filter({ hasText: "Look at this cast" });
      await expect(withPlain.locator("blockquote.quote")).toContainText("A plain quoted cast");
      await expect(withPlain.locator(".mini-app-link, .mini-app-badge")).toHaveCount(0);
    });

    test("a direct mini app body follows Settings both ways without a reload", async ({ page }) => {
      await serve(page);
      let reads = 0;
      const direct = { ...app, embeds: [{ kind: "link", url: "https://game.example/", title: "Game details" }] };
      await page.route("**/api/v1/cast?*", (route) => {
        reads++;
        const selected = new URL(route.request().url()).searchParams.get("hash") === app.hash ? direct : cast(1, "An ordinary post");
        return route.fulfill({ json: { thread: { cast: selected, children: [] }, quotes: [{ hash: plainQuoted.hash, cast: plainQuoted }], notes: {}, meta } });
      });
      await page.goto(`/p/${app.hash}`);
      const post = page.locator("#app .post");
      await expect(post.locator(".mini-app-note")).toBeVisible();
      await expect(post.locator(".post-text, .link-card")).toHaveCount(0);
      // Keep the real mini app's escape link even while its body is hidden.
      await expect(post.locator(".mini-app-link")).toHaveCount(1);
      for (const on of [true, false, true]) {
        const { toggle } = await settings(page);
        await toggle.click();
        await expect(toggle).toHaveAttribute("aria-checked", String(on));
        await page.keyboard.press("Escape");
        if (on) {
          await expect(post.locator(".post-text")).toHaveText("Play my game", { timeout: 2000 });
          await expect(post.locator(".link-card")).toContainText("Game details");
          await expect(post.locator("blockquote.quote")).toContainText("A plain quoted cast");
          await expect(post.locator(".mini-app-note")).toHaveCount(0);
        } else {
          await expect(post.locator(".post-text, .link-card, blockquote.quote")).toHaveCount(0);
          await expect(post.locator(".mini-app-note")).toBeVisible();
        }
      }
      expect(reads, "changing Settings uses the carried post, without refetching it").toBe(1);
      await page.goto(`/p/${hash(1)}`);
      const ordinaryBody = post.locator(".post-text");
      const ordinaryQuoteBody = post.locator("blockquote.quote .quote-text");
      await expect(ordinaryBody).toHaveText("An ordinary post");
      await expect(ordinaryQuoteBody).toHaveText("A plain quoted cast");
      await ordinaryBody.evaluate((element) => (window as any).__ordinaryBody = element);
      await ordinaryQuoteBody.evaluate((element) => (window as any).__ordinaryQuoteBody = element);
      for (const _ of [false, true]) {
        await (await settings(page)).toggle.click();
        await page.keyboard.press("Escape");
        await expect(ordinaryBody).toHaveText("An ordinary post");
        expect(await ordinaryBody.evaluate((element) => element === (window as any).__ordinaryBody)).toBe(true);
        await expect(ordinaryQuoteBody).toHaveText("A plain quoted cast");
        expect(await ordinaryQuoteBody.evaluate((element) => element === (window as any).__ordinaryQuoteBody)).toBe(true);
        await expect(post.locator(".mini-app-note, .mini-app-link")).toHaveCount(0);
      }
      expect(reads).toBe(2);
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
      await expect(page.locator(".comment").filter({ hasText: "A plain reply" }).locator(".mini-app-link")).toHaveCount(0);
    });
  });
}

test("signed in, the account menu leaves the switch to Settings", async ({ page }) => {
  await serve(page);
  await page.addInitScript((user) => {
    document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax`;
  }, me);
  await page.goto("/");
  const avatar = page.getByRole("link", { name: "Signed in as @reader: your profile" });
  await (test.info().project.name === "phone" ? avatar.tap() : avatar.hover());
  const menu = page.getByRole("dialog", { name: "Your account" });
  await expect(menu).toBeVisible();
  await expect(menu.getByRole("switch", { name: "Show mini app casts" })).toHaveCount(0);
  await page.mouse.move(0, 0);
  await page.keyboard.press("Escape");
  const { toggle } = await settings(page);
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-checked", "true");
  await expect(page.locator(".cast-card").filter({ hasText: "Play my game" })).toBeVisible();
});
