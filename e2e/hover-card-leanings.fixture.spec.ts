// SPDX-License-Identifier: AGPL-3.0-or-later
import axe from "axe-core";
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";
import { appCame } from "./helpers/app";
import { signIn } from "./helpers/farcaster";

/*
 * Favorite and Show less on a profile card, as on the profile: on anyone's
 * card but the reader's own, toggles (aria-pressed) that are one or the
 * other, kept in this browser with the profile's, and said to screen
 * readers. A feed on screen keeps its order while the card is over it, and
 * takes the new one when it is next loaded. At 1280 a card opens on hover,
 * at 390 on a tap.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = Math.floor(Date.now() / 1000) - 3600;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const bob = { fid: 43, username: "bob", display_name: "Bob" };
const carol = { fid: 44, username: "carol", display_name: "Carol" };
const users = {
  "42": { bio: "Reads conversations.", followers: 120, following: 7 },
  "43": { bio: "Replies a lot.", followers: 12, following: 3 },
  "44": { bio: "Replies to replies.", followers: 5, following: 1 },
};
// Hot: alice's first, then bob's, then carol's.
const casts = [
  { hash: hex(1), author: alice, text: "Alice's cast, the most liked", ts, likes: 30, recasts: 0, replies: 0 },
  { hash: hex(2), author: bob, text: "Bob's cast", ts, likes: 20, recasts: 0, replies: 0 },
  { hash: hex(3), author: carol, text: "Carol's cast", ts, likes: 10, recasts: 0, replies: 0 },
];

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : casts, notes: { users }, meta } });
  });
  await page.route("**/api/v1/vote?*", (route) => {
    const fid = Number(new URL(route.request().url()).searchParams.get("fid"));
    return route.fulfill({ json: { fid, onchain: "stay", app: "opted_in", meta } });
  });
  await page.route("**/api/v1/wallets?*", (route) => {
    const fid = Number(new URL(route.request().url()).searchParams.get("fid"));
    return route.fulfill({ json: { fid, addresses: [], chains: [], total: 0, sources: [], fetched_at: Date.now() } });
  });
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: {
    user: { ...alice, follower_count: 120, following_count: 7 }, pool: [casts[0]], notes: { users }, meta,
  } }));
}

async function at(browser: Browser, testInfo: TestInfo, phone: boolean) {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL,
    viewport: phone ? { width: 390, height: 844 } : { width: 1280, height: 900 },
    isMobile: phone,
    hasTouch: phone,
  });
  return { context, page: await context.newPage() };
}

const card = (page: Page, who: { display_name: string; username: string }) =>
  page.getByRole("dialog", { name: `Profile: ${who.display_name} (@${who.username})`, exact: true });

/** Open `who`'s card from their cast's name: a hover with a mouse, a tap on a phone. */
async function open(page: Page, who: typeof alice, phone: boolean) {
  const name = page.locator(`#app .cast-card .user-chip > .display-name`, { hasText: who.display_name }).first();
  if (phone) await name.tap();
  else await name.hover();
  const panel = card(page, who);
  await expect(panel).toBeVisible();
  return panel;
}

const order = (page: Page) => page.locator("#app .cast-card:not(.skeleton)").evaluateAll((all) => all.map((c) => c.getAttribute("data-hash")));
const kept = (page: Page) =>
  page.evaluate(() => ({
    favorites: JSON.parse(localStorage.getItem("allthing.favorites") ?? "[]").map((a: { fid: number }) => a.fid),
    less: JSON.parse(localStorage.getItem("allthing.show-less") ?? "[]").map((a: { fid: number }) => a.fid),
  }));

/** A press: a tap on a phone, a click with a mouse. */
const press = (button: Locator, phone: boolean) => (phone ? button.tap() : button.click());

async function pressed(panel: Locator, favorite: boolean, less: boolean) {
  await expect(panel.getByRole("button", { name: "Favorite" })).toHaveAttribute("aria-pressed", String(favorite));
  await expect(panel.getByRole("button", { name: "Show less" })).toHaveAttribute("aria-pressed", String(less));
}

