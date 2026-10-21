// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";
import { appCame } from "./helpers/app";
import { signIn } from "./helpers/farcaster";

/*
 * Cast on the reader's own profile: the feeds' pill, in Allthing's accent,
 * at the profile header's top right, where Favorite and Show less are on
 * anyone else's; on a phone, in the row of its own under the picture and
 * name where they sit, as no round button floats over a profile. It opens
 * the New cast composer for a cast of the reader's own, and closing that
 * gives focus back to it. It isn't there signed out or on anyone else's
 * profile, and the static copy has no composer.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in or composer");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const bob = { fid: 2, username: "bob", display_name: "Bob" };
const mine = { hash: hex(1), author: me, text: "My own cast", ts, likes: 2, recasts: 0, replies: 0 };
const theirs = { hash: hex(2), author: bob, text: "Bob's cast", ts: ts - 60, likes: 1, recasts: 0, replies: 0 };
// What the feed says of its authors, for a profile's header before its payload.
const users = {
  [me.fid]: { bio: "Reads everything.", followers: 12, following: 3 },
  [bob.fid]: { bio: "Writes about books.", followers: 10, following: 5 },
};
const ACCENT = "rgb(79, 67, 212)";

/**
 * The pages, with the Worker endorsing no keys, as on allthing.social.
 * With `hold`, a profile's payload waits for `release`.
 */
async function serve(page: Page, { hold = false } = {}) {
  const pending: (() => void)[] = [];
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [mine, theirs], notes: { users }, meta } });
  });
  await page.route("**/api/v1/user?*", (route) => {
    const name = new URL(route.request().url()).searchParams.get("name");
    const [who, cast] = name === bob.username ? [bob, theirs] : [me, mine];
    const card = users[who.fid];
    const user = { ...who, bio: card.bio, follower_count: card.followers, following_count: card.following };
    const send = () => void route.fulfill({ json: { user, pool: [cast], notes: {}, meta } });
    if (hold) pending.push(send);
    else send();
  });
  await page.route("**/api/v1/signer", (route) => route.fulfill({ status: 404, json: { error: "Liking and recasting from Allthing isn't set up on this server" } }));
  // Signed in, the browser reads the node: the reader's likes and recasts
  // (none), and whether they follow the profile's account (no).
  const cors = { "access-control-allow-origin": "*" };
  await page.route("https://haatz.quilibrium.com/v1/**", (route) =>
    new URL(route.request().url()).pathname.endsWith("/reactionsByFid")
      ? route.fulfill({ headers: cors, json: { messages: [] } })
      : route.fulfill({ status: 400, headers: cors, json: { error: "Failed to get it", error_detail: 'status: NotFound, message: "not found"' } }),
  );
  return { waiting: () => pending.length, release: () => pending.splice(0).forEach((send) => send()) };
}

/** A page `width` wide, a phone's at 390, signed in as the reader or not. */
async function open(browser: Browser, testInfo: TestInfo, width: number, { signedIn = true } = {}) {
  const phone = width < 641;
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL,
    viewport: { width, height: phone ? 844 : 900 },
    isMobile: phone,
    hasTouch: phone,
  });
  const page = await context.newPage();
  if (signedIn) await signIn(page, me);
  return { context, page };
}

/** The profile's header, once its own payload is in: its casts show. */
async function profile(page: Page, path: string, text: string) {
  await page.goto(path);
  await expect(page.locator(".cast-card .cast-text")).toHaveText(text);
  return page.locator("section.profile");
}

/** How a pill is drawn: its colour and sheen, lettering and shape. */
const look = (pill: Locator) =>
  pill.evaluate((el) => {
    const style = getComputedStyle(el);
    return {
      tag: el.tagName,
      type: el.getAttribute("type"),
      fill: style.backgroundColor,
      sheen: style.backgroundImage,
      ink: style.color,
      font: style.fontFamily,
      size: style.fontSize,
      radius: style.borderTopLeftRadius,
      padding: style.padding,
    };
  });

type Box = { x: number; y: number; width: number; height: number };
const overlaps = (a: Box, b: Box) => a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

