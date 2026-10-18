// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const hash = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aP1cAAAAASUVORK5CYII=", "base64");

// A long unbroken token must wrap inside the card on a phone.
const quotedWords = "Words from the quoted cast, signed 0x" + "f".repeat(64);
const quoted = {
  hash: hash(0xa1), text: quotedWords, ts: 1792230000,
  author: { fid: 2, username: "quoted-author", display_name: "Quoted Author", pfp_url: "https://media.example/pfp.png" },
  embeds: [{ kind: "image", url: "https://media.example/quoted.png" }],
};
const spam = { hash: hash(0xa3), author: { fid: 4, username: "spammer" }, text: "", ts: 1792230000, spam: true };
const missing = hash(0xa2);
const quoting = { hash: hash(1), author: { fid: 1, username: "fixture" }, text: "Outer cast that quotes another",
  ts: 1792235681, likes: 30, embeds: [{ kind: "cast", hash: quoted.hash, fid: 2 }] };
const unloaded = { hash: hash(2), author: { fid: 1, username: "fixture" }, text: "Outer cast whose quote is not in the payload",
  ts: 1792235600, likes: 20, embeds: [{ kind: "cast", hash: missing, fid: 3 }] };
const quotingSpam = { hash: hash(3), author: { fid: 1, username: "fixture" }, text: "Outer cast that quotes spam",
  ts: 1792235500, likes: 10, embeds: [{ kind: "cast", hash: spam.hash, fid: 4 }] };
const posts: Record<string, object> = {
  [quoted.hash]: { thread: { cast: quoted }, meta },
  [quoting.hash]: { thread: { cast: quoting }, quotes: [{ hash: quoted.hash, cast: quoted }], meta },
};

test.beforeEach(async ({ page }) => {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home", (route) => route.fulfill({ json: {
    pool: [quoting, unloaded, quotingSpam], notes: { quotes: { [quoted.hash]: quoted, [spam.hash]: spam } }, meta,
  } }));
  await page.route("**/api/v1/cast?*", (route) => {
    const post = posts[new URL(route.request().url()).searchParams.get("hash") ?? ""];
    return post ? route.fulfill({ json: post }) : route.fulfill({ status: 404, json: { error: "No such cast" } });
  });
  await page.route("https://media.example/**", (route) => route.fulfill({ contentType: "image/png", body: png }));
});

const layouts = [
  { name: "desktop", use: { viewport: { width: 1280, height: 900 } } },
  { name: "phone", use: { viewport: { width: 390, height: 844 }, hasTouch: true } },
];

for (const layout of layouts) {
  test.describe(layout.name, () => {
    test.use(layout.use);

    test("a quoted cast shows in the feed as a card, and a missing one as a link", async ({ page }) => {
      await page.goto("/");
      const card = page.locator(`.cast-card[data-hash="${quoting.hash}"]`);
      const quote = card.locator(".quote");
      await expect(quote).toBeVisible();
      await expect(quote.locator(".user-chip")).toContainText("Quoted Author");
      await expect(quote.locator(".user-chip")).toContainText("@quoted-author");
      await expect(quote.locator(".user-chip .avatar")).toHaveAttribute("src", quoted.author.pfp_url);
      await expect(quote.locator(".quote-text")).toHaveText(quotedWords);
      await expect(quote.locator(".tile-image")).toHaveAttribute("href", `/p/${quoted.hash}`);
      await expect(quote.locator("a.when")).toHaveAttribute("href", `/p/${quoted.hash}`);
      await expect(card.locator(".quote-chip")).toHaveCount(0);
      await expect(card).not.toContainText("quotes a cast");

      const fallback = page.locator(`.cast-card[data-hash="${unloaded.hash}"]`);
      await expect(fallback.locator(".quote")).toHaveCount(0);
      await expect(fallback.locator(".quote-chip")).toHaveText("↳ quotes a cast");
      await expect(fallback.locator(".quote-chip")).toHaveAttribute("href", `/p/${missing}`);

      const hidden = page.locator(`.cast-card[data-hash="${quotingSpam.hash}"] .quote`);
      await expect(hidden.locator(".quote-hidden")).toHaveText("Quoted cast hidden: spam");
      await expect(hidden.locator("a.when")).toHaveAttribute("href", `/p/${spam.hash}`);

      // Nothing sideways: the card and its long token fit the screen.
      const width = page.viewportSize()!.width;
      const box = (await quote.boundingBox())!;
      expect(box.x + box.width).toBeLessThanOrEqual(width);
      expect(await quote.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)).toBeLessThanOrEqual(0);
    });

    test("a click on the quote opens the quoted cast, and on the card the quoting post", async ({ page }) => {
      const errors: string[] = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await page.goto("/");
      const card = page.locator(`.cast-card[data-hash="${quoting.hash}"]`);
      await card.locator(".quote .quote-text").click();
      await expect(page).toHaveURL(new RegExp(`/p/${quoted.hash}$`));
      await expect(page.locator(".post .post-text")).toContainText("Words from the quoted cast");

      await page.goBack();
      await expect(page).toHaveURL(/\/$/);
      await card.locator(".cast-text").click();
      await expect(page).toHaveURL(new RegExp(`/p/${quoting.hash}$`));
      await expect(page.locator(".post .post-text")).toContainText("Outer cast that quotes another");

      // On the post page the quote is the same card, and opens the same way.
      // Wait for the thread, which renders the page again.
      await expect(page.locator(".replies .empty")).toHaveText("No replies yet.");
      await page.locator(".post .quote .quote-text").click();
      await expect(page).toHaveURL(new RegExp(`/p/${quoted.hash}$`));
      expect(errors).toEqual([]);
    });
  });
}
