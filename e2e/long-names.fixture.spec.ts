// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";

/*
 * Long names and long words never break a card. In a feed card, a quoted
 * cast and a reply, who wrote it and when stay on one line: the picture
 * keeps its size, the name and the handle are cut short with an ellipsis,
 * and the time keeps its place, with the dot before it (the channel and the
 * app follow, on the next line if they must). A profile card shows the name
 * and the handle a line each, cut short when long; a profile's bio breaks a
 * word or a link longer than its line, and "fid 99" keeps together.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = Math.floor(Date.now() / 1000) - 3 * 3600;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64");
const pfp = (name: string) => `https://imagedelivery.net/fixture/avatar-${name}/original`;
const alice = { fid: 42, username: "alice", display_name: "Alice", pfp_url: pfp("alice") };
// As long as real ones get: base.eth handles run long.
const long = {
  fid: 99,
  username: "averyveryverylongusername-for-testing.eth",
  display_name: "Bartholomew Montgomery-Fitzwilliam the Third",
  pfp_url: pfp("long"),
};
const cast = (n: number, author: object, text: string, extra: object = {}) =>
  ({ hash: hex(n), author, text, ts: ts - n * 60, likes: 3, recasts: 1, replies: 2, ...extra });
const longCard = cast(1, long, "A cast by an account with a long name and handle, in a channel, via an app.", { channel: { id: "farcaster", label: "farcaster" } });
const quoted = cast(2, long, "The quoted cast, by the account with the long name.");
const quoting = cast(3, alice, "Quoting someone with a long name", { embeds: [{ kind: "cast", hash: quoted.hash, fid: long.fid }] });
const short = cast(4, alice, "A cast by Alice, whose name is short.");
const pool = [longCard, quoting, short];
const app = { fid: 9152, username: "warpcast", display_name: "Warpcast" };
const origins = Object.fromEntries([...pool, quoted].map((c) => [c.hash, { signer: `0x${"ab".repeat(32)}`, app_fid: app.fid }]));
const notes = { origins, apps: { [app.fid]: app }, quotes: { [quoted.hash]: quoted }, users: { "99": { bio: "A long-named account.", followers: 12, following: 3 } } };
// A thread: a reply by the long name at the top, and one four replies deep.
const reply = (n: number, author: object, text: string, children: object[] = []) =>
  ({ cast: { ...cast(n, author, text), parent_hash: short.hash }, children });
const thread = {
  cast: short,
  children: [
    reply(10, long, "A reply from the long name."),
    reply(11, alice, "A reply.", [reply(12, alice, "Deeper.", [reply(13, alice, "Deeper still.", [reply(14, long, "Four deep, from the long name.")])])]),
  ],
};
const bio = "A bio with a link https://example.com/a/very/long/path/that/keeps/going/and/going/forever and a word: Pneumonoultramicroscopicsilicovolcanoconiosis-and-then-some-more-letters.";

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : pool, notes, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: { thread, notes, meta } }));
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: {
    user: { ...long, bio, follower_count: 12, following_count: 3, registered_at: 1650000000 }, pool: [longCard], notes, meta,
  } }));
  await page.route("**/api/v1/vote?*", (route) => route.fulfill({ json: { fid: 99, onchain: "stay", meta } }));
  await page.route("**/api/v1/wallets?*", (route) => route.fulfill({ json: { fid: 99, addresses: [], chains: [], total: 0, sources: [], fetched_at: Date.now() } }));
  await page.route("https://imagedelivery.net/**", (route) => route.fulfill({ contentType: "image/png", body: png }));
}

async function at(browser: Browser, testInfo: TestInfo, width: number) {
  const phone = width < 600;
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL, viewport: { width, height: phone ? 844 : 900 }, isMobile: phone, hasTouch: phone,
  });
  return { context, page: await context.newPage() };
}

