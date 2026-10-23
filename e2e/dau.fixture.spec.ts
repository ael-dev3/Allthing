// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";

/*
 * /dau: Farcaster's daily active accounts as the cron counts them, laid out
 * after SnapMeter's dashboard: its meter, the history over 48 hours, 7 or
 * 30 days, what accounts did, which apps they used, and its definitions.
 * `/api/v1/activity` is a fixture.
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
const hh = (at: number) => `${String(new Date(at * 1000).getUTCHours()).padStart(2, "0")}:00`;
const hour = `${hh(to)} UTC`;
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
// Each of the 48 hours to `to`, oldest first: 500 to 1,199, and the busiest,
// 1,300, the hour that ended 30 hours before `to`.
const hourly = Array.from({ length: 48 }, (_, i) => (i === 17 ? 1_300 : 500 + ((i * 37) % 700)));
const busiest = `${short(Math.floor((to - 31 * 3_600) / DAY))}, ${hh(to - 31 * 3_600)}–${hh(to - 30 * 3_600)} UTC`;

function payload(extra: object = {}) {
  return {
    reached, rolling_to: to, rolling_24h: 6_174, previous_24h: 5_982,
    active_30d: { day: today - 1, active: 41_234, complete: false },
    since: (today - 29) * DAY + 5_000,
    days, hourly,
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

/** The history card, the chart of the range shown, and its parts. */
function shown(page: Page) {
  const chart = page.locator(".dau-chart");
  const plot = chart.locator(".dau-history:not([hidden])");
  return {
    chart, plot, ranges: chart.getByRole("group", { name: "Range" }).getByRole("button"),
    bars: plot.locator(".dau-bar"), hits: plot.locator(".dau-col"), tip: plot.locator(".dau-tip"),
  };
}

/** `inner` lies inside `outer`, left to right and top to bottom. */
async function inside(inner: Locator, outer: Locator) {
  const [a, b] = [(await inner.boundingBox())!, (await outer.boundingBox())!];
  expect(a.x).toBeGreaterThanOrEqual(b.x - 0.5);
  expect(a.x + a.width).toBeLessThanOrEqual(b.x + b.width + 0.5);
  expect(a.y).toBeGreaterThanOrEqual(b.y - 0.5);
  expect(a.y + a.height).toBeLessThanOrEqual(b.y + b.height + 0.5);
}

