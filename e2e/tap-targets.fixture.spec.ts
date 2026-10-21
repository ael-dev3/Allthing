// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Locator, type Page } from "@playwright/test";

/*
 * On a phone, the small targets of the header, a feed, a post, a profile
 * and the footer each take a tap 44 px tall, centred on them, with the
 * layout as it was: a band that is the target's own (its ::after), as wide
 * as the target, so one never covers the next (the sign-in button, on its
 * own, 44 px wide too). A tap on each one's middle is still its own.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in or writes");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const cast = { hash: hex(1), author: alice, text: "Small targets", ts, likes: 3, recasts: 1, replies: 1 };
const reply = { hash: hex(2), author: { fid: 7, username: "bob", display_name: "Bob" }, text: "A reply", ts: ts + 60, likes: 0, recasts: 0, replies: 0, parent_hash: cast.hash };

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [cast], notes: {}, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) =>
    route.fulfill({ json: { thread: { cast, children: [{ cast: reply, children: [] }] }, notes: {}, meta } }),
  );
  await page.route("**/api/v1/user?*", (route) =>
    route.fulfill({ json: { user: { ...alice, follower_count: 12, following_count: 3 }, pool: [cast], notes: {}, meta } }),
  );
}

/** For the screenshots only: each tap band, outlined. */
const outlined = [
  ".main-nav a", ".sign-in-button", ".sort-tabs a", ".tabs a", ".engagement .act", ".engagement .popover-trigger",
  ".reply-button", ".follow-button", ".favorite-button", ".show-less-button", ".site-footer a",
].map((target) => `${target}::after`).join(", ") + " { outline: 1px dashed #d6247a; outline-offset: -1px; }";

/** The tap band of `target`: its ::after, as laid out. */
async function band(target: Locator) {
  return target.evaluate((el) => {
    const after = getComputedStyle(el, "::after");
    const box = el.getBoundingClientRect();
    return { width: parseFloat(after.width), height: parseFloat(after.height), box: { w: box.width, h: box.height } };
  });
}

/** Whether a tap `dy` px above or below the middle of `target` lands on it:
 * no further than the edge of the card it's in, which clips what passes it
 * (a feed card skips drawing out of view, `content-visibility: auto`). */
async function lands(target: Locator, dy: number) {
  return target.evaluate((el, dy) => {
    const box = el.getBoundingClientRect();
    const card = el.closest(".cast-card")?.getBoundingClientRect();
    const middle = box.y + box.height / 2;
    const y = card ? Math.min(Math.max(middle + dy, card.top + 2), card.bottom - 2) : middle + dy;
    const hit = document.elementFromPoint(box.x + box.width / 2, y);
    return hit === el || el.contains(hit);
  }, dy);
}

test("at 390 on a touch screen, the small targets take a 44 px tap", async ({ browser }, info) => {
  const context = await browser.newContext({ baseURL: info.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  const page = await context.newPage();
  await serve(page);
  expect(await page.evaluate(() => matchMedia("(pointer: coarse)").matches)).toBe(true);
  const check = async (name: string, target: Locator, { wide = false } = {}) => {
    await expect(target, name).toBeVisible();
    const b = await band(target);
    expect(b.height, `${name}: band height`).toBeGreaterThanOrEqual(44);
    if (wide) expect(b.width, `${name}: band width`).toBeGreaterThanOrEqual(44);
    // As wide as it is, inside any border: never over a neighbour.
    else expect(b.box.w - b.width, `${name}: band width`).toBeGreaterThanOrEqual(-0.5);
    if (!wide) expect(b.box.w - b.width, `${name}: band width`).toBeLessThanOrEqual(4);
    // Its middle, and past its own box above and below, a tap lands on it.
    const reach = Math.max(b.box.h / 2 + 3, 20);
    expect(await lands(target, 0), `${name}: middle`).toBe(true);
    expect(await lands(target, -reach), `${name}: above`).toBe(true);
    expect(await lands(target, reach), `${name}: below`).toBe(true);
  };

  await page.goto("/");
  await check("Home", page.locator(".main-nav a"));
  await check("Sign in", page.locator(".sign-in-button"), { wide: true });
  await check("hot", page.locator(".sort-tabs a").first());
  const card = page.locator(`.cast-card[data-hash="${cast.hash}"]`);
  await check("the heart", card.getByRole("button", { name: "Like", exact: true }));
  await check("the likes count", card.locator(".stat-likes .popover-trigger"));
  await check("the replies count", card.locator(".stat-replies"));
  await page.screenshot({ path: info.outputPath("tap-targets-390-feed.png"), style: outlined });

  await page.goto(`/p/${cast.hash}`);
  await check("Reply", page.locator(".post").getByRole("button", { name: "Reply to @alice" }));
  await check("Quote", page.locator(".post").getByRole("button", { name: "Quote @alice's cast" }));
  await check("a reply's Reply", page.locator(".comment").getByRole("button", { name: "Reply to @bob" }));

  await page.goto("/u/alice");
  await check("Follow", page.getByRole("button", { name: "Follow @alice" }));
  await check("Favorite", page.getByRole("button", { name: "Favorite" }));
  await check("Casts", page.locator(".tabs a").first());
  await check("the footer's Source", page.locator(".site-footer").getByRole("link", { name: "Source on GitHub" }));
  // No wider than the screen for all of it.
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await page.screenshot({ path: info.outputPath("tap-targets-390-profile.png"), style: outlined });
  await context.close();
});

test("with a mouse, the targets are as they were", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  const heart = page.locator(`.cast-card[data-hash="${cast.hash}"]`).getByRole("button", { name: "Like", exact: true });
  await expect(heart).toBeVisible();
  expect(await heart.evaluate((el) => getComputedStyle(el, "::after").content)).toBe("none");
});