/** How the author's line in `meta` (a card's, a quote's or a reply's) is laid out. */
async function byline(meta: Locator) {
  return meta.evaluate((meta) => {
    const lines = (el: Element) => {
      const range = document.createRange();
      range.selectNodeContents(el);
      return new Set([...range.getClientRects()].filter((r) => r.width > 0).map((r) => Math.round(r.top))).size;
    };
    const middle = (el: Element) => {
      const r = el.getBoundingClientRect();
      return r.top + r.height / 2;
    };
    const chip = meta.querySelector(".user-chip")!;
    // Laid out, not as an animation may scale it.
    const avatar = chip.querySelector(".avatar") as HTMLElement;
    const when = meta.querySelector(".when")!;
    const card = meta.parentElement!.getBoundingClientRect();
    const text = (el: Element | null) => {
      if (!el) return null;
      const style = getComputedStyle(el);
      return {
        lines: lines(el),
        cut: el.scrollWidth > el.clientWidth + 0.5,
        ellipsis: style.textOverflow === "ellipsis" && style.whiteSpace === "nowrap",
        right: el.getBoundingClientRect().right,
      };
    };
    const toggle = meta.querySelector(".toggle");
    return {
      avatar: [avatar.offsetWidth, avatar.offsetHeight],
      name: text(chip.querySelector(".display-name")),
      handle: text(chip.querySelector(".author"))!,
      // The time on the chip's line, and the dot before it, the time's own.
      level: Math.abs(middle(when) - middle(chip)),
      toggleLevel: toggle ? Math.abs(middle(toggle) - middle(chip)) : 0,
      whenLines: lines(when.querySelector("time") ?? when),
      dot: getComputedStyle(when, "::before").content,
      separators: meta.querySelectorAll(".sep").length,
      right: Math.max(...[...meta.querySelectorAll(".user-chip, .when")].map((e) => e.getBoundingClientRect().right)),
      edge: card.right,
      pageWidth: document.documentElement.scrollWidth,
    };
  });
}

type Line = Awaited<ReturnType<typeof byline>>;

function oneLine(line: Line, what: string, { cut, width }: { cut: boolean; width: number }) {
  expect(line.avatar, `${what}: the picture keeps its size`).toEqual([22, 22]);
  expect(line.handle.lines, `${what}: the handle on one line`).toBe(1);
  if (line.name) expect(line.name.lines, `${what}: the name on one line`).toBe(1);
  expect(line.level, `${what}: the time on the author's line`).toBeLessThanOrEqual(2);
  expect(line.toggleLevel, `${what}: the toggle on the author's line`).toBeLessThanOrEqual(2);
  expect(line.whenLines, `${what}: the time on one line`).toBe(1);
  expect(line.dot, `${what}: the dot is the time's`).toContain("·");
  expect(line.separators, `${what}: no dot of its own`).toBe(0);
  expect(line.right, `${what}: inside its card`).toBeLessThanOrEqual(line.edge);
  expect(line.pageWidth, `${what}: nothing wider than the screen`).toBeLessThanOrEqual(width);
  if (cut) {
    expect(line.handle.ellipsis, `${what}: the handle cut short with an ellipsis`).toBe(true);
    expect(line.handle.cut || Boolean(line.name?.cut), `${what}: cut short`).toBe(true);
  } else {
    expect(line.handle.cut || Boolean(line.name?.cut), `${what}: shown whole`).toBe(false);
  }
}