test.describe("with the Worker", () => {
  test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no count");

  test("the meter: the past 24 hours against the 24 before, today beside them, and SnapMeter's readouts", async ({ page }) => {
    const calls = await serve(page, payload());
    await page.goto("/dau");
    await expect(page).toHaveTitle(/Daily active accounts/);
    await expect(page.getByRole("heading", { level: 1, name: "Daily active accounts" })).toBeVisible();
    const meter = page.locator(".dau-meter");
    await expect(meter.getByRole("heading", { name: "Past 24 hours" })).toBeVisible();
    await expect(meter.locator(".dau-window")).toHaveText(`Unique accounts active in the 24 hours to ${hour}`);
    await expect(meter.locator(".dau-big")).toHaveText("6,174");
    // One request, started by the inline script before the app, which took it.
    expect(calls).toEqual(["/api/v1/activity"]);
    expect(await page.evaluate(() => "__allthingEarly" in window)).toBe(false);
    // The change as a count and a percent, signed: its direction is in the
    // signs and an arrow as well as in color.
    await expect(meter.locator(".dau-change")).toHaveText("↑ +192 · +3.2% on the 24 hours before (5,982)");
    await expect(meter.locator(".dau-change")).toHaveClass(/up/);
    // How far the counts go.
    await expect(meter.locator(".dau-status")).toHaveText("Up to date · 11:59 UTC");
    const tiles = meter.locator(".dau-tile");
    await expect(tiles.locator(".dau-tile-label")).toHaveText(["Today so far", "Yesterday", "Past 30 days", "7-day average"]);
    await expect(tiles.locator(".dau-tile-count")).toHaveText(["2,100", "6,300", "41,234", "6,214"]);
    await expect(tiles.nth(0).locator(".dau-tile-note")).toHaveText("00:00 to 11:59 UTC");
    await expect(tiles.nth(1).locator(".dau-tile-note")).toHaveText(short(yesterday.day));
    await expect(tiles.nth(2).locator(".dau-tile-note")).toHaveText(`to the end of ${short(today - 1)}, since counting began`);
    // The last 7 days read whole against the 7 before: 6,214 against 6,157.
    await expect(tiles.nth(3).locator(".dau-tile-note")).toHaveText("↑ +0.9% on the 7 days before");
    // Today so far against the median of the month's days read whole.
    const baseline = meter.locator(".dau-baseline");
    await expect(baseline).toContainText("Today so far, against the month's median day (6,200)");
    await expect(baseline.locator(".dau-baseline-pct")).toHaveText("34%");
    // Today so far sits beside the 24 hours, as SnapMeter's "Today UTC DAU".
    const [big, todayTile] = [(await meter.locator(".dau-big").boundingBox())!, (await tiles.nth(0).boundingBox())!];
    expect(todayTile.x).toBeGreaterThan(big.x + big.width);
    await meter.screenshot({ path: test.info().outputPath("dau-meter-1280.png") });
  });

  test("the month's days as bars on a round axis, and as a table", async ({ page }) => {
    await serve(page, payload());
    await page.goto("/dau");
    const { chart, plot, bars, hits, tip } = shown(page);
    await expect(chart.getByRole("button", { name: "30 days" })).toHaveAttribute("aria-pressed", "true");
    await expect(plot.locator(".dau-sub")).toHaveText(`Unique accounts each UTC day, the 30 days to ${short(today)}`);
    await expect(bars).toHaveCount(30);
    await expect(bars.first()).toHaveClass(/partial/);
    await expect(bars.last()).toHaveClass(/today/);
    await expect(bars.nth(12)).not.toHaveClass(/partial|today/);
    await expect(plot.getByRole("img")).toHaveAttribute("aria-label", new RegExp(`most 7,200 on ${short(today - 17)}$`));
    // One value axis, of round steps.
    await expect(plot.locator(".dau-gridline span")).toHaveText(["0", "2k", "4k", "6k", "8k"]);
    // No native tooltip: a day's count and date show by its bar on hover,
    // on keyboard focus (the arrows go from day to day), and on a tap.
    await expect(chart.locator("[title]")).toHaveCount(0);
    await expect(tip).toBeHidden();
    await hits.nth(12).hover();
    await expect(tip).toContainText("7,200 accounts");
    await expect(tip.locator(".dau-tip-day")).toHaveText(new RegExp(`^${short(today - 17)} `));
    await expect(bars.nth(12)).toHaveClass(/\bon\b/);
    // The tallest bar's readout is beside its top, on the plot, not over the
    // range buttons above it.
    await inside(tip, plot.locator(".dau-area"));
    await page.screenshot({ path: test.info().outputPath("dau-tip-1280.png") });
    // A short bar's is above it.
    await hits.last().hover();
    await expect(tip).toContainText("2,100 accounts so far");
    const [over, under] = [(await tip.boundingBox())!, (await bars.last().boundingBox())!];
    expect(over.y + over.height).toBeLessThanOrEqual(under.y);
    await page.mouse.move(2, 2);
    await expect(tip).toBeHidden();
    await plot.locator(".dau-cols").focus();
    await expect(tip).toContainText("2,100 accounts so far");
    await page.keyboard.press("ArrowLeft");
    await expect(tip.locator(".dau-tip-day")).toHaveText(new RegExp(`^${short(today - 1)} `));
    await page.keyboard.press("Home");
    await expect(tip.locator(".dau-tip-day")).toHaveText(new RegExp(`^${short(today - 29)} `));
    await page.keyboard.press("Escape");
    await expect(tip).toBeHidden();
    // A label a week, ending under today's bar.
    await expect(plot.locator(".dau-ticks span")).toHaveText([28, 21, 14, 7, 0].map((back) => short(today - back)));
    await expect(plot.locator(".dau-legend")).toHaveText("a whole UTC daytoday so far, or a day partly counted");
    await expect(plot.locator(".dau-summary")).toHaveText(`On average 6,236 a day over the 28 days read whole; the most, 7,200, on ${short(today - 17)}.`);
    // The table: newest first.
    await plot.getByText("The days as a table").click();
    const rows = plot.locator("tbody tr");
    await expect(rows).toHaveCount(30);
    await expect(rows.first().locator("th")).toContainText("(so far)");
    await expect(rows.first().locator("td").first()).toHaveText("2,100");
    await expect(rows.last().locator("th")).toContainText("(partly)");
  });

  test("the range: the past 48 hours by the hour, or the past 7 days", async ({ page }) => {
    await serve(page, payload());
    await page.goto("/dau");
    const { chart, plot, ranges, bars, hits, tip } = shown(page);
    await expect(ranges).toHaveText(["48 hours", "7 days", "30 days"]);
    const hours = chart.getByRole("button", { name: "48 hours" });
    await hours.click();
    // The button keeps focus: the buttons aren't drawn again.
    await expect(hours).toBeFocused();
    await expect(hours).toHaveAttribute("aria-pressed", "true");
    await expect(chart.getByRole("button", { name: "30 days" })).toHaveAttribute("aria-pressed", "false");
    await expect(plot.locator(".dau-sub")).toHaveText(`Unique accounts each UTC hour, the 48 hours to ${hour}`);
    // The 24 the meter counts, and the 24 before them, lighter.
    await expect(bars).toHaveCount(48);
    await expect(plot.locator(".dau-bar.before")).toHaveCount(24);
    await expect(bars.last()).not.toHaveClass(/before/);
    await expect(plot.locator(".dau-legend")).toHaveText(`the 24 hours to ${hour}the 24 before`);
    await expect(plot.getByRole("img")).toHaveAttribute("aria-label", `Active accounts each UTC hour of the 48 to ${hour}: most 1,300 in ${busiest}`);
    await expect(plot.locator(".dau-gridline span")).toHaveText(["0", "500", "1k", "1.5k"]);
    // An hour's count and its span, on the plot.
    await hits.last().hover();
    await expect(tip).toContainText(`${hourly[47].toLocaleString("en-US")} accounts`);
    await expect(tip.locator(".dau-tip-day")).toHaveText(`${short(Math.floor((to - 3_600) / DAY))}, ${hh(to - 3_600)}–${hour}`);
    await inside(tip, plot.locator(".dau-area"));
    await hits.nth(17).hover();
    await expect(tip).toContainText("1,300 accounts");
    await inside(tip, plot.locator(".dau-area"));
    // Labels on the hours' boundaries, every 6 hours, the date at midnight.
    const ticks = await plot.locator(".dau-ticks span").allTextContents();
    expect(ticks.length).toBeGreaterThanOrEqual(7);
    expect(ticks.every((t) => /^(06|12|18):00$/.test(t) || /^\d+ [A-Z][a-z]{2}$/.test(t))).toBe(true);
    await expect(plot.locator(".dau-summary")).toHaveText(`The busiest hour: 1,300 accounts, ${busiest}. An account active in several hours counts in each.`);
    await plot.getByText("The hours as a table").click();
    await expect(plot.locator("tbody tr")).toHaveCount(48);
    await expect(plot.locator("thead th")).toHaveText(["Hour (UTC)", "Active accounts"]);
    await chart.screenshot({ path: test.info().outputPath("dau-48h-1280.png") });

    // The past 7 days: a slot a day, a label a day.
    await chart.getByRole("button", { name: "7 days" }).click();
    await expect(bars).toHaveCount(7);
    await expect(plot.locator(".dau-ticks span")).toHaveText([6, 5, 4, 3, 2, 1, 0].map((back) => short(today - back)));
    await expect(plot.locator(".dau-summary")).toHaveText(`On average 6,217 a day over the 6 days read whole; the most, 6,400, on ${short(today - 5)}.`);
    await expect(plot.locator(".dau-gridline span")).toHaveText(["0", "2k", "4k", "6k", "8k"]);
    // Only one range's chart is shown.
    await expect(chart.locator(".dau-history:not([hidden])")).toHaveCount(1);
    await expect(chart.getByRole("img")).toHaveCount(1);
  });

  test("without hours counted yet, the range is 7 or 30 days", async ({ page }) => {
    await serve(page, payload({ hourly: undefined }));
    await page.goto("/dau");
    await expect(shown(page).ranges).toHaveText(["7 days", "30 days"]);
    await expect(page.locator(".dau-method dt", { hasText: "By hour" })).toHaveCount(0);
  });

  test("what accounts did, accounts by app, and how they were counted", async ({ page }) => {
    await serve(page, payload());
    await page.goto("/dau");
    // The last whole day's messages, by kind, largest first, a bar each.
    const mix = page.locator(".dau-mix");
    await expect(mix).toContainText(`45,140 messages merged on ${short(yesterday.day)}, by kind.`);
    await expect(mix.locator("li .dau-mix-name")).toHaveText(["Reactions", "Casts", "Follows", "Profile changes", "Verifications", "Username proofs"]);
    await expect(mix.locator("li").first()).toContainText("19,628");
    await expect(mix.locator("li").first()).toContainText("43%");
    await expect(mix.locator("li").nth(4)).toContainText("1.3%");
    await expect(mix.locator("li").last()).toContainText("<0.1%");
    const widths = await mix.locator(".dau-row-bar span").evaluateAll((spans) => spans.map((s) => s.getBoundingClientRect().width));
    expect(widths).toEqual([...widths].sort((a, b) => b - a));
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
    // SnapMeter's definitions, term by term.
    const method = page.locator(".dau-method");
    await expect(method.locator("dt")).toHaveText(["Active", "Past 24 hours", "Today, a day", "Past 30 days", "7-day average", "By hour", "By app", "From", "Gaps", "Exact"]);
    await expect(method).toContainText("SnapMeter's closed list for Snapchain, message types 1–8 and 11–21");
    await expect(method).toContainText("(t − 24 h, t]");
    await expect(method.locator("dd").nth(7)).toContainText("The event stream of Hypersnap's public node, read every 2 minutes by Allthing's Worker");
    await expect(method.locator("dd").nth(8)).toHaveText("None: every block since counting began has been read.");
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
    await expect(page.locator(".dau-change")).toHaveText("↓ −326 · −5.0% on the 24 hours before (6,500)");
    await expect(page.locator(".dau-change")).toHaveClass(/down/);
    await expect(page.locator(".dau-apps .dau-note")).toHaveText("Apps are known for 80% of active accounts so far: the counts below are floors while the rest are looked up.");
    await expect(page.locator(".dau-apps-foot")).toHaveText("1,234 accounts with keys whose app isn't known yet.");
    await expect(page.locator(".dau-method")).toContainText(`Not read: ${short(today - 3)}`);
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
    const meter = page.locator(".dau-meter");
    await expect(meter.locator(".dau-big")).toHaveText("—");
    await expect(meter.locator(".dau-what")).toHaveText(`The first 24-hour count comes once the 24 hours to ${short(Math.floor(first / DAY))}, ${hh(first)} UTC have been read.`);
    await expect(meter.locator(".dau-change")).toHaveCount(0);
    await expect(page.locator(".dau-behind")).toHaveCount(0);
  });

  test("while catching up, today and yesterday follow the clock, and the page says how far it has read", async ({ page }) => {
    await serve(page, catchingUp);
    await page.goto("/dau");
    const year = new Date((today - 1) * DAY * 1000).getUTCFullYear();
    await expect(page.locator(".dau-behind")).toHaveText(
      `Catching up: Allthing has read the network's messages to ${short(today - 1)} ${year}, 08:24 UTC so far, and reads them faster than they come until it is current. The counts below go to then.`,
    );
    await expect(page.getByRole("status").filter({ hasText: "Catching up:" })).toBeVisible();
    const meter = page.locator(".dau-meter");
    await expect(meter.locator(".dau-status")).toHaveText(`Catching up · ${short(today - 1)} ${year}, 08:24 UTC`);
    await expect(meter.locator(".dau-what")).toHaveText(`The first 24-hour count comes once the 24 hours to ${short(today)}, 00:00 UTC have been read.`);
    // Today hasn't been read; yesterday is being read; no whole day yet.
    const tiles = meter.locator(".dau-tile");
    await expect(tiles.locator(".dau-tile-count")).toHaveText(["—", "3,047", "—", "—"]);
    await expect(tiles.locator(".dau-tile-note")).toHaveText([
      "not read yet", `${short(today - 1)}, read to 08:24 UTC so far`, "once a whole day has been read", "once 14 days have been read whole",
    ]);
    // Nothing to measure today against yet, and no hours counted.
    await expect(meter.locator(".dau-baseline")).toHaveCount(0);
    const { plot, ranges, bars, hits, tip } = shown(page);
    await expect(ranges).toHaveText(["7 days", "30 days"]);
    // The chart ends today, not read yet; the day being read is partly
    // counted so far, not today's.
    await expect(bars).toHaveCount(2);
    await expect(bars.last()).toHaveClass(/partial/);
    await expect(bars.last()).not.toHaveClass(/today/);
    await hits.last().hover();
    await expect(tip).toContainText("3,047 accounts (read so far)");
    await hits.first().hover();
    await expect(tip).toContainText("134 accounts (partly counted)");
    const [area, last] = [(await plot.locator(".dau-cols").boundingBox())!, (await bars.last().boundingBox())!];
    // Yesterday's bar sits one slot in from the right, today's slot empty.
    expect(area.x + area.width - (last.x + last.width)).toBeGreaterThan(area.width / 30);
    // The last label names that bar, under it; today's empty slot has none.
    const label = plot.locator(".dau-ticks span").last();
    await expect(label).toHaveText(short(today - 1));
    await expect(plot.locator(".dau-ticks span", { hasText: short(today) })).toHaveCount(0);
    const tick = (await label.boundingBox())!;
    expect(Math.abs(tick.x + tick.width / 2 - (last.x + last.width / 2))).toBeLessThan(1);
    await expect(plot.locator(".dau-summary")).toHaveText("No day here has been read whole yet.");
    await plot.getByText("The days as a table").click();
    await expect(plot.locator("tbody tr").first().locator("th")).toContainText("(read so far)");
    // What they did: the day being read, said so, not the 31 minutes before it.
    await expect(page.locator(".dau-mix")).toContainText(`5,740 messages merged on ${short(today - 1)}, read to 08:24 UTC so far, by kind.`);
  });

  test("a short history keeps a month's room, the newest day on the right", async ({ page }) => {
    await serve(page, payload({ days: days.slice(-3) }));
    await page.goto("/dau");
    const { plot, bars } = shown(page);
    await expect(bars).toHaveCount(3);
    const [area, last] = [(await plot.locator(".dau-cols").boundingBox())!, (await bars.last().boundingBox())!];
    expect(area.x + area.width - (last.x + last.width)).toBeLessThan(area.width / 30);
    expect(last.width).toBeLessThan(area.width / 25);
  });

  test("a client card links to the page, which the app opens without a reload", async ({ page }) => {
    await serve(page, payload());
    const hash = (i: number) => `0x${(i + 1).toString(16).padStart(40, "0")}`;
    // `serve` pins this browser to noon. Process time can be later, which
    // would make every row future-dated and correctly remove the app legend.
    const pool = Array.from({ length: 25 }, (_, i) => ({ hash: hash(i), author: { fid: 1, username: "fixture" }, text: `DAU link fixture ${i}`, ts: now - 19 - i }));
    const origins = Object.fromEntries(pool.map((c) => [c.hash, { signer: "0x01", app_fid: 9152 }]));
    await page.route("**/api/v1/home**", (route) => route.fulfill({
      json: { pool, notes: { origins, apps: { "9152": { fid: 9152, username: "warpcast", display_name: "Warpcast" } } }, meta: payload().meta, apps_24h: payload().apps_24h },
    }));
    // Its count is a link to the page, and so is how it's counted.
    for (const link of ["4,812 active accounts, on the page of every app's active accounts", "How it's counted"]) {
      await page.goto("/");
      // Gone if the page reloads. (The pinned clock fakes `performance`, so
      // its navigation entries can't say.)
      await page.evaluate(() => Object.assign(window, { sameDocument: true }));
      await page.locator(".client-legend").getByRole("link", { name: /Farcaster/ }).hover();
      const card = page.getByRole("dialog", { name: "Farcaster: active accounts" });
      await card.getByRole("link", { name: link }).click();
      await expect(page).toHaveURL(/\/dau$/);
      await expect(page.locator(".dau-meter")).toContainText("6,174");
      expect(await page.evaluate(() => "sameDocument" in window)).toBe(true);
    }
  });

  test("at 390, everything fits the screen", async ({ browser }, testInfo) => {
    const { context, page } = await phone(browser, testInfo);
    await serve(page, payload());
    await page.goto("/dau");
    await expect(page.locator(".dau-meter")).toContainText("6,174");
    for (const section of [".dau-meter", ".dau-chart", ".dau-mix", ".dau-apps", ".dau-method"]) {
      const box = (await page.locator(section).boundingBox())!;
      expect(box.x, section).toBeGreaterThanOrEqual(0);
      expect(box.x + box.width, section).toBeLessThanOrEqual(390);
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    // The meter keeps SnapMeter's shape: today beside the 24 hours, and the
    // readouts side by side.
    const meter = page.locator(".dau-meter");
    const tiles = meter.locator(".dau-tile");
    const [big, todayTile] = [(await meter.locator(".dau-big").boundingBox())!, (await tiles.nth(0).boundingBox())!];
    expect(todayTile.x).toBeGreaterThan(big.x + big.width);
    const [first, third] = [(await tiles.nth(1).boundingBox())!, (await tiles.nth(3).boundingBox())!];
    expect(Math.abs(first.y - third.y)).toBeLessThan(1);
    for (let i = 0; i < 4; i++) await inside(tiles.nth(i), meter);
    await meter.screenshot({ path: testInfo.outputPath("dau-meter-390.png") });
    // A tapped day's count and date, on the plot: the tallest's beside it,
    // not over the range buttons.
    const { chart, plot, hits, tip } = shown(page);
    await hits.nth(12).tap();
    await expect(tip).toContainText("7,200 accounts");
    await inside(tip, plot.locator(".dau-area"));
    await inside(tip, chart);
    const [over, buttons] = [(await tip.boundingBox())!, (await chart.getByRole("group", { name: "Range" }).boundingBox())!];
    expect(over.y).toBeGreaterThan(buttons.y + buttons.height);
    // Focused by the tap, the chart draws no ring.
    await expect(plot.locator(".dau-cols")).toHaveCSS("outline-style", "none");
    await chart.scrollIntoViewIfNeeded();
    await page.screenshot({ path: testInfo.outputPath("dau-tip-month-390.png") });
    // The past 48 hours: an hour a tap, the labels 12 hours apart.
    await chart.getByRole("button", { name: "48 hours" }).tap();
    await expect(plot.locator(".dau-bar")).toHaveCount(48);
    await hits.nth(17).tap();
    await expect(tip).toContainText("1,300 accounts");
    await inside(tip, chart);
    for (const label of await plot.locator(".dau-ticks span:visible").all()) await inside(label, chart);
    const labels = await plot.locator(".dau-ticks span:visible").allTextContents();
    expect(labels.every((t) => t === "12:00" || /^\d+ [A-Z][a-z]{2}$/.test(t))).toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    await chart.screenshot({ path: testInfo.outputPath("dau-48h-390.png") });
    await context.close();
  });

  test("at 390, while catching up, the chart's label names the bar above it, on the card", async ({ browser }, testInfo) => {
    const { context, page } = await phone(browser, testInfo);
    await serve(page, catchingUp);
    await page.goto("/dau");
    const { chart, plot, bars, hits, tip } = shown(page);
    const last = bars.last();
    // A tap shows the day's count and date by its bar, on the card.
    await hits.last().tap();
    await expect(tip).toContainText("3,047 accounts (read so far)");
    await expect(tip.locator(".dau-tip-day")).toHaveText(new RegExp(`^${short(today - 1)} `));
    await inside(tip, chart);
    await page.screenshot({ path: testInfo.outputPath("dau-tip-390.png") });
    const label = plot.locator(".dau-ticks span").last();
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
