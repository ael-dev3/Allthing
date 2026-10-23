// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Locator, type Page, type TestInfo } from "@playwright/test";
import { appCame } from "./helpers/app";
import { approvedKey, signIn } from "./helpers/farcaster";
import { standIn } from "./helpers/node";
import { NOW } from "./fixture-time";

/*
 * A reply's page shows the casts it answers as full cards, root first, joined
 * to the reply by a line from each avatar to the next. A chain of more than
 * three folds its middle behind "Show N earlier", which opens in place and
 * drops nothing. A hidden ancestor is left out, and the chain's casts keep
 * their media, counts and hover cards.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = (NOW - 19);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const people = {
  alice: { fid: 42, username: "alice", display_name: "Alice" },
  bob: { fid: 43, username: "bob", display_name: "Bob" },
  carol: { fid: 44, username: "carol", display_name: "Carol" },
  me: { fid: 1234, username: "reader", display_name: "Reader" },
};
const cast = (n: number, author: keyof typeof people, text: string, parent?: number, extra: object = {}) => ({
  hash: hex(n),
  author: people[author],
  text,
  mentions: [],
  embeds: [],
  ts: ts - (100 - n) * 60,
  likes: n,
  recasts: 0,
  replies: 0,
  parent_hash: parent === undefined ? undefined : hex(parent),
  ...extra,
});

// A reply with one cast above it.
const root = cast(10, "alice", "Who is reading Hyperion this month?");
const oneReply = cast(11, "bob", "Me, halfway through.", 10);
// A reply under six casts, root first.
const longChain = [20, 21, 22, 23, 24, 25].map((n, i) => cast(n, i % 2 ? "bob" : "alice", `Chain cast ${i}`, i === 0 ? undefined : n - 1));
const deepReply = cast(26, "carol", "The last one in the chain.", 25);
// A reply under a cast with a picture, and a cast with a picture above it.
const pictured = cast(30, "carol", "A picture above this.", undefined, {
  embeds: [{ kind: "image", url: "https://media.example/chart.svg" }],
});
const pictureReply = cast(31, "bob", "Nice chart.", 30);

const chains: Record<string, { cast: object; ancestors: object[] }> = {
  [hex(11)]: { cast: oneReply, ancestors: [root] },
  [hex(26)]: { cast: deepReply, ancestors: longChain },
  [hex(31)]: { cast: pictureReply, ancestors: [pictured] },
};

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("https://media.example/**", (route) =>
    route.fulfill({ body: `<svg xmlns="http://www.w3.org/2000/svg" width="640" height="360"><rect width="640" height="360" fill="#4f43d4"/></svg>`, contentType: "image/svg+xml" }),
  );
  await page.route("**/api/v1/user?*", (route) =>
    route.fulfill({ json: { user: { ...people.carol, follower_count: 1200 }, pool: [], meta } }),
  );
  await page.route("**/api/v1/cast?*", (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash") ?? "";
    const found = chains[hash];
    return route.fulfill({
      json: found
        ? { thread: { cast: found.cast, children: [] }, ancestors: found.ancestors, earlier: false, notes: {}, meta }
        : { thread: { cast: root, children: [{ cast: oneReply, children: [] }] }, ancestors: [], earlier: false, notes: {}, meta },
    });
  });
}

/** The centre of an element's box, in page pixels. */
async function centre(locator: Locator) {
  const box = await locator.boundingBox();
  if (!box) throw new Error("not in the page");
  return { x: box.x + box.width / 2, y: box.y + box.height / 2, top: box.y, bottom: box.y + box.height };
}

const widths = [
  { name: "1280", use: { viewport: { width: 1280, height: 800 } } },
  { name: "390", use: { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true } },
] as const;