for (const width of [1280, 390]) {
  const phone = width < 600;

  test(`at ${width}, a long name keeps a feed card's and a quote's author and time on one line`, async ({ browser }, testInfo) => {
    const { context, page } = await at(browser, testInfo, width);
    await serve(page);
    await page.goto("/");
    const card = page.locator(`#app .cast-card[data-hash="${longCard.hash}"]`);
    await expect(card).toBeVisible();
    oneLine(await byline(card.locator(".cast-meta")), "a feed card", { cut: true, width });
    // The channel and the app follow it, inside the card.
    const rest = await card.locator(".cast-meta").evaluate((meta) => {
      const edge = meta.getBoundingClientRect().right;
      return [...meta.querySelectorAll(".in-channel, .origin")].map((e) => e.getBoundingClientRect().right <= edge + 0.5);
    });
    expect(rest).toEqual([true, true]);
    const quote = page.locator(`#app .cast-card[data-hash="${quoting.hash}"] .quote`);
    await quote.scrollIntoViewIfNeeded();
    oneLine(await byline(quote.locator(".cast-meta")), "a quote", { cut: true, width });
    // A short name is shown whole, on one line with its time, its app and all.
    const plain = page.locator(`#app .cast-card[data-hash="${short.hash}"] .cast-meta`);
    await plain.scrollIntoViewIfNeeded();
    oneLine(await byline(plain), "a short name", { cut: false, width });
    expect(await plain.evaluate((m) => m.getBoundingClientRect().height)).toBeLessThan(30);
    await card.screenshot({ path: testInfo.outputPath(`card-${width}.png`) });
    await context.close();
  });

  test(`at ${width}, a long name keeps a reply's toggle, author and time on one line`, async ({ browser }, testInfo) => {
    const { context, page } = await at(browser, testInfo, width);
    await serve(page);
    await page.goto(`/p/${short.hash}`);
    await expect(page.locator(".comment-head")).toHaveCount(5);
    // The long name's two replies, at the top and four deep.
    const heads = page.locator(".comment-head").filter({ has: page.locator(".author", { hasText: long.username }) });
    await expect(heads).toHaveCount(2);
    for (const [n, label] of [[0, "a reply"], [1, "a reply four deep"]] as const) {
      const head = heads.nth(n);
      await head.scrollIntoViewIfNeeded();
      oneLine(await byline(head), label, { cut: phone, width });
    }
    await context.close();
  });

  test(`at ${width}, a profile card shows a long name and handle on a line each`, async ({ browser }, testInfo) => {
    const { context, page } = await at(browser, testInfo, width);
    await serve(page);
    await page.goto("/");
    const handle = page.locator(`#app .cast-card[data-hash="${longCard.hash}"] .user-chip .author`);
    if (phone) await handle.tap();
    else await handle.hover();
    const card = page.locator(".popover-panel.profile-card");
    await expect(card).toContainText("A long-named account.");
    const head = await card.evaluate((card) => {
      const lines = (el: Element) => {
        const range = document.createRange();
        range.selectNodeContents(el);
        return new Set([...range.getClientRects()].filter((r) => r.width > 0).map((r) => Math.round(r.top))).size;
      };
      const name = card.querySelector(".profile-card-name")!;
      const handle = card.querySelector(".profile-card-handle")!;
      const picture = card.querySelector(".avatar") as HTMLElement;
      return {
        picture: [picture.offsetWidth, picture.offsetHeight],
        lines: [lines(name), lines(handle)],
        ellipsis: [name, handle].map((el) => getComputedStyle(el).textOverflow === "ellipsis"),
        inside: [name, handle].every((el) => el.getBoundingClientRect().right <= card.getBoundingClientRect().right),
      };
    });
    expect(head.picture).toEqual([44, 44]);
    expect(head.lines).toEqual([1, 1]);
    expect(head.ellipsis).toEqual([true, true]);
    expect(head.inside).toBe(true);
    await context.close();
  });

  test(`at ${width}, a profile's bio breaks a long word and a long link, and "fid 99" keeps together`, async ({ browser }, testInfo) => {
    const { context, page } = await at(browser, testInfo, width);
    await serve(page);
    await page.goto(`/u/${long.username}`);
    const profile = page.locator(".profile");
    await expect(profile.locator(".bio")).toContainText("Pneumonoultramicroscopicsilicovolcanoconiosis");
    const laid = await profile.evaluate((profile) => {
      const bio = profile.querySelector(".bio")!;
      const box = profile.getBoundingClientRect();
      const words = [...bio.querySelectorAll("*")].concat(bio).map((el) => el.getBoundingClientRect().right);
      return {
        overflow: bio.scrollWidth - bio.clientWidth,
        right: Math.max(...words),
        edge: box.right,
        page: document.documentElement.scrollWidth,
        fid: profile.querySelector(".handle .muted")!.textContent,
      };
    });
    expect(laid.overflow, "nothing of the bio runs past it").toBeLessThanOrEqual(0);
    expect(laid.right).toBeLessThanOrEqual(laid.edge);
    expect(laid.page).toBeLessThanOrEqual(width);
    // No line can break between "fid" and its number, or before them after the dot.
    expect(laid.fid).toBe(" · fid 99");
    await page.screenshot({ path: testInfo.outputPath(`profile-${width}.png`) });
    await context.close();
  });
}
