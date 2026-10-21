// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";

/*
 * On a touch screen, the meta line's links take a tap 25 or 26 px tall,
 * centred on them, where they are 15 to 22 px: who wrote it (the picture,
 * the name and the handle), when, the channel and the app, on a feed card,
 * a quoted cast, a reply and a post. None meets another target's tap area
 * or reaches the text under it, so a tap on the text's first line still
 * opens the cast; the layout is as it was on one line, and a line that
 * wraps keeps its lines apart. With a mouse, nothing changes.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = Math.floor(Date.now() / 1000) - 3 * 3600;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64");
const pfp = (name: string) => `https://imagedelivery.net/fixture/avatar-${name}/original`;
const alice = { fid: 42, username: "alice", display_name: "Alice", pfp_url: pfp("alice") };
const long = { fid: 99, username: "averyveryverylongusername-for-testing.eth", display_name: "Bartholomew Montgomery-Fitzwilliam", pfp_url: pfp("long") };
const channel = { id: "farcaster", label: "farcaster" };
const cast = (n: number, author: object, text: string, extra: object = {}) =>
  ({ hash: hex(n), author, text, ts: ts - n * 60, likes: 3, recasts: 1, replies: 2, ...extra });
const short = cast(1, alice, "A short name, in a channel, via an app.", { channel });
const wrapped = cast(2, long, "A long name: its channel and app go to a second line.", { channel });
const quoted = cast(3, alice, "The quoted cast.");
const quoting = cast(4, alice, "Quoting a cast.", { embeds: [{ kind: "cast", hash: quoted.hash, fid: alice.fid }] });
const imageOnly = cast(5, alice, "", { channel, embeds: [{ kind: "image", url: "https://imagedelivery.net/fixture/photo/original" }] });
// Who and when alone, with no channel and no app: a meta line well short of the card's width.
const plain = cast(6, alice, "A short name, and nothing else on its line.");
const pool = [short, wrapped, quoting, imageOnly, plain];
const app = { fid: 9152, username: "warpcast", display_name: "Warpcast" };
const origins = Object.fromEntries([short, wrapped, quoting, imageOnly, quoted].map((c) => [c.hash, { signer: `0x${"ab".repeat(32)}`, app_fid: app.fid }]));
const reply = (n: number, author: object, text: string) => ({ cast: { ...cast(n, author, text), parent_hash: short.hash }, children: [] });
const notes = { origins: { ...origins, [hex(10)]: origins[short.hash], [hex(11)]: origins[short.hash] }, apps: { [app.fid]: app }, quotes: { [quoted.hash]: quoted } };

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : pool, notes, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
    thread: { cast: short, children: [reply(10, alice, "A reply."), reply(11, long, "A reply from the long name.")] }, notes, meta,
  } }));
  await page.route("https://imagedelivery.net/**", (route) => route.fulfill({ contentType: "image/png", body: png }));
}

async function at(browser: Browser, testInfo: TestInfo, touch: boolean) {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL, viewport: { width: touch ? 390 : 1280, height: 900 }, isMobile: touch, hasTouch: touch,
  });
  return { context, page: await context.newPage() };
}

const LINKS = [
  ".user-chip > a", ".byline > a.when", ".in-channel > a", ".origin > a", ".post-meta a",
].join(", ");

/** Each meta-line link in `root`: its tap area's height, and whether taps at its middle and 9 px above and below land on it. */
async function taps(page: Page, root: string) {
  return page.locator(root).first().evaluate((root, links) => {
    return [...root.querySelectorAll(links)].filter((el) => el.getClientRects().length > 0).map((el) => {
      const box = el.getBoundingClientRect();
      const [x, y] = [box.left + Math.min(box.width / 2, 8), box.top + box.height / 2];
      const lands = (dy: number) => {
        const hit = document.elementFromPoint(x, y + dy);
        return hit === el || el.contains(hit);
      };
      return { link: `${el.className || el.parentElement!.className} "${(el.textContent ?? "").trim().slice(0, 16)}"`, height: box.height, lands: [lands(0), lands(-9), lands(9)] };
    });
  }, LINKS);
}

