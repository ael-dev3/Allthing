// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Locator, type Page } from "@playwright/test";

/*
 * Item 91, on a post page with replies: every Likes, Recasts and Replies
 * popover, the post's and a reply's, is fully opaque, has a shadow, sits
 * above the cards around it, and has no native tooltip, at 1280 and at 390.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const person = (fid: number, username: string) => ({ fid, username, display_name: `Person ${fid}` });
const post = { hash: hex(1), author: person(42, "alice"), text: "A post with replies", ts, likes: 12, recasts: 4, replies: 3 };
// Long replies, so a reply's popover lies over the next reply's card.
const reply = (n: number) => ({
  hash: hex(10 + n), author: person(50 + n, `replier${n}`), text: `Reply ${n}, long enough to fill its card. `.repeat(6),
  ts: ts + 60 * n, parent_hash: post.hash, likes: 3, recasts: 2, replies: 1,
});
const nested = { hash: hex(30), author: person(60, "nested"), text: "A reply to a reply", ts: ts + 600, parent_hash: hex(11) };
const people = Array.from({ length: 10 }, (_, i) => person(100 + i, `person${i}`));

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
    thread: { cast: post, children: [1, 2, 3].map((n) => ({ cast: reply(n) })) }, notes: {}, meta,
  } }));
  await page.route("**/api/v1/reactions?*", (route) => {
    const url = new URL(route.request().url());
    const kind = url.searchParams.get("type") ?? url.searchParams.get("kind") ?? "likes";
    return route.fulfill({ json: { hash: url.searchParams.get("hash"), kind, users: people, more: false, meta } });
  });
  await page.route("**/api/v1/replies?*", (route) => route.fulfill({ json: {
    hash: new URL(route.request().url()).searchParams.get("hash"), replies: [nested], notes: {}, meta,
  } }));
}

/** Every animation that ends has ended: the popover has finished coming in. */
const settled = (page: Page) =>
  page.evaluate(() => Promise.all(document.getAnimations().filter((a) => a.effect?.getComputedTiming().iterations !== Infinity).map((a) => a.finished)));

/** Item 91's rules for one open popover. */
async function opaqueAboveWithoutTooltips(page: Page, panel: Locator) {
  await expect(panel.getByRole("link").first()).toBeVisible();
  await settled(page);
  const facts = await panel.evaluate((p) => {
    // Opaque all the way up: no ancestor fades it either.
    let opacity = 1;
    for (let e: Element | null = p; e; e = e.parentElement) opacity *= Number(getComputedStyle(e).opacity);
    const style = getComputedStyle(p);
    // What is on top across the panel, at a grid of points in view: the
    // panel itself, never a card under or beside it.
    const r = p.getBoundingClientRect();
    let covered = 0;
    for (let i = 1; i < 6; i++) {
      for (let j = 1; j < 6; j++) {
        const [x, y] = [r.left + (r.width * i) / 6, r.top + (r.height * j) / 6];
        if (x < 0 || y < 0 || x >= innerWidth || y >= innerHeight) continue;
        const hit = document.elementFromPoint(x, y);
        if (!hit || !p.contains(hit)) covered += 1;
      }
    }
    return {
      opacity, background: style.backgroundColor, backdrop: style.backdropFilter, shadow: style.boxShadow !== "none", covered,
      tooltips: p.querySelectorAll("[title], svg title").length,
    };
  });
  expect(facts).toEqual({ opacity: 1, background: "rgb(255, 255, 255)", backdrop: "none", shadow: true, covered: 0, tooltips: 0 });
}

for (const width of [1280, 390]) {
  test(`at ${width}, the post's and a reply's Likes, Recasts and Replies popovers are opaque, shadowed, on top and tooltip-free`, async ({ browser }, testInfo) => {
    const phone = width === 390;
    const context = await browser.newContext({
      baseURL: testInfo.project.use.baseURL, viewport: { width, height: phone ? 844 : 900 }, isMobile: phone, hasTouch: phone,
    });
    const page = await context.newPage();
    await serve(page);
    await page.goto(`/p/${post.hash}`);
    await expect(page.locator(".replies .comment")).toHaveCount(3);
    // Nothing on the page has a native tooltip.
    await expect(page.locator("main [title], main svg title")).toHaveCount(0);
    const popovers: [string, Locator, string, string][] = [
      ["the post", page.locator(".post .engagement"), "12 likes: show who", "Liked by"],
      ["the post", page.locator(".post .engagement"), "4 recasts: show who", "Recasted by"],
      ["the post", page.locator(".post .engagement"), "3 replies: preview", "Replies"],
      ["a reply", page.locator(".replies .comment").first().locator(".engagement").first(), "3 likes: show who", "Liked by"],
      ["a reply", page.locator(".replies .comment").first().locator(".engagement").first(), "2 recasts: show who", "Recasted by"],
      ["a reply", page.locator(".replies .comment").first().locator(".engagement").first(), "1 reply: preview", "Replies"],
    ];
    for (const [whose, foot, trigger, name] of popovers) {
      await test.step(`${whose}: ${name}`, async () => {
        const button = foot.getByRole("button", { name: trigger, exact: true });
        await button.scrollIntoViewIfNeeded();
        if (phone) await button.tap();
        else await button.hover();
        const panel = page.getByRole("dialog", { name, exact: true });
        await opaqueAboveWithoutTooltips(page, panel);
        await page.screenshot({ path: testInfo.outputPath(`${whose === "the post" ? "post" : "reply"}-${name.split(" ")[0].toLowerCase()}-${width}.png`) });
        await page.keyboard.press("Escape");
        if (!phone) await page.mouse.move(2, 2);
        await expect(panel).toBeHidden();
      });
    }
    await context.close();
  });
}