/** How much room the card's row of buttons has to spare, on one line, and its height. */
const fit = (panel: Locator) =>
  panel.locator(".profile-card-leaning").evaluate((row) => {
    const items = [...row.children].filter((item) => item.getBoundingClientRect().width > 1);
    const gap = parseFloat(getComputedStyle(row).columnGap) || 0;
    const used = items.reduce((width, item) => width + item.getBoundingClientRect().width, 0) + gap * (items.length - 1);
    return { spare: row.clientWidth - used, height: Math.round(row.getBoundingClientRect().height) };
  });

/** Targets in the card whose tap areas (each box, and its ::after band) meet one of Favorite's or Show less's. */
const meeting = (panel: Locator) =>
  panel.evaluate((panel) => {
    type Box = { x: number; y: number; right: number; bottom: number };
    const areas = (el: Element): Box[] => {
      const r = el.getBoundingClientRect();
      const boxes = [{ x: r.left, y: r.top, right: r.right, bottom: r.bottom }];
      const band = getComputedStyle(el, "::after");
      if (band.content !== "none" && band.position === "absolute") {
        const m = band.transform && band.transform !== "none" ? new DOMMatrixReadOnly(band.transform) : new DOMMatrixReadOnly();
        const left = r.left + el.clientLeft + parseFloat(band.left) + m.e;
        const top = r.top + el.clientTop + parseFloat(band.top) + m.f;
        boxes.push({ x: left, y: top, right: left + parseFloat(band.width), bottom: top + parseFloat(band.height) });
      }
      return boxes;
    };
    const mine = [...panel.querySelectorAll(".profile-card-leaning button")];
    const others = [...panel.querySelectorAll("a[href], button")].filter((el) => !mine.includes(el) && el.getClientRects().length > 0);
    const met: string[] = [];
    for (const a of mine) {
      for (const b of [...others, ...mine.filter((m) => m !== a)]) {
        for (const A of areas(a)) {
          for (const B of areas(b)) {
            const [w, h] = [Math.min(A.right, B.right) - Math.max(A.x, B.x), Math.min(A.bottom, B.bottom) - Math.max(A.y, B.y)];
            if (w > 0.5 && h > 0.5) met.push(`${a.className} meets ${b.tagName.toLowerCase()}.${b.className} (${w.toFixed(1)} × ${h.toFixed(1)})`);
          }
        }
      }
    }
    return [...new Set(met)];
  });

