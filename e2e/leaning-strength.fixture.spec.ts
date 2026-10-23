// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

/*
 * How far Show less and Favorite move an account's casts, measured on the
 * front page: a day of 120 casts, 6 of them by a typical account whose
 * median place starts in the middle of hot, top and new. Seen less of, its
 * casts drop by at least 40% of the list in hot and top, into the last
 * quarter, while New stays chronological; as a favorite, they rise as far
 * in hot and top, into the first quarter, and new stays in time order.
 * Every cast is still listed. The casts are made as allthing_core::ranking's
 * `a_day_of_casts` makes them for `leanings_move_a_typical_account_far`.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
test.use({ viewport: { width: 1280, height: 900 } });

const TYPICAL = 100;
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const now = Math.floor(Date.now() / 1000);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const typical = { fid: TYPICAL, username: "typical", display_name: "Typical", follower_count: 10, following_count: 5, bio: "" };
const account = (fid: number) => (fid === TYPICAL ? typical : { fid, username: `account${fid}`, display_name: `Account ${fid}` });

/** 114 casts by 29 accounts at random times over the past 24 hours, with 3
 * to 446 points, and 6 by the typical account, one every 4 hours, the older
 * ones busier, with 20 to 80. */
function aDayOfCasts() {
  // Numerical Recipes' generator, seeded with a date, as in ranking.rs.
  let state = 20261021;
  const unit = () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    return (state >>> 8) / 2 ** 24;
  };
  const cast = (fid: number, n: number, likes: number, ts: number) => ({
    hash: hex(n),
    author: account(fid),
    text: `${account(fid).display_name}, cast ${n}`,
    ts,
    likes,
    recasts: 0,
    replies: 0,
  });
  const others = Array.from({ length: 114 }, (_, n) => {
    const age = Math.floor(unit() * 86_400);
    const points = Math.floor(10 ** (1.6 + 0.7 * (unit() + unit() + unit() - 1.5)));
    return cast(1 + (n % 29), n, points, now - age);
  });
  const theirs = [20, 30, 35, 45, 60, 80].map((points, k) => cast(TYPICAL, 200 + k, points, now - (2 + 4 * k) * 3600));
  return [...others, ...theirs];
}

const day = aDayOfCasts();
const n = day.length;
const hashes = day.map((c) => c.hash).sort();

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : day, notes: {}, meta } });
  });
  await page.route("**/api/v1/latest", (route) => route.fulfill({ json: { pool: day, notes: {}, cursors: [], meta } }));
  await page.route("**/api/v1/user?*", (route) =>
    route.fulfill({ json: { user: typical, pool: day.filter((c) => c.author.fid === TYPICAL), notes: {}, meta } }),
  );
}

const FEEDS = { hot: "/", top: "/?sort=top", new: "/?sort=new" };
type Sort = keyof typeof FEEDS;
const SORTS = Object.keys(FEEDS) as Sort[];

/** The front page's list in `sort`, every card drawn (it grows by 25 as its
 * end comes near): each card's hash and account. */
async function listed(page: Page, sort: Sort) {
  await page.goto(FEEDS[sort]);
  await expect(page.locator("#app .cast-list")).toBeVisible();
  // Compare ranking with the complete pool through the reader's real control.
  const seen = page.getByRole("button", { name: "Show seen posts", exact: true });
  if (await seen.isVisible()) await seen.click();
  const cards = page.locator("#app .cast-list .cast-card:not(.skeleton)");
  await expect(cards.first()).toBeVisible();
  await expect
    .poll(
      async () => {
        await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
        return cards.count();
      },
      { timeout: 30_000 },
    )
    .toBe(n);
  const list = await cards.evaluateAll((all) =>
    all.map((card) => ({ hash: (card as HTMLElement).dataset.hash ?? "", fid: Number((card as HTMLElement).dataset.fid) })),
  );
  // Every cast is listed, once.
  expect(list.map((card) => card.hash).sort()).toEqual(hashes);
  return list;
}

/** The median place, counting from 1, of the typical account's casts. */
function medianPlace(list: { fid: number }[]) {
  const places = list.flatMap((card, i) => (card.fid === TYPICAL ? [i + 1] : []));
  return (places[(places.length - 1) >> 1] + places[places.length >> 1]) / 2;
}

/** The typical account's median place in each sort, and the lists. */
async function measure(page: Page) {
  const lists = {} as Record<Sort, { hash: string; fid: number }[]>;
  const places = {} as Record<Sort, number>;
  for (const sort of SORTS) {
    lists[sort] = await listed(page, sort);
    places[sort] = medianPlace(lists[sort]);
  }
  return { lists, places };
}

async function lean(page: Page, button: "Show less" | "Favorite") {
  await page.goto("/u/typical");
  const pressed = page.getByRole("button", { name: button });
  await pressed.click();
  await expect(pressed).toHaveAttribute("aria-pressed", "true");
}

test("Show less drops a typical account's casts from the middle of hot and top into the last quarter, while New stays chronological", async ({ page }) => {
  await serve(page);
  const before = await measure(page);
  await lean(page, "Show less");
  const after = await measure(page);
  console.log(`Show less: the typical account's median place of ${n}, before and after:`, JSON.stringify({ before: before.places, after: after.places }));
  for (const sort of SORTS) {
    expect(before.places[sort], `${sort} starts in the middle`).toBeGreaterThanOrEqual(0.4 * n);
    expect(before.places[sort], `${sort} starts in the middle`).toBeLessThanOrEqual(0.6 * n);
  }
  for (const sort of ["hot", "top"] as const) {
    expect(after.places[sort] - before.places[sort], `${sort}: down by 40% of the list`).toBeGreaterThanOrEqual(0.4 * n);
    expect(after.places[sort], `${sort}: in the last quarter`).toBeGreaterThan(0.75 * n);
  }
  expect(after.lists.new, "new: in time order, as before").toEqual(before.lists.new);
});

test("Favorite raises a typical account's casts from the middle of hot and top into the first quarter, and new stays in time order", async ({ page }) => {
  await serve(page);
  const before = await measure(page);
  await lean(page, "Favorite");
  const after = await measure(page);
  console.log(`Favorite: the typical account's median place of ${n}, before and after:`, JSON.stringify({ before: before.places, after: after.places }));
  for (const sort of ["hot", "top"] as const) {
    expect(before.places[sort] - after.places[sort], `${sort}: up by 40% of the list`).toBeGreaterThanOrEqual(0.4 * n);
    expect(after.places[sort], `${sort}: in the first quarter`).toBeLessThanOrEqual(0.25 * n);
  }
  expect(after.lists.new, "new: in time order, as before").toEqual(before.lists.new);
});
