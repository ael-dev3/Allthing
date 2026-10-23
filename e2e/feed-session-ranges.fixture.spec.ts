// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { signIn } from "./helpers/farcaster";

test.skip(Boolean(process.env.E2E_PAGES), "these fixtures exercise signed-in feed tabs");

const now = Math.floor(Date.now() / 1000);
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const other = { fid: 5678, username: "other", display_name: "Other" };
const privateHeaders = { "cache-control": "private, no-store" };
const cast = (n: number, age: number, likes = 0) => ({
  hash: `0x${n.toString(16).padStart(40, "0")}`,
  author: { fid: n, username: `author${n}`, display_name: `Author ${n}` },
  text: `Cast ${n}`, ts: now - age, likes, recasts: 0, replies: 0,
});
const pool = [1800, 3 * 3600, 9 * 3600, 18 * 3600, 2 * 86400, 15 * 86400, 100 * 86400, 400 * 86400]
  .map((age, i) => cast(i + 1, age, i * 10));
// Upstreams and old cached payloads can contain invalid future dates. They
// must never dominate Hot/Top/New, even in All time or Following.
pool.push(cast(9, -86400, 1_000_000), { ...cast(10, 0, 1_000_000), ts: 2_893_718_638 });
const order = (page: Page) => page.locator("#app .cast-list .cast-card:not(.skeleton)")
  .evaluateAll((cards) => cards.map((card) => card.querySelector(".cast-text")?.textContent?.trim()));

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  for (const path of ["home", "latest", "following"]) {
    await page.route(`**/api/v1/${path}**`, (route) => route.fulfill({
      // Deliberately include out-of-range casts, as an aged cached payload
      // can: the browser must apply its own current window too.
      json: { pool: new URL(route.request().url()).searchParams.has("offset") ? [] : pool, notes: {}, meta, index: path === "home" ? { casts: 100, since: now - 500 * 86400, ranked_at: Date.now() } : undefined },
      headers: path === "following" ? privateHeaders : {},
    }));
  }
  await page.route("**/api/v1/notifications**", (route) => route.fulfill({ json: { groups: [], meta }, headers: privateHeaders }));
  await page.route("https://haatz.quilibrium.com/**", (route) =>
    route.fulfill({ headers: { "access-control-allow-origin": "*" }, json: { messages: [] } }));
}