/** Pairs of targets in `root`, one of them a meta-line link, whose tap areas meet: each one's box, and its ::after band if it has one. */
async function meeting(page: Page, root: string) {
  return page.locator(root).first().evaluate((root, links) => {
    type Box = { x: number; y: number; right: number; bottom: number };
    const areas = (el: Element): Box[] => {
      const r = el.getBoundingClientRect();
      const boxes = [{ x: r.left, y: r.top, right: r.right, bottom: r.bottom }];
      const after = getComputedStyle(el, "::after");
      if (after.content !== "none" && after.position === "absolute") {
        // Placed from the padding box, inside any border, then transformed.
        const m = after.transform && after.transform !== "none" ? new DOMMatrixReadOnly(after.transform) : new DOMMatrixReadOnly();
        const left = r.left + el.clientLeft + parseFloat(after.left) + m.e;
        const top = r.top + el.clientTop + parseFloat(after.top) + m.f;
        boxes.push({ x: left, y: top, right: left + parseFloat(after.width), bottom: top + parseFloat(after.height) });
      }
      return boxes;
    };
    const overlap = (A: Box, B: Box) => [Math.min(A.right, B.right) - Math.max(A.x, B.x), Math.min(A.bottom, B.bottom) - Math.max(A.y, B.y)];
    const targets = [...root.querySelectorAll("a[href], button")].filter((el) => el.getClientRects().length > 0 && !el.closest(".popover-panel"));
    const name = (el: Element) => `${el.tagName.toLowerCase()}.${[...el.classList].join(".")} "${(el.textContent ?? "").trim().slice(0, 14)}"`;
    const met: string[] = [];
    for (let i = 0; i < targets.length; i++) {
      for (let j = i + 1; j < targets.length; j++) {
        const [a, b] = [targets[i], targets[j]];
        if (a.contains(b) || b.contains(a) || !(a.matches(links) || b.matches(links))) continue;
        for (const A of areas(a)) {
          for (const B of areas(b)) {
            const [w, h] = overlap(A, B);
            if (w > 0.5 && h > 0.5) met.push(`${name(a)} meets ${name(b)} (${w.toFixed(1)} × ${h.toFixed(1)})`);
          }
        }
      }
    }
    return met;
  }, LINKS);
}

const card = (hash: string) => `#app .cast-card[data-hash="${hash}"]`;

test("on a touch screen, a card's and a quote's meta-line links take a tap of 25 px or more, and none meets another target", async ({ browser }, testInfo) => {
  const { context, page } = await at(browser, testInfo, true);
  await serve(page);
  await page.goto("/");
  for (const hash of [short.hash, wrapped.hash, quoting.hash, imageOnly.hash, plain.hash]) {
    const root = card(hash);
    await page.locator(root).scrollIntoViewIfNeeded();
    for (const t of await taps(page, root)) {
      expect(t.height, `${t.link}: its tap's height`).toBeGreaterThanOrEqual(24.5);
      expect(t.lands, `${t.link}: a tap on it, 9 px above and below`).toEqual([true, true, true]);
    }
    expect(await meeting(page, root), `${hash}: no tap areas meet`).toEqual([]);
  }
  // A meta line on one line keeps its height (one with room to spare, so a
  // wider font can't wrap it); one that wraps keeps its lines 6 px apart.
  const lines = await page.evaluate((hashes) => hashes.map((h) => {
    const line = document.querySelector(`#app .cast-card[data-hash="${h}"] .cast-meta`)!;
    const items = [...line.children].filter((item) => item.getBoundingClientRect().width > 0);
    const used = items.reduce((width, item) => width + item.getBoundingClientRect().width, 0) + (parseFloat(getComputedStyle(line).columnGap) || 0) * (items.length - 1);
    return { height: Math.round(line.getBoundingClientRect().height), spare: line.clientWidth - used };
  }), [plain.hash, wrapped.hash]);
  expect(lines[0].spare, "the plain card's meta line: room to spare").toBeGreaterThanOrEqual(40);
  expect(lines[0].height).toBe(22);
  expect(lines[1].height).toBeGreaterThanOrEqual(22 + 6 + 15);
  expect(await page.locator(`${card(wrapped.hash)} .cast-meta`).evaluate((el) => getComputedStyle(el).rowGap)).toBe("6px");
  await page.screenshot({ path: testInfo.outputPath("meta-targets-390-feed.png"), fullPage: true });
  await context.close();
});

test("on a touch screen, a reply's and the post's meta-line links take a tap of 25 px or more, and none meets another target", async ({ browser }, testInfo) => {
  const { context, page } = await at(browser, testInfo, true);
  await serve(page);
  await page.goto(`/p/${short.hash}`);
  await expect(page.locator(".comment")).toHaveCount(2);
  for (const root of [".post", ".comment >> nth=0", ".comment >> nth=1"]) {
    await page.locator(root).first().scrollIntoViewIfNeeded();
    for (const t of await taps(page, root)) {
      expect(t.height, `${t.link}: its tap's height`).toBeGreaterThanOrEqual(24.5);
      expect(t.lands, `${t.link}: a tap on it, 9 px above and below`).toEqual([true, true, true]);
    }
  }
  expect(await meeting(page, ".post")).toEqual([]);
  expect(await meeting(page, ".replies")).toEqual([]);
  // A reply's head that wraps keeps its lines apart too.
  expect(await page.locator(".comment-head").first().evaluate((el) => getComputedStyle(el).rowGap)).toBe("6px");
  await page.screenshot({ path: testInfo.outputPath("meta-targets-390-post.png"), fullPage: true });
  await context.close();
});

test("with a mouse, the meta-line links are as they were", async ({ browser }, testInfo) => {
  const { context, page } = await at(browser, testInfo, false);
  await serve(page);
  await page.goto("/");
  const when = page.locator(`${card(short.hash)} .byline > a.when`);
  await expect(when).toBeVisible();
  expect(await when.evaluate((el) => [getComputedStyle(el).paddingTop, getComputedStyle(el).marginTop])).toEqual(["0px", "0px"]);
  expect(await page.locator(`${card(short.hash)} .cast-meta`).evaluate((el) => getComputedStyle(el).rowGap)).toBe("4px");
  await context.close();
});