test("at 1280, Cast is the feed's pill at the top right of the reader's own profile, and opens a new cast", async ({ browser }, testInfo) => {
  const { context, page } = await open(browser, testInfo, 1280);
  await serve(page);
  // The feed's pill, to compare with.
  await page.goto("/");
  await appCame(page);
  const feeds = await look(page.locator(".listing-head").getByRole("button", { name: "Write a cast" }));

  const card = await profile(page, "/u/reader", "My own cast");
  const name = card.getByRole("heading", { level: 1, name: "Reader" });
  const pill = card.getByRole("button", { name: "Write a cast" });
  await expect(pill).toBeVisible();
  await expect(pill).toHaveText("Cast");
  // The page's one Cast, where Favorite and Show less are on anyone else's.
  await expect(page.getByRole("button", { name: "Write a cast" })).toHaveCount(1);
  await expect(card.getByRole("button", { name: /^(Favorite|Show less)$/ })).toHaveCount(0);

  // At the header's top right, level with the name.
  const [p, c, n] = [(await pill.boundingBox())!, (await card.boundingBox())!, (await name.boundingBox())!];
  const fromRight = c.x + c.width - (p.x + p.width);
  expect(fromRight).toBeGreaterThan(0);
  expect(fromRight).toBeLessThan(40);
  expect(p.y - c.y).toBeGreaterThan(0);
  expect(p.y - c.y).toBeLessThan(40);
  expect(p.x).toBeGreaterThan(c.x + c.width / 2);
  expect(p.x).toBeGreaterThan(n.x + n.width);
  expect(Math.abs(p.y + p.height / 2 - (n.y + n.height / 2))).toBeLessThan(8);

  // The feed's pill: Allthing's accent, white lettering in its serif.
  const drawn = await look(pill);
  expect(drawn).toEqual(feeds);
  expect([drawn.tag, drawn.type]).toEqual(["BUTTON", "button"]);
  expect(drawn.fill).toBe(ACCENT);
  expect(drawn.sheen).toMatch(/^linear-gradient\(115deg, rgb\(79, 67, 212\) 0%, rgb\(79, 67, 212\) 38%/);
  expect(drawn.ink).toBe("rgb(255, 255, 255)");
  expect(drawn.font).toContain("Instrument Serif");
  expect(parseFloat(drawn.radius)).toBeGreaterThanOrEqual(p.height / 2);
  await card.screenshot({ path: testInfo.outputPath("castpill-1280.png"), animations: "disabled" });

  // Pointed at, it rings in the accent.
  const shadow = () => pill.evaluate((el) => getComputedStyle(el).boxShadow);
  expect(await shadow()).not.toContain("0px 0px 0px 4px");
  await pill.hover();
  await expect.poll(shadow).toContain("rgba(79, 67, 212, 0.1) 0px 0px 0px 4px");

  // A click opens a new cast, of the reader's own: in no channel, answering
  // nothing. Its text box has the focus.
  await pill.click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("textbox", { name: "What's happening?" })).toBeFocused();
  await expect(dialog.locator(".compose-context")).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("castpill-1280-composer.png"), animations: "disabled" });
  // Escape closes it, and focus is back on Cast, with its ring.
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(pill).toBeFocused();
  const ring = await pill.evaluate((el) => [el.matches(":focus-visible"), getComputedStyle(el).outlineStyle, getComputedStyle(el).outlineColor]);
  expect(ring).toEqual([true, "solid", ACCENT]);
  // The keyboard opens it too: Enter, then Space.
  for (const key of ["Enter", "Space"]) {
    await page.keyboard.press(key);
    await expect(dialog.getByRole("textbox", { name: "What's happening?" })).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    await expect(pill).toBeFocused();
  }
  await context.close();
});

test("at 390, Cast has the row under the picture and name, inside the screen, and opens a new cast", async ({ browser }, testInfo) => {
  const { context, page } = await open(browser, testInfo, 390);
  await serve(page);
  const card = await profile(page, "/u/reader", "My own cast");
  const pill = card.getByRole("button", { name: "Write a cast" });
  await expect(pill).toBeInViewport({ ratio: 1 });
  await expect(pill).toHaveText("Cast");
  // No round button floats over a profile: this is the page's one Cast.
  await expect(page.getByRole("button", { name: "Write a cast" })).toHaveCount(1);
  await expect(page.locator(".cast-fab")).toHaveCount(0);

  // Under the picture and name, clear of both, where Favorite and Show less
  // sit on anyone else's: from the picture's left edge, inside the card.
  const box = (target: Locator) => target.boundingBox().then((b) => b!);
  const [p, c, a, n, handle] = await Promise.all([
    box(pill),
    box(card),
    box(card.locator(".avatar")),
    box(card.getByRole("heading", { level: 1, name: "Reader" })),
    box(card.locator(".handle")),
  ]);
  expect(p.y).toBeGreaterThanOrEqual(a.y + a.height);
  expect(p.y).toBeGreaterThanOrEqual(handle.y + handle.height);
  for (const other of [a, n, handle]) expect(overlaps(p, other)).toBe(false);
  expect(Math.abs(p.x - a.x)).toBeLessThan(1);
  expect(p.x + p.width).toBeLessThanOrEqual(c.x + c.width);
  expect(p.x + p.width).toBeLessThanOrEqual(390);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  // A fingertip's tap: a band 44 px tall, as wide as the pill.
  const band = await pill.evaluate((el) => {
    const after = getComputedStyle(el, "::after");
    return { width: parseFloat(after.width), height: parseFloat(after.height) };
  });
  expect(band.height).toBeGreaterThanOrEqual(44);
  expect(Math.abs(band.width - p.width)).toBeLessThan(1);
  const drawn = await look(pill);
  expect(drawn.fill).toBe(ACCENT);
  expect(drawn.ink).toBe("rgb(255, 255, 255)");
  await card.screenshot({ path: testInfo.outputPath("castpill-390.png"), animations: "disabled" });

  // A tap opens a new cast, on the screen, with the text box focused.
  await pill.tap();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await expect(dialog).toBeInViewport({ ratio: 1 });
  await expect(dialog.getByRole("textbox", { name: "What's happening?" })).toBeFocused();
  await expect(dialog.locator(".compose-context")).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("castpill-390-composer.png"), animations: "disabled" });
  // Escape closes it, and focus is back on Cast.
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(pill).toBeFocused();
  await context.close();
});

