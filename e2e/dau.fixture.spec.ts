// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";

/*
 * /dau: Farcaster's daily active accounts as the cron counts them, laid out
 * after SnapMeter's dashboard. `/api/v1/activity` is a fixture.
 */

const DAY = 86_400;
// The page's clock is pinned to noon UTC today (`serve`), so "today", how far
// reading has got and the clock never fall either side of midnight, as they
// did for the minute after it, when reading was a minute behind in the day
// before.
const today = Math.floor(Date.now() / 1000 / DAY);
const now = today * DAY + 12 * 3_600;
const reached = now - 60;
const to = Math.floor(reached / 3_600) * 3_600;
const hour = `${String(new Date(to * 1000).getUTCHours()).padStart(2, "0")}:00 UTC`;
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const short = (day: number) => { const d = new Date(day * DAY * 1000); return `${d.getUTCDate()} ${MONTHS[d.getUTCMonth()]}`; };

// 30 days: the first begun mid-day, the last in progress, the busiest 7,200.
const days = Array.from({ length: 30 }, (_, i) => {
  const day = today - 29 + i;
  const active = i === 29 ? 2_100 : i === 0 ? 3_300 : i === 12 ? 7_200 : 6_000 + (i % 5) * 100;
  return {
    day, active, complete: i > 0 && i < 29,
    actions: { reaction: 19_600 + i, cast: 12_300, link: 10_700, user_data: 1_900, verification: 600, username_proof: 12 },
  };
});
const yesterday = days[28];

function payload(extra: object = {}) {
  return {
    reached, rolling_to: to, rolling_24h: 6_174, previous_24h: 5_982,
    active_30d: { day: today - 1, active: 41_234, complete: false },
    since: (today - 29) * DAY + 5_000,
    days,
    apps_24h: {
      to, counted: to + 120, active: 6_174, none: 14,
      apps: [
        { app: 9152, active: 4_812 }, { app: 309857, active: 1_203 }, { app: 16823, active: 120 }, { app: 777, active: 3 },
      ],
    },
    app_accounts: [
      { fid: 9152, username: "warpcast", display_name: "Warpcast" },
      { fid: 309857, username: "baseapp.base.eth", display_name: "Base App" },
      { fid: 16823, username: "recaster", display_name: "Recaster" },
    ],
    meta: { backend: "hypersnap", fetched_at: Date.now() },
    ...extra,
  };
}

/** Serve `body` (or 503, before the first count); the requests are counted. */
async function serve(page: Page, body: object | null) {
  await page.clock.setFixedTime(now * 1000);
  const calls: string[] = [];
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/activity", (route) => {
    calls.push(new URL(route.request().url()).pathname);
    return body
      ? route.fulfill({ json: body })
      : route.fulfill({ status: 503, headers: { "retry-after": "120" }, json: { error: "Active accounts aren't counted yet" } });
  });
  return calls;
}

// As after a deploy: counting began at 23:29 the day before yesterday, and
// reading has got to 08:24 yesterday, many hours behind the clock.
const behind = (today - 1) * DAY + 8 * 3_600 + 24 * 60 + 7;
const actions = (n: number) => ({ reaction: 316 * n, cast: 156 * n, link: 45 * n, user_data: 44 * n, verification: 7 * n, storage_lending: 6 * n });
const catchingUp = {
  reached: behind, rolling_to: Math.floor(behind / 3_600) * 3_600, since: (today - 1) * DAY - 1_808,
  days: [
    { day: today - 2, active: 134, complete: false, actions: actions(1) },
    { day: today - 1, active: 3_047, complete: false, actions: actions(10) },
  ],
  meta: { backend: "hypersnap", fetched_at: Date.now() },
};

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true,
  });
  return { context, page: await context.newPage() };
}