/** Accessibility problems axe finds in the open card. */
async function axeViolations(page: Page, scope: string) {
  await page.evaluate(() =>
    Promise.all(document.getAnimations().filter((a) => a.effect?.getComputedTiming().endTime !== Infinity).map((a) => a.finished.catch(() => undefined))),
  );
  await page.evaluate(axe.source);
  return page.evaluate(async (scope) => {
    // @ts-expect-error axe is the injected script's global
    const result = await window.axe.run(scope, { resultTypes: ["violations"] });
    return result.violations.map((v: { id: string; nodes: { target: string[] }[] }) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`);
  }, scope);
}

for (const phone of [false, true]) {
  const width = phone ? 390 : 1280;

  test(`at ${width}, a card's Favorite and Show less are toggles, one or the other, kept with the profile's, and the feed keeps its order`, async ({ browser }, testInfo) => {
    const { context, page } = await at(browser, testInfo, phone);
    await serve(page);
    await page.goto("/");
    await appCame(page);
    const before = await order(page);
    expect(before).toEqual(casts.map((c) => c.hash));

    const panel = await open(page, alice, phone);
    await pressed(panel, false, false);
    const live = panel.locator('[aria-live="polite"]');
    // One row 28 px tall, with room to spare (a font a little wider can't
    // wrap it), and taps that meet no other target's.
    const row = await fit(panel);
    expect(row.height).toBe(28);
    expect(row.spare).toBeGreaterThanOrEqual(8);
    expect(await meeting(panel)).toEqual([]);

    await press(panel.getByRole("button", { name: "Favorite" }), phone);
    await pressed(panel, true, false);
    await expect(live).toHaveText("@alice's casts rise in your feeds.");
    expect(await kept(page)).toEqual({ favorites: [42], less: [] });
    // Her casts carry the star at once; nothing moves.
    await expect(page.locator(`#app .cast-card[data-hash="${casts[0].hash}"] .favorite-mark`)).toBeVisible();

    await press(panel.getByRole("button", { name: "Show less" }), phone);
    await pressed(panel, false, true);
    await expect(live).toHaveText("@alice's casts sink in your feeds.");
    expect(await kept(page)).toEqual({ favorites: [], less: [42] });
    await page.screenshot({ path: testInfo.outputPath(`hover-card-leanings-${width}.png`) });

    // The card is still open over the same cast, and the feed's order is as it was.
    await expect(panel).toBeVisible();
    expect(await order(page)).toEqual(before);
    await page.keyboard.press("Escape");
    await expect(panel).toBeHidden();
    expect(await order(page)).toEqual(before);

    // The profile shows the same.
    await page.goto("/u/alice");
    await expect(page.locator(".profile").getByRole("button", { name: "Show less" })).toHaveAttribute("aria-pressed", "true");
    await expect(page.locator(".profile").getByRole("button", { name: "Favorite" })).toHaveAttribute("aria-pressed", "false");

    // The next load of the feed takes the new order: alice's cast sinks.
    await page.goto("/");
    await appCame(page);
    await expect.poll(() => order(page)).toEqual([casts[1].hash, casts[2].hash, casts[0].hash]);

    // And pressing again takes it back.
    const again = await open(page, alice, phone);
    await pressed(again, false, true);
    await press(again.getByRole("button", { name: "Show less" }), phone);
    await pressed(again, false, false);
    await expect(again.locator('[aria-live="polite"]')).toHaveText("@alice's casts are back in place in your feeds.");
    expect(await kept(page)).toEqual({ favorites: [], less: [] });
    await context.close();
  });

  test(`at ${width}, the reader's own card has neither, and axe finds nothing on another's`, async ({ browser }, testInfo) => {
    const { context, page } = await at(browser, testInfo, phone);
    await signIn(page, alice);
    await serve(page);
    await page.goto("/");
    await appCame(page);
    const own = await open(page, alice, phone);
    await expect(own.getByRole("button", { name: "Favorite" })).toHaveCount(0);
    await expect(own.getByRole("button", { name: "Show less" })).toHaveCount(0);
    await page.keyboard.press("Escape");
    await expect(own).toBeHidden();
    if (!phone) await page.mouse.move(2, 2);
    const theirs = await open(page, bob, phone);
    await pressed(theirs, false, false);
    expect(await axeViolations(page, ".popover-panel.profile-card")).toEqual([]);
    await context.close();
  });
}

test("with the keyboard, Tab reaches Favorite and Show less in a card, and Enter and Space press them", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  await appCame(page);
  // Keyboard focus resting on a name opens its card.
  await page.locator("#app .cast-card .user-chip > .display-name", { hasText: "Bob" }).first().focus();
  const panel = card(page, bob);
  await expect(panel).toBeVisible();
  const favorite = panel.getByRole("button", { name: "Favorite" });
  const less = panel.getByRole("button", { name: "Show less" });
  for (let i = 0; i < 6 && !(await favorite.evaluate((el) => el === document.activeElement)); i++) await page.keyboard.press("Tab");
  await expect(favorite).toBeFocused();
  await page.keyboard.press("Enter");
  await pressed(panel, true, false);
  await page.keyboard.press("Tab");
  await expect(less).toBeFocused();
  await page.keyboard.press("Space");
  await pressed(panel, false, true);
  expect(await kept(page)).toEqual({ favorites: [], less: [43] });
});