for (const width of widths) {
  test.describe(`at ${width.name}`, () => {
    test.use(width.use);

    test("a reply with one cast above it: the cast as a card, joined to the reply by a line under both avatars", async ({ page }, testInfo: TestInfo) => {
      await serve(page);
      await page.goto(`/p/${oneReply.hash}`);
      await appCame(page);
      const ancestor = page.locator(".thread-chain > .chain-item .cast-card");
      await expect(ancestor).toHaveCount(1);
      await expect(ancestor.locator(".cast-text")).toHaveText("Who is reading Hyperion this month?");
      await expect(ancestor.locator(".cast-foot")).toBeVisible();
      await expect(page.locator(".parent-context")).toHaveCount(0);
      await expect(page.locator(".thread-chain .chain-more")).toHaveCount(0);

      // The line starts where the card ends and stops where the reply starts,
      // on the centre of both avatars.
      // Cards rise into place when they come: measure them once they've landed.
      await expect
        .poll(() => page.evaluate(() => [...document.querySelectorAll(".thread-chain .cast-card, article.post")].every((el) => getComputedStyle(el).transform === "none")))
        .toBe(true);
      const link = await centre(page.locator(".thread-chain > .chain-link").first());
      const cardAvatar = await centre(ancestor.locator(".user-chip .avatar"));
      const replyAvatar = await centre(page.locator("article.post .post-author .avatar"));
      expect(Math.abs(link.x - cardAvatar.x)).toBeLessThanOrEqual(1);
      expect(Math.abs(link.x - replyAvatar.x)).toBeLessThanOrEqual(1);
      expect(Math.abs(link.top - (await centre(ancestor)).bottom)).toBeLessThanOrEqual(1);
      expect(Math.abs(link.bottom - (await centre(page.locator("article.post"))).top)).toBeLessThanOrEqual(1);
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width.name === "390" ? 390 : 1280);
      await page.screenshot({ path: testInfo.outputPath(`chain-1-${width.name}.png`), fullPage: true });
    });

    test("a chain of six: the root and the last two show, and Show 3 earlier opens the rest in place", async ({ page }, testInfo: TestInfo) => {
      await serve(page);
      await page.goto(`/p/${deepReply.hash}`);
      await appCame(page);
      const cards = page.locator(".thread-chain > .chain-item .cast-text");
      await expect(cards).toHaveCount(3);
      await expect(cards.nth(0)).toHaveText("Chain cast 0");
      await expect(cards.nth(2)).toHaveText("Chain cast 5");
      const more = page.getByRole("button", { name: "Show 3 earlier" });
      await expect(more).toBeVisible();
      await page.screenshot({ path: testInfo.outputPath(`chain-6-folded-${width.name}.png`), fullPage: true });
      await more.click();
      await expect(more).toHaveCount(0);
      await expect(cards).toHaveCount(6);
      await expect(cards.nth(1)).toHaveText("Chain cast 1");
      await expect(cards.nth(3)).toHaveText("Chain cast 3");
      // Every cast is in the chain, and the reply below it.
      for (let i = 0; i < 6; i++) await expect(cards.nth(i)).toHaveText(`Chain cast ${i}`);
      await expect(page.locator("article.post .post-text")).toContainText("The last one in the chain.");
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width.name === "390" ? 390 : 1280);
      await page.screenshot({ path: testInfo.outputPath(`chain-6-open-${width.name}.png`), fullPage: true });
    });

    test("a chain cast keeps its picture", async ({ page }, testInfo: TestInfo) => {
      await serve(page);
      await page.goto(`/p/${pictureReply.hash}`);
      await appCame(page);
      // The picture is drawn in its tile, in the card above the reply.
      const tile = page.locator(".thread-chain .cast-card .tile-image").first();
      await expect(tile).toBeVisible();
      expect((await tile.boundingBox())?.width ?? 0).toBeGreaterThan(100);
      await page.screenshot({ path: testInfo.outputPath(`chain-media-${width.name}.png`), fullPage: true });
    });

    test("the reply's Reply button still opens its composer, under a chain", async ({ page }, testInfo: TestInfo) => {
      await signIn(page, people.me);
      const node = await standIn(page, { fid: people.me.fid, key: () => "" });
      await serve(page);
      await page.goto(`/p/${oneReply.hash}`);
      await expect(page.locator(".thread-chain .cast-card").first()).toBeVisible();
      await approvedKey(page, people.me.fid, 3_600_000);
      await page.reload();
      await appCame(page);
      await page.getByRole("button", { name: "Reply to @bob" }).first().click();
      const replying = page.getByRole("dialog", { name: "Reply" });
      await expect(replying).toBeVisible();
      await replying.getByRole("textbox").fill("Still halfway.");
      await expect(replying.getByRole("textbox")).toContainText("Still halfway.");
      await page.screenshot({ path: testInfo.outputPath(`chain-composer-${width.name}.png`) });
      void node;
    });
  });
}

test.describe("with a mouse", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("an ancestor's author shows a hover card", async ({ page }) => {
    await serve(page);
    await page.goto(`/p/${pictureReply.hash}`);
    await appCame(page);
    await page.locator(".thread-chain > .chain-item .cast-card .user-chip > .display-name").first().hover();
    await expect(page.getByRole("dialog", { name: "Profile: Carol (@carol)", exact: true })).toBeVisible();
  });
});