test.describe("with the Worker", () => {
  test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no count");

  test("the past 24 hours against the 24 before, today, yesterday and the past 30 days", async ({ page }) => {
    const calls = await serve(page, payload());
    await page.goto("/dau");
    await expect(page).toHaveTitle(/Daily active accounts/);
    await expect(page.getByRole("heading", { level: 1, name: "Daily active accounts" })).toBeVisible();
    const meter = page.locator(".dau-meter");
    await expect(meter).toContainText(`6,174 accounts active in the 24 hours to ${hour}`);
    // One request, started by the inline script before the app, which took it.
    expect(calls).toEqual(["/api/v1/activity"]);
    expect(await page.evaluate(() => "__allthingEarly" in window)).toBe(false);
    await expect(meter.locator(".dau-change")).toHaveText("↑ 3.2% on the 24 hours before (5,982)");
    await expect(meter.locator(".dau-change")).toHaveClass(/up/);
    const tiles = page.locator(".dau-tile");
    await expect(tiles).toHaveCount(3);
    await expect(tiles.nth(0)).toContainText("Today so far");
    await expect(tiles.nth(0)).toContainText("2,100");
    await expect(tiles.nth(1)).toContainText(`${(yesterday.active).toLocaleString("en-US")}`);
    await expect(tiles.nth(1)).toContainText(short(yesterday.day));
    await expect(tiles.nth(2)).toContainText("41,234");
    await expect(tiles.nth(2)).toContainText(`to the end of ${short(today - 1)}, since counting began`);
  });

  test("the month's days as bars and as a table, and what the accounts did", async ({ page }) => {
    await serve(page, payload());
    await page.goto("/dau");
    const chart = page.locator(".dau-chart");
    const bars = chart.locator("rect.dau-bar");
    await expect(bars).toHaveCount(30);
    await expect(bars.first()).toHaveClass(/partial/);
    await expect(bars.last()).toHaveClass(/today/);
    await expect(bars.nth(12)).not.toHaveClass(/partial|today/);
    await expect(chart.getByRole("img")).toHaveAttribute("aria-label", new RegExp(`most 7,200 on ${short(today - 17)}$`));
    // No native tooltip: a day's count and date show above its bar on hover,
    // on keyboard focus (the arrows go from day to day), and on a tap.
    await expect(chart.locator("svg title")).toHaveCount(0);
    const hits = chart.locator("rect.dau-hit");
    const tip = chart.locator(".dau-tip");
    await expect(tip).toBeHidden();
    await hits.nth(12).hover();
    await expect(tip).toContainText("7,200 accounts");
    await expect(tip.locator(".dau-tip-day")).toHaveText(new RegExp(`^${short(today - 17)} `));
    await expect(bars.nth(12)).toHaveClass(/\bon\b/);
    await page.screenshot({ path: test.info().outputPath("dau-tip-1280.png") });
    await hits.last().hover();
    await expect(tip).toContainText("2,100 accounts so far");
    await page.mouse.move(2, 2);
    await expect(tip).toBeHidden();
    await chart.locator("svg").focus();
    await expect(tip).toContainText("2,100 accounts so far");
    await page.keyboard.press("ArrowLeft");
    await expect(tip.locator(".dau-tip-day")).toHaveText(new RegExp(`^${short(today - 1)} `));
    await page.keyboard.press("Home");
    await expect(tip.locator(".dau-tip-day")).toHaveText(new RegExp(`^${short(today - 29)} `));
    await page.keyboard.press("Escape");
    await expect(tip).toBeHidden();
    // A label a week, ending under today's bar.
    await expect(chart.locator(".dau-ticks span")).toHaveText([28, 21, 14, 7, 0].map((back) => short(today - back)));
    // The table: newest first.
    await chart.getByText("The days as a table").click();
    const rows = chart.locator("tbody tr");
    await expect(rows).toHaveCount(30);
    await expect(rows.first().locator("th")).toContainText("(so far)");
    await expect(rows.first().locator("td").first()).toHaveText("2,100");
    await expect(rows.last().locator("th")).toContainText("(partly)");
    // The last whole day's messages, by kind, largest first.
    const mix = page.locator(".dau-mix");
    await expect(mix).toContainText(`45,140 messages merged on ${short(yesterday.day)}, by kind.`);
    await expect(mix.locator("li .dau-mix-name")).toHaveText(["Reactions", "Casts", "Follows", "Profile changes", "Verifications", "Username proofs"]);
    await expect(mix.locator("li").first()).toContainText("19,628");
    await expect(mix.locator("li").first()).toContainText("43%");
    await expect(mix.locator("li").nth(4)).toContainText("1.3%");
    await expect(mix.locator("li").last()).toContainText("<0.1%");
  });

  test("accounts by app, and how they were counted", async ({ page }) => {
    await serve(page, payload());
    await page.goto("/dau");
    const apps = page.locator(".dau-apps");
    await expect(apps).toContainText(`Accounts active through each app in the 24 hours to ${hour}.`);
    await expect(apps.locator(".dau-note")).toHaveCount(0);
    const rows = apps.locator("li");
    await expect(rows.locator(".dau-app-name")).toHaveText(["Farcaster", "Base App", "Recaster", "app fid 777"]);
    await expect(rows.first().locator(".dau-app-count")).toHaveText("4,812");
    await expect(rows.first().locator(".dau-app-share")).toHaveText("78%");
    await expect(rows.nth(2).locator(".dau-app-share")).toHaveText("1.9%");
    // The Farcaster app's account is still named Warpcast: linked by fid.
    await expect(rows.first().getByRole("link", { name: "Farcaster" })).toHaveAttribute("href", /\/u\/fid(:|%3A)9152$/);
    await expect(rows.nth(1).getByRole("link", { name: "Base App" })).toHaveAttribute("href", /\/u\/baseapp\.base\.eth$/);
    await expect(rows.nth(3).getByRole("link")).toHaveCount(0);
    await expect(apps.locator(".dau-apps-foot")).toHaveText("14 accounts signed with keys no app requested.");
    const method = page.locator(".dau-method");
    await expect(method).toContainText("SnapMeter's closed list for Snapchain, message types 1–8 and 11–21");
    await expect(method).toContainText("(t − 24 h, t]");
    await expect(method).toContainText("Gaps: none.");
    await expect(method.getByRole("link", { name: "SnapMeter's definitions" })).toHaveAttribute("href", "https://github.com/ael-dev3/SnapMeter/blob/main/docs/metrics.md");
    await expect(method.getByRole("link", { name: "The data as JSON" })).toHaveAttribute("href", "/api/v1/activity");
  });

  test("while counting by app warms up, a gap, and a falling count", async ({ page }) => {
    const body = payload({
      previous_24h: 6_500,
      gaps: [[(today - 3) * DAY + 3_600, (today - 3) * DAY + 7_200]],
      apps_24h: { to, counted: to + 120, active: 6_174, unknown: 1_234, apps: [{ app: 9152, active: 3_900 }] },
    });
    await serve(page, body);
    await page.goto("/dau");
    await expect(page.locator(".dau-change")).toHaveText("↓ 5.0% on the 24 hours before (6,500)");
    await expect(page.locator(".dau-change")).toHaveClass(/down/);
    await expect(page.locator(".dau-apps .dau-note")).toHaveText("Apps are known for 80% of active accounts so far: the counts below are floors while the rest are looked up.");
    await expect(page.locator(".dau-apps-foot")).toHaveText("1,234 accounts with keys whose app isn't known yet.");
    await expect(page.locator(".dau-method")).toContainText(`Gaps: not read: ${short(today - 3)}`);
  });

  test("before the first count, the page says so", async ({ page }) => {
    await serve(page, null);
    await page.goto("/dau");
    const empty = page.locator(".dau-empty");
    await expect(empty.getByRole("heading", { name: "Not counted yet" })).toBeVisible();
    await expect(empty).toContainText("Allthing hasn't counted active accounts yet.");
    // Before a whole window: no 24-hour figure, and when it comes.
    await serve(page, payload({ rolling_24h: undefined, previous_24h: undefined, since: to - 3_600 }));
    await page.reload();
    const first = to - 3_600 + DAY;
    const at = `${String(new Date(first * 1000).getUTCHours()).padStart(2, "0")}:00 UTC`;
    await expect(page.locator(".dau-meter")).toHaveText(`The first 24-hour count comes once the 24 hours to ${short(Math.floor(first / DAY))}, ${at} have been read.`);
    await expect(page.locator(".dau-behind")).toHaveCount(0);
  });

  test("while catching up, today and yesterday follow the clock, and the page says how far it has read", async ({ page }) => {
    await serve(page, catchingUp);
    await page.goto("/dau");
    const year = new Date((today - 1) * DAY * 1000).getUTCFullYear();
    await expect(page.locator(".dau-behind")).toHaveText(
      `Catching up: Allthing has read the network's messages to ${short(today - 1)} ${year}, 08:24 UTC so far, and reads them faster than they come until it is current. The counts below go to then.`,
    );
    await expect(page.getByRole("status").filter({ hasText: "Catching up" })).toBeVisible();
    await expect(page.locator(".dau-meter")).toHaveText(`The first 24-hour count comes once the 24 hours to ${short(today)}, 00:00 UTC have been read.`);
    // Today hasn't been read; yesterday is being read; no whole day yet.
    const tiles = page.locator(".dau-tile");
    await expect(tiles.nth(0).locator(".dau-tile-count")).toHaveText("—");
    await expect(tiles.nth(0).locator(".dau-tile-note")).toHaveText("not read yet");
    await expect(tiles.nth(1).locator(".dau-tile-count")).toHaveText("3,047");
    await expect(tiles.nth(1).locator(".dau-tile-note")).toHaveText(`${short(today - 1)}, read to 08:24 UTC so far`);
    await expect(tiles.nth(2).locator(".dau-tile-count")).toHaveText("—");
    await expect(tiles.nth(2).locator(".dau-tile-note")).toHaveText("once a whole day has been read");
    // The chart ends today, not read yet; the day being read is partly
    // counted so far, not today's.
    const chart = page.locator(".dau-chart");
    const bars = chart.locator("rect.dau-bar");
    await expect(bars).toHaveCount(2);
    await expect(bars.last()).toHaveClass(/partial/);
    await expect(bars.last()).not.toHaveClass(/today/);
    const hits = chart.locator("rect.dau-hit");
    await hits.last().hover();
    await expect(chart.locator(".dau-tip")).toContainText("3,047 accounts (read so far)");
    await hits.first().hover();
    await expect(chart.locator(".dau-tip")).toContainText("134 accounts (partly counted)");
    const [svg, last] = await Promise.all([chart.locator("svg").boundingBox(), bars.last().boundingBox()]);
    // Yesterday's bar sits one slot in from the right, today's slot empty.
    expect(svg!.x + svg!.width - (last!.x + last!.width)).toBeGreaterThan(svg!.width / 30);
    // The last label names that bar, under it; today's empty slot has none.
    const label = chart.locator(".dau-ticks span").last();
    await expect(label).toHaveText(short(today - 1));
    await expect(chart.locator(".dau-ticks span", { hasText: short(today) })).toHaveCount(0);
    const tick = (await label.boundingBox())!;
    expect(Math.abs(tick.x + tick.width / 2 - (last!.x + last!.width / 2))).toBeLessThan(1);
    await chart.getByText("The days as a table").click();
    await expect(chart.locator("tbody tr").first().locator("th")).toContainText("(read so far)");
    // What they did: the day being read, said so, not the 31 minutes before it.
    await expect(page.locator(".dau-mix")).toContainText(`5,740 messages merged on ${short(today - 1)}, read to 08:24 UTC so far, by kind.`);
  });

  test("a short history keeps a month's room, the newest day on the right", async ({ page }) => {
    await serve(page, payload({ days: days.slice(-3) }));
    await page.goto("/dau");
    const svg = page.locator(".dau-chart svg");
    await expect(svg.locator("rect.dau-bar")).toHaveCount(3);
    const [chart, last] = await Promise.all([svg.boundingBox(), svg.locator("rect.dau-bar").last().boundingBox()]);
    expect(chart!.x + chart!.width - (last!.x + last!.width)).toBeLessThan(chart!.width / 30);
    expect(last!.width).toBeLessThan(chart!.width / 25);
  });

  test("a client card links to the page, which the app opens without a reload", async ({ page }) => {
    await serve(page, payload());
    const hash = (i: number) => `0x${(i + 1).toString(16).padStart(40, "0")}`;
    const pool = Array.from({ length: 25 }, (_, i) => ({ hash: hash(i), author: { fid: 1, username: "fixture" }, text: `DAU link fixture ${i}`, ts: 1792235681 - i }));
    const origins = Object.fromEntries(pool.map((c) => [c.hash, { signer: "0x01", app_fid: 9152 }]));
    await page.route("**/api/v1/home**", (route) => route.fulfill({
      json: { pool, notes: { origins, apps: { "9152": { fid: 9152, username: "warpcast", display_name: "Warpcast" } } }, meta: payload().meta, apps_24h: payload().apps_24h },
    }));
    await page.goto("/");
    // Gone if the page reloads. (The pinned clock fakes `performance`, so
    // its navigation entries can't say.)
    await page.evaluate(() => Object.assign(window, { sameDocument: true }));
    await page.locator(".client-legend").getByRole("link", { name: /Farcaster/ }).hover();
    const card = page.getByRole("dialog", { name: "Farcaster: active accounts" });
    // Its count is the link.
    const count = card.getByRole("link", { name: "4,812 active accounts, on the page of every app's active accounts" });
    await count.click();
    await expect(page).toHaveURL(/\/dau$/);
    await expect(page.locator(".dau-meter")).toContainText("6,174");
    expect(await page.evaluate(() => "sameDocument" in window)).toBe(true);
  });

  test("at 390, everything fits the screen", async ({ browser }, testInfo) => {
    const { context, page } = await phone(browser, testInfo);
    await serve(page, payload());
    await page.goto("/dau");
    await expect(page.locator(".dau-meter")).toContainText("6,174");
    for (const section of [".dau-meter", ".dau-tiles", ".dau-chart", ".dau-mix", ".dau-apps", ".dau-method"]) {
      const box = (await page.locator(section).boundingBox())!;
      expect(box.x, section).toBeGreaterThanOrEqual(0);
      expect(box.x + box.width, section).toBeLessThanOrEqual(390);
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    // A tapped day's count and date, on the card.
    const chart = page.locator(".dau-chart");
    await chart.locator("rect.dau-hit").nth(12).tap();
    const tip = chart.locator(".dau-tip");
    await expect(tip).toContainText("7,200 accounts");
    const [shown, card] = [(await tip.boundingBox())!, (await chart.boundingBox())!];
    expect(shown.x).toBeGreaterThanOrEqual(card.x);
    expect(shown.x + shown.width).toBeLessThanOrEqual(card.x + card.width);
    // Focused by the tap, the chart draws no ring.
    await expect(chart.locator("svg")).toHaveCSS("outline-style", "none");
    await chart.scrollIntoViewIfNeeded();
    await page.screenshot({ path: testInfo.outputPath("dau-tip-month-390.png") });
    await context.close();
  });

  test("at 390, while catching up, the chart's label names the bar above it, on the card", async ({ browser }, testInfo) => {
    const { context, page } = await phone(browser, testInfo);
    await serve(page, catchingUp);
    await page.goto("/dau");
    const chart = page.locator(".dau-chart");
    const last = chart.locator("rect.dau-bar").last();
    // A tap shows the day's count and date above its bar, on the card.
    await chart.locator("rect.dau-hit").last().tap();
    const tip = chart.locator(".dau-tip");
    await expect(tip).toContainText("3,047 accounts (read so far)");
    await expect(tip.locator(".dau-tip-day")).toHaveText(new RegExp(`^${short(today - 1)} `));
    const shown = (await tip.boundingBox())!;
    const onCard = (await chart.boundingBox())!;
    expect(shown.x).toBeGreaterThanOrEqual(onCard.x);
    expect(shown.x + shown.width).toBeLessThanOrEqual(onCard.x + onCard.width);
    await page.screenshot({ path: testInfo.outputPath("dau-tip-390.png") });
    const label = chart.locator(".dau-ticks span").last();
    await expect(label).toHaveText(short(today - 1));
    const [bar, tick, card] = [(await last.boundingBox())!, (await label.boundingBox())!, (await chart.boundingBox())!];
    expect(Math.abs(tick.x + tick.width / 2 - (bar.x + bar.width / 2))).toBeLessThan(1);
    expect(tick.x + tick.width).toBeLessThanOrEqual(card.x + card.width);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    await context.close();
  });
});

test("the static copy points to allthing.social for the counts", async ({ page }) => {
  test.skip(!process.env.E2E_PAGES, "only the static copy lacks the Worker");
  await page.goto("/Allthing/dau");
  await expect(page.locator(".dau-empty")).toContainText("They're at allthing.social/dau.");
  await expect(page.getByRole("link", { name: "allthing.social/dau" })).toHaveAttribute("href", "https://allthing.social/dau");
});