for (const width of [1280, 390]) {
  for (const feed of ["", "following"]) {
    test(`${feed || "For you"}: every sort and time range at ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 844 });
      await signIn(page, me);
      await serve(page);
      // Neither favorites nor Show less may disturb New's time order.
      await page.addInitScript(() => {
        localStorage.setItem("allthing.favorites", JSON.stringify([{ fid: 8, username: "author8" }]));
        localStorage.setItem("allthing.show-less", JSON.stringify([{ fid: 1, username: "author1" }]));
      });
      for (const sort of ["hot", "new", "top"]) {
        for (const [range, span] of [
          ["hour", 3600], ["6h", 6 * 3600], ["12h", 12 * 3600], ["day", 86400],
          ["week", 7 * 86400], ["month", 30 * 86400], ["year", 365 * 86400], ["all", Infinity],
        ] as const) {
          await page.goto(`/?sort=${sort}&t=${range}${feed ? `&feed=${feed}` : ""}`);
          const selector = page.getByRole("combobox", { name: "Time range" });
          await expect(selector).toBeVisible();
          await expect(selector).toHaveValue(range);
          const expected = pool.filter((c) => c.ts <= now && now - c.ts < span).map((c) => c.text);
          await expect.poll(async () => {
            // Earlier ranges can genuinely be read while their page loads.
            // Use this generation's control before checking exact membership.
            const show = page.getByRole("button", { name: "Show seen posts", exact: true });
            if (await show.isVisible()) await show.click();
            return (await order(page)).toSorted();
          }).toEqual(expected.toSorted());
          if (sort === "new") expect(await order(page)).toEqual(expected);
          expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
        }
      }
    });
  }
}

async function switchReader(page: Page, user: typeof me | null) {
  await page.evaluate((user) => {
    document.cookie = user
      ? `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax`
      : "__Host-allthing-user=; Path=/; Secure; Max-Age=0";
    const channel = new BroadcastChannel("allthing-session");
    channel.postMessage("changed");
    channel.close();
  }, user);
}

for (const feed of ["For you", "Following"]) {
  test(`${feed} hot ranks within its window before applying favorites`, async ({ page }) => {
    await signIn(page, me);
    await serve(page);
    await page.addInitScript(() => localStorage.setItem("allthing.favorites", JSON.stringify([{ fid: 1, username: "author1" }])));
    const quietFavorite = cast(1, 50 * 60);
    const busyRecent = cast(2, 10 * 60, 100);
    const outside = cast(3, 400 * 86400);
    const future = { ...cast(4, 0), ts: 2_893_718_638 };
    for (const path of ["home", "following"]) await page.route(`**/api/v1/${path}**`, (route) => route.fulfill({
      json: { pool: [future, outside, quietFavorite, busyRecent], notes: {}, meta }, headers: privateHeaders,
    }));
    await page.goto(`/?t=hour${feed === "Following" ? "&feed=following" : ""}`);
    // In an hour's pool, 25x a quiet cast's weight cannot beat 100 points
    // posted more recently. Including an old or future cast before ranking
    // scales that lift by its entire span, sending the favorite to the top.
    await expect.poll(() => order(page)).toEqual([busyRecent.text, quietFavorite.text]);
  });
}

test("Following crosses a cached future-only page to reach the present", async ({ page }) => {
  await signIn(page, me);
  await serve(page);
  const asked: number[] = [];
  const future = { ...cast(40, 0), ts: 2_893_718_638 };
  const present = cast(41, 60);
  await page.route("**/api/v1/following**", (route) => {
    const cursor = Number(new URL(route.request().url()).searchParams.get("cursor") || 0);
    asked.push(cursor);
    return route.fulfill({ json: { pool: cursor === 0 ? [future] : [present], next: cursor === 0 ? "1" : null, notes: {}, meta }, headers: privateHeaders });
  });
  await page.goto("/?feed=following&t=hour");
  await expect.poll(() => order(page)).toEqual([present.text]);
  expect(asked).toEqual([0, 1]);
});

test("Following crosses empty pages, stops at an old tail, and resumes when the range widens", async ({ page }) => {
  await signIn(page, me);
  await serve(page);
  const asked: number[] = [];
  await page.route("**/api/v1/following**", (route) => {
    const cursor = Number(new URL(route.request().url()).searchParams.get("cursor") || 0);
    asked.push(cursor);
    const source = cursor === 1 ? [] : [cast(cursor + 1, [1800, 0, 3 * 3600, 8 * 3600][cursor] || 86400)];
    return route.fulfill({ json: { pool: source, next: String(cursor + 1), notes: {}, meta }, headers: privateHeaders });
  });
  // A real client filter empties every rendered page while preserving the
  // source timestamps: the same sentinel path as a wholly seen batch.
  await page.goto("/?feed=following&t=hour&client=999999");
  await expect(page.locator(".following-end")).toHaveText("No more casts in this time range.");
  expect(asked).toEqual([0, 1, 2]);
  await page.getByRole("combobox", { name: "Time range" }).selectOption("6h");
  await expect.poll(() => asked).toEqual([0, 1, 2, 3]);
  await expect(page.locator(".following-end")).toBeVisible();
  expect(asked).toEqual([0, 1, 2, 3]);
});

test("a completely filtered Following feed needs an explicit action after ten payloads", async ({ page }) => {
  await signIn(page, me);
  await serve(page);
  const asked: number[] = [];
  await page.route("**/api/v1/following**", (route) => {
    const cursor = Number(new URL(route.request().url()).searchParams.get("cursor") || 0);
    asked.push(cursor);
    return route.fulfill({
      json: { pool: [cast(cursor + 1, 600 + cursor)], next: String(cursor + 1), notes: {}, meta }, headers: privateHeaders,
    });
  });
  await page.goto("/?feed=following&client=999999");
  const more = page.getByRole("button", { name: "Load more posts", exact: true });
  await expect(more).toBeVisible();
  expect(asked).toEqual(Array.from({ length: 10 }, (_, i) => i));
  await expect(page.locator("#app .cast-card:not(.skeleton)")).toHaveCount(0);
  await more.click();
  await expect.poll(() => asked).toEqual(Array.from({ length: 20 }, (_, i) => i));
  await expect(more).toBeVisible();
});

for (const feed of ["", "following"]) {
  test(`an account change replaces ${feed || "For you"}'s private state without reloading`, async ({ page }) => {
    await signIn(page, me);
    await serve(page);
    const a = cast(1, 600, 10);
    const b = cast(2, 600, 150);
    const readers: number[] = [];
    await page.route("**/api/v1/home**", (route) => route.fulfill({ json: { pool: [b, a], notes: {}, meta } }));
    await page.route("**/api/v1/following**", (route) => {
      const cookie = route.request().headers().cookie || "";
      const reader = cookie.includes(encodeURIComponent('"fid":5678')) ? other.fid : me.fid;
      readers.push(reader);
      return route.fulfill({ json: { pool: [reader === me.fid ? a : b], notes: {}, meta }, headers: privateHeaders });
    });
    await page.addInitScript(() => {
      localStorage.setItem("allthing.followed", JSON.stringify({ reader: 1234, at: Date.now(), fids: [1] }));
      localStorage.setItem("allthing.followed-tried", JSON.stringify({ 1234: Date.now() }));
    });
    await page.goto(feed ? "/?feed=following" : "/");
    await expect.poll(() => order(page)).toEqual(feed ? [a.text] : [a.text, b.text]);
    await switchReader(page, other);
    await expect.poll(() => order(page)).toEqual(feed ? [b.text] : [b.text, a.text]);
    // A's recent attempt must not suppress B's refresh for ten minutes.
    await expect.poll(() => readers.includes(other.fid)).toBe(true);
    await expect.poll(() => page.evaluate(() => JSON.parse(localStorage.getItem("allthing.followed") || "null")))
      .toMatchObject({ reader: other.fid, fids: [2] });
    await switchReader(page, null);
    await expect(page.getByRole("navigation", { name: "Feed" })).toHaveCount(0);
    await expect.poll(() => order(page)).toEqual([b.text, a.text]);
  });
}

test("a follow refresh from a signed-out account cannot save or refill the private cache", async ({ page }) => {
  await signIn(page, me);
  await serve(page);
  let release!: () => void;
  const held = new Promise<void>((resolve) => { release = resolve; });
  let asked = 0;
  await page.route("**/api/v1/following**", async (route) => {
    asked += 1;
    if (asked === 1) await held;
    await route.fulfill({ json: { pool: [cast(asked === 1 ? 1 : 2, 600)], notes: {}, meta }, headers: privateHeaders });
  });
  await page.goto("/");
  await expect.poll(() => asked).toBe(1);
  await switchReader(page, null);
  await expect(page.getByRole("navigation", { name: "Feed" })).toHaveCount(0);
  release();
  await switchReader(page, other);
  await expect.poll(() => asked).toBe(2);
  await expect.poll(() => page.evaluate(() => JSON.parse(localStorage.getItem("allthing.followed") || "null")))
    .toMatchObject({ reader: other.fid, fids: [2] });
});