for (const width of [1280, 390]) {
  test(`at ${width}, there is no Cast on anyone else's profile, nor signed out: Favorite and Show less are there`, async ({ browser }, testInfo) => {
    // Signed in, on bob's profile.
    const signed = await open(browser, testInfo, width);
    await serve(signed.page);
    const theirCard = await profile(signed.page, "/u/bob", "Bob's cast");
    await expect(theirCard.getByRole("button", { name: "Favorite" })).toBeVisible();
    await expect(theirCard.getByRole("button", { name: "Show less" })).toBeVisible();
    await expect(signed.page.getByRole("button", { name: "Write a cast" })).toHaveCount(0);
    await expect(signed.page.locator(".cast-pill")).toHaveCount(0);
    await signed.context.close();

    // Signed out, the reader's profile is anyone's.
    const out = await open(browser, testInfo, width, { signedIn: false });
    await serve(out.page);
    const card = await profile(out.page, "/u/reader", "My own cast");
    await expect(card.getByRole("button", { name: "Favorite" })).toBeVisible();
    await expect(card.getByRole("button", { name: "Show less" })).toBeVisible();
    await expect(out.page.getByRole("button", { name: "Write a cast" })).toHaveCount(0);
    await expect(out.page.locator(".cast-pill")).toHaveCount(0);
    await out.context.close();
  });
}

test("Cast comes with the header, from the feed's preview to the profile and on a visit, and nothing moves", async ({ browser }, testInfo) => {
  const { context, page } = await open(browser, testInfo, 1280);
  // Whether each profile header was drawn with Cast in it.
  await page.addInitScript(() => {
    const drawn: boolean[] = [];
    Object.assign(window, { drawn });
    new MutationObserver((records) => {
      for (const record of records) {
        for (const node of record.addedNodes) {
          if (!(node instanceof Element)) continue;
          const headers = node.matches("section.profile") ? [node] : [...node.querySelectorAll("section.profile")];
          for (const header of headers) drawn.push(header.querySelector(".cast-pill") !== null);
        }
      }
    }).observe(document, { childList: true, subtree: true });
  });
  const drawn = () => page.evaluate(() => (window as unknown as { drawn: boolean[] }).drawn);
  // Where an element is on the page, wherever it's scrolled, once the
  // page's entrance (it rises 6 px as it fades in) has ended.
  const place = (target: Locator) =>
    target.evaluate(async (el) => {
      const moving: Animation[] = [];
      for (let node: Element | null = el; node; node = node.parentElement) moving.push(...node.getAnimations());
      const ending = moving.filter((a) => a.effect?.getComputedTiming().endTime !== Infinity);
      await Promise.all(ending.map((a) => a.finished.catch(() => null)));
      const r = el.getBoundingClientRect();
      return [r.x + scrollX, r.y + scrollY, r.width, r.height];
    });
  const api = await serve(page, { hold: true });
  await page.goto("/");
  await appCame(page);
  // The reader's name on their cast opens their profile: its header at once,
  // from the feed, while its payload waits.
  await page.locator(`.cast-card header.cast-meta > .user-chip > a.author[href="/u/${me.username}"]`).dispatchEvent("click");
  await expect(page).toHaveURL(/\/u\/reader$/);
  const header = page.locator("section.profile");
  const name = header.locator(".profile-name");
  const pill = header.getByRole("button", { name: "Write a cast" });
  await expect(name).toHaveText("Reader");
  await expect(page.locator(".cast-list[aria-busy=true]")).toBeVisible();
  await expect(pill).toBeVisible();
  const [pillAt, nameAt] = [await place(pill), await place(name)];
  // Then the profile's own: the same Cast, in the same place.
  await expect.poll(api.waiting).toBe(1);
  api.release();
  await expect(page.locator(".cast-card .cast-text")).toHaveText("My own cast");
  expect(await place(pill)).toEqual(pillAt);
  expect(await place(name)).toEqual(nameAt);
  // Every header drawn had it from the start.
  const fromFeed = await drawn();
  expect(fromFeed.length).toBeGreaterThanOrEqual(1);
  expect(fromFeed).not.toContain(false);

  // Visited directly, the header comes with its payload, Cast and all.
  await page.reload();
  await expect.poll(api.waiting).toBe(1);
  expect(await drawn()).toEqual([]);
  api.release();
  await expect(page.locator(".cast-card .cast-text")).toHaveText("My own cast");
  expect(await place(pill)).toEqual(pillAt);
  const visited = await drawn();
  expect(visited.length).toBeGreaterThanOrEqual(1);
  expect(visited).not.toContain(false);
  await context.close();
});
