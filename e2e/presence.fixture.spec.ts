// SPDX-License-Identifier: AGPL-3.0-or-later
import axe from "axe-core";
import { expect, test, type Browser, type BrowserContext, type Page, type TestInfo } from "@playwright/test";

/*
 * Who is reading Allthing now: the header's small "● 12"
 * (components/presence.rs). The browser beats (POST /api/v1/presence) once
 * the page's own call is in, never as part of it; only a visible tab in
 * use beats, and one tab beats for the whole browser. The chip, a dot and
 * a count ("● 12", named "12 readers online"), fades in after Home without
 * moving anything; Home stays as it is, including on a phone. Its popover names the
 * readers who show themselves and only counts the others. The Worker's
 * side is presence-worker.fixture.spec.ts.
 */

const PAGES = Boolean(process.env.E2E_PAGES);
/** Where the screenshots go, if anywhere besides the test's output. */
const SHOTS = process.env.PRESENCE_SHOTS;

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = Math.floor(Date.now() / 1000) - 3_600;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const person = (fid: number, username: string) => ({
  fid,
  username,
  display_name: username[0].toUpperCase() + username.slice(1),
  pfp_url: `https://imagedelivery.net/fixture/pfp-${username}/original`,
});
const [ann, bob, cat] = [person(1, "ann"), person(2, "bob"), person(3, "cat")];
const me = person(1234, "reader");
const pool = Array.from({ length: 8 }, (_, i) => ({ hash: hex(100 + i), author: [ann, bob, cat][i % 3], text: `Cast number ${i}`, ts: ts - i * 60, likes: 8 - i, recasts: 0, replies: 0 }));

type Answer = { online: number; readers: object[]; next: number };
type Beat = { at: number; method: string; body: { id: string; hidden: boolean; refresh: boolean } };

const svg = (fill: string) => `<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><rect width="64" height="64" fill="${fill}"/></svg>`;

/**
 * The fixture's endpoints, for a page or a whole browser (its tabs): the
 * front page, pictures, and the beats, each answered by `answer` (its
 * number, from 1) once `gate` lets it. Every other API path is a 404.
 */
async function serve(target: Page | BrowserContext, answer: (n: number) => { status?: number; json: object }, gate: Promise<void> = Promise.resolve()) {
  const beats: Beat[] = [];
  const served = { home: 0 };
  await target.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await target.route("**/api/v1/home**", async (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    await route.fulfill({ json: { pool: later ? [] : pool, notes: {}, meta } });
    served.home ||= Date.now();
  });
  await target.route("https://imagedelivery.net/**", (route) =>
    route.fulfill({ body: svg(route.request().url().includes("reader") ? "#0a8fa7" : "#4f43d4"), contentType: "image/svg+xml", headers: { "access-control-allow-origin": "*" } }),
  );
  await target.route("**/api/v1/presence", async (route) => {
    const request = route.request();
    beats.push({ at: Date.now(), method: request.method(), body: request.postDataJSON() });
    const n = beats.length;
    await gate;
    const { status = 200, json } = answer(n);
    await route.fulfill({ status, json, headers: { "cache-control": "no-store" } });
  });
  return { beats, served };
}

/** Signed in, as the Worker's readable user cookie says (helpers/farcaster.ts's `signIn`), in a page or a whole browser. */
const signIn = (target: Page | BrowserContext, user: object) =>
  target.addInitScript((user) => {
    document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax`;
  }, user);

const ok = (online: number, readers: object[] = [], next = 240) => ({ json: { online, readers, next } satisfies Answer });

async function phone(browser: Browser, info: TestInfo) {
  const context = await browser.newContext({ baseURL: info.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true, deviceScaleFactor: 2 });
  return { context, page: await context.newPage() };
}

/** A page at `width`: a phone's at 390. */
async function opened(browser: Browser, page: Page, info: TestInfo, width: number) {
  if (width > 400) {
    await page.setViewportSize({ width, height: 900 });
    return { page, close: async () => {} };
  }
  const { context, page: onPhone } = await phone(browser, info);
  return { page: onPhone, close: () => context.close() };
}

/** Layout shifts from here on, as the page reports them. */
const watchShifts = (page: Page) =>
  page.addInitScript(() => {
    const shifts: { value: number; at: number }[] = [];
    (window as unknown as { __shifts: typeof shifts }).__shifts = shifts;
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries() as (PerformanceEntry & { value: number; hadRecentInput: boolean })[]) {
        if (!entry.hadRecentInput) shifts.push({ value: entry.value, at: entry.startTime });
      }
    }).observe({ type: "layout-shift", buffered: true });
  });
const shifts = (page: Page) => page.evaluate(() => (window as unknown as { __shifts: { value: number }[] }).__shifts.splice(0));

/** The header's parts and the first card, as laid out. */
const boxes = (page: Page) =>
  page.evaluate(() => {
    const box = (selector: string) => {
      const el = document.querySelector(selector);
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return [r.x, r.y, r.width, r.height].map((n) => Math.round(n * 10) / 10);
    };
    return {
      wordmark: box("#app .wordmark"),
      home: box("#app .main-nav a"),
      search: box("#app .search-box"),
      account: box("#app .header-actions"),
      header: box("#app .site-header"),
      card: box("#app .cast-card"),
    };
  });

const chipOf = (page: Page) => page.locator("#app .presence-chip");
const panelOf = (page: Page) => page.getByRole("dialog", { name: "Who's reading Allthing now" });

/** Once the entrances have run: the chip's count fades in. */
const settle = (page: Page) =>
  page.evaluate(() => Promise.all(document.getAnimations().filter((a) => a.effect?.getComputedTiming().endTime !== Infinity).map((a) => a.finished.catch(() => undefined))));

async function shoot(page: Page, info: TestInfo, name: string, clip?: { x: number; y: number; width: number; height: number }) {
  await settle(page);
  await page.evaluate(() => document.fonts.ready);
  const path = SHOTS ? `${SHOTS}/${name}.png` : info.outputPath(`${name}.png`);
  await page.screenshot({ path, clip });
}

/** An answer kept from an earlier visit, so the chip shows from the app's first frame. */
const kept = (page: Page | BrowserContext, answer: Answer, at = Date.now()) =>
  page.addInitScript(
    ({ answer, at }) => {
      const day = Math.floor(Date.now() / 86_400_000);
      localStorage.setItem("allthing.presence", JSON.stringify({ id: "0123456789abcdef0123456789abcdef", day, at, next: answer.next, answer, off: 0 }));
    },
    { answer, at },
  );

test.describe("in front of the Worker", () => {
  test.skip(PAGES, "the static copy has no Worker to beat to");

  test("a visibility change during an in-flight beat refreshes after its answer", async ({ page }) => {
    await signIn(page, me);
    await kept(page, { online: 2, readers: [ann], next: 240 }, 0);
    let release!: () => void;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    const { beats } = await serve(page, () => ok(2, [ann]), gate);
    await page.goto("/");
    const chip = chipOf(page);
    await expect(chip).toBeVisible();
    await expect.poll(() => beats.length).toBe(1);
    await chip.click();
    const show = panelOf(page).getByRole("switch", { name: "Show me here" });
    await show.click();
    await expect(show).toHaveAttribute("aria-checked", "false");
    expect(beats[0].body.hidden).toBe(false);
    release();
    await expect.poll(() => beats.length).toBe(2);
    expect(beats[1].body).toEqual({ ...beats[0].body, hidden: true, refresh: true });
    await expect(panelOf(page).locator(".presence-readers a")).toHaveCount(1);
    await expect(panelOf(page).getByRole("link", { name: "@ann", exact: true })).toBeVisible();
  });

  for (const width of [1280, 921, 920, 390]) {
    test(`the chip comes after the page's own call and moves nothing at width ${width}`, async ({ browser, page: desktop }, info) => {
      const { page, close } = await opened(browser, desktop, info, width);
      let open = () => {};
      const gate = new Promise<void>((done) => (open = done));
      const { beats, served } = await serve(page, () => ok(12, [ann, bob]), gate);
      await watchShifts(page);
      const started = Date.now();
      await page.goto("/");
      await expect(page.locator("#app .cast-card").first()).toBeVisible();
      // The beat is no part of the page view: it waits for the page's own
      // call, two seconds and an idle moment, and is the page's only POST.
      await expect.poll(() => beats.length, { timeout: 15_000 }).toBe(1);
      expect(served.home).toBeGreaterThan(0);
      expect(beats[0].at).toBeGreaterThan(served.home);
      expect(beats[0].at - started).toBeGreaterThanOrEqual(2_000);
      expect(beats[0].method).toBe("POST");
      expect(beats[0].body.id).toMatch(/^[0-9a-f]{32}$/);
      expect(beats[0].body.hidden).toBe(false);
      await settle(page);
      const before = await boxes(page);
      await shifts(page);
      await expect(chipOf(page)).toHaveCount(0);

      // The answer comes: the chip shows at every width, and nothing else moves.
      open();
      const chip = chipOf(page);
      await expect(chip).toHaveCount(1);
      await expect(chip).toHaveAttribute("aria-label", "12 readers online");
      expect(await chip.getAttribute("title")).toBeNull();
      await settle(page);
      expect(await boxes(page)).toEqual(before);
      expect(await shifts(page)).toEqual([]);
      // Home stays as it was, at every width.
      await expect(page.getByRole("link", { name: "Home", exact: true })).toBeVisible();
      {
        // A dot and the count after Home, clear of search on either row.
        await expect(chip).toBeVisible();
        await expect(chip).toHaveAccessibleName("12 readers online");
        await expect(chip).toHaveText("12", { useInnerText: true });
        const box = (await chip.boundingBox())!;
        const [hx, , hw] = before.home!;
        const [sx, sy] = before.search!;
        expect(box.x).toBeGreaterThanOrEqual(hx + hw + 4);
        if (width <= 640) expect(box.y + box.height + 8).toBeLessThanOrEqual(sy);
        else expect(box.x + box.width + 8).toBeLessThanOrEqual(sx);
        expect(box.height).toBeLessThanOrEqual(before.header![3]);
        // 920 is the exact compact breakpoint; the next pixel is full size.
        const leftPadding = await chip.evaluate((el) => getComputedStyle(el).paddingLeft);
        expect(leftPadding).toBe(width <= 920 ? "6px" : "8px");
      }
      await page.waitForTimeout(1_000);
      expect(beats).toHaveLength(1);
      await shoot(page, info, `chip-${width}-signed-out`, { x: 0, y: 0, width, height: 120 });
      await close();
    });

    test(`hovering or clicking it shows who is reading, naming only those shown by name, at width ${width}`, async ({ browser, page: desktop }, info) => {
      const { page, close } = await opened(browser, desktop, info, width);
      const { beats } = await serve(page, () => ok(5, [ann, bob]));
      await page.goto("/");
      const chip = chipOf(page);
      await expect(chip).toBeVisible({ timeout: 15_000 });
      const panel = panelOf(page);
      if (width > 920) await chip.hover();
      else await chip.click();
      await expect(panel).toBeVisible();
      await expect(panel.locator(".presence-head")).toHaveText("5 reading Allthing now");
      const named = panel.getByRole("link");
      await expect(named).toHaveCount(2);
      await expect(panel.getByRole("link", { name: "@ann", exact: true })).toHaveAttribute("href", "/u/ann");
      await expect(panel.getByRole("link", { name: "@bob", exact: true })).toHaveAttribute("href", "/u/bob");
      await expect(panel.locator(".presence-readers img")).toHaveCount(2);
      // 24 px each, as laid out (the panel's entrance scales it a moment).
      for (const img of await panel.locator(".presence-readers img").all()) {
        expect(await img.evaluate((el) => [(el as HTMLElement).offsetWidth, (el as HTMLElement).offsetHeight])).toEqual([24, 24]);
      }
      // The others are a count, and nothing more.
      await expect(panel.locator(".presence-foot")).toContainText("and 3 more");
      expect((await panel.innerText()).replace(/\s+/g, " ").trim()).toBe("5 reading Allthing now and 3 more");
      await expect(panel.getByRole("switch")).toHaveCount(0);
      await expect(panel.locator("[title]")).toHaveCount(0);
      // The panel stays on screen.
      const p = (await panel.boundingBox())!;
      expect(p.x).toBeGreaterThanOrEqual(0);
      expect(p.x + p.width).toBeLessThanOrEqual(width);
      {
        // A picture's name shows as the pointer rests on it.
        await panel.getByRole("link", { name: "@bob", exact: true }).hover();
        await expect(panel.locator(".presence-caption")).toHaveText("@bob");
      }
      await shoot(page, info, `popover-${width}-signed-out`, { x: 0, y: 0, width, height: 260 });
      // Escape closes it, back on the chip.
      await page.keyboard.press("Escape");
      await expect(panel).toHaveCount(0);
      expect(beats).toHaveLength(1);
      await close();
    });
  }

  test("the keyboard opens it with Enter and closes it with Escape, back on the chip", async ({ page }) => {
    await serve(page, () => ok(3, [ann]));
    await page.goto("/");
    const chip = chipOf(page);
    await expect(chip).toBeVisible({ timeout: 15_000 });
    await page.getByRole("link", { name: "Home", exact: true }).focus();
    await page.keyboard.press("Tab");
    await expect(chip).toBeFocused();
    // Focus alone doesn't open it: Enter does, as a press would.
    await page.waitForTimeout(400);
    await expect(chip).toHaveAttribute("aria-expanded", "false");
    await page.keyboard.press("Enter");
    const panel = panelOf(page);
    await expect(panel).toBeVisible();
    await expect(chip).toHaveAttribute("aria-expanded", "true");
    // Tab goes on into it, and its picture's name shows for the keyboard too.
    await page.keyboard.press("Tab");
    await expect(panel.getByRole("link", { name: "@ann", exact: true })).toBeFocused();
    await expect(panel.locator(".presence-caption")).toHaveText("@ann");
    await page.keyboard.press("Escape");
    await expect(panel).toHaveCount(0);
    await expect(chip).toBeFocused();
    await page.keyboard.press("Space");
    await expect(panel).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(chip).toBeFocused();
  });

  for (const width of [1280, 920, 390]) {
    test(`a signed-in reader leads the list and can hide, which beats at once, at width ${width}`, async ({ browser, page: desktop }, info) => {
      const { page, close } = await opened(browser, desktop, info, width);
      await signIn(page, me);
      const { beats } = await serve(page, (n) => (n <= 2 ? ok(2, [ann]) : ok(2, [me, ann])));
      await page.goto("/");
      const chip = chipOf(page);
      await expect(chip).toBeVisible({ timeout: 15_000 });
      // Named locally without adding a reader the server already counted.
      await expect(chip).toHaveAccessibleName("2 readers online");
      await chip.click();
      const panel = panelOf(page);
      await expect(panel.locator(".presence-head")).toHaveText("2 reading Allthing now");
      expect(await panel.locator(".presence-readers a").evaluateAll((links) => links.map((a) => a.getAttribute("aria-label")))).toEqual(["@reader", "@ann"]);
      // On touch screens this optional footer is absent when everyone is
      // named; assert the panel's text without requiring that footer.
      await expect(panel).not.toContainText("and 1 more");
      const show = panel.getByRole("switch", { name: "Show me here" });
      await expect(show).toHaveAttribute("aria-checked", "true");
      await shoot(page, info, `popover-${width}-signed-in`, { x: 0, y: 0, width, height: 300 });

      // Hidden: counted, not named, and the Worker hears it at once.
      await show.click();
      await expect(show).toHaveAttribute("aria-checked", "false");
      expect(await page.evaluate(() => localStorage.getItem("allthing.presence-hidden"))).toBe("1");
      await expect.poll(() => beats.length).toBe(2);
      expect(beats[1].body.hidden).toBe(true);
      expect(beats[1].body.refresh).toBe(true);
      expect(beats[1].body.id).toBe(beats[0].body.id);
      expect(await panel.locator(".presence-readers a").evaluateAll((links) => links.map((a) => a.getAttribute("aria-label")))).toEqual(["@ann"]);
      await expect(panel.locator(".presence-head")).toHaveText("2 reading Allthing now");
      await shoot(page, info, `popover-${width}-signed-in-hidden`, { x: 0, y: 0, width, height: 300 });

      // Shown again.
      await show.click();
      await expect(show).toHaveAttribute("aria-checked", "true");
      await expect.poll(() => beats.length).toBe(3);
      expect(beats[2].body.hidden).toBe(false);
      expect(beats[2].body.refresh).toBe(true);
      expect(await page.evaluate(() => localStorage.getItem("allthing.presence-hidden"))).toBeNull();
      expect(await panel.locator(".presence-readers a").evaluateAll((links) => links.map((a) => a.getAttribute("aria-label")))).toEqual(["@reader", "@ann"]);
      // Closed (focus back on the chip, then away), the chip as signed in.
      await page.keyboard.press("Escape");
      await expect(panel).toHaveCount(0);
      await expect(chip).toBeFocused();
      await chip.blur();
      await page.mouse.move(width - 5, 400);
      await shoot(page, info, `chip-${width}-signed-in`, { x: 0, y: 0, width, height: 120 });
      await close();
    });
  }

  test("a hidden tab doesn't beat; shown again, it beats at once", async ({ page }) => {
    await page.addInitScript(() => {
      let hidden = true;
      Object.defineProperty(document, "hidden", { get: () => hidden, configurable: true });
      Object.defineProperty(document, "visibilityState", { get: () => (hidden ? "hidden" : "visible"), configurable: true });
      const flip = (to: boolean) => {
        hidden = to;
        document.dispatchEvent(new Event("visibilitychange"));
      };
      Object.assign(window, { __show: () => flip(false), __hide: () => flip(true) });
    });
    const { beats } = await serve(page, () => ok(4));
    await page.goto("/");
    await expect(page.locator("#app .cast-card").first()).toBeVisible();
    await page.waitForTimeout(5_000);
    expect(beats).toHaveLength(0);
    await expect(chipOf(page)).toHaveCount(0);
    await page.evaluate(() => (window as unknown as { __show: () => void }).__show());
    await expect.poll(() => beats.length).toBe(1);
    await expect(chipOf(page)).toBeVisible();
    // Hidden again, and back before the interval is up: no beat.
    await page.evaluate(() => (window as unknown as { __hide: () => void }).__hide());
    await page.evaluate(() => (window as unknown as { __show: () => void }).__show());
    await page.waitForTimeout(1_500);
    expect(beats).toHaveLength(1);
  });

  test("a tab no one has used for five minutes stops beating, and input brings it back", async ({ page }) => {
    await page.clock.install();
    // Each beat's answer one more, to see that the page has it.
    const { beats } = await serve(page, (n) => ok(1 + n));
    await page.goto("/");
    const chip = chipOf(page);
    await expect(chip).toHaveAccessibleName("2 readers online", { timeout: 15_000 });
    // Four and a half minutes on, still in use (its last input, the visit,
    // is within five minutes): the next beat.
    await page.clock.fastForward("04:30");
    await expect(chip).toHaveAccessibleName("3 readers online");
    expect(beats).toHaveLength(2);
    // Nine minutes on, idle: no more.
    await page.clock.fastForward("04:30");
    await page.waitForTimeout(1_000);
    await page.clock.fastForward("10:00");
    await page.waitForTimeout(1_000);
    expect(beats).toHaveLength(2);
    // The reader is back: a beat at once, as the last is long past.
    await page.mouse.move(200, 300);
    await page.mouse.move(220, 320);
    await expect(chip).toHaveAccessibleName("4 readers online");
    expect(beats).toHaveLength(3);
  });

  test("one tab beats for the browser: another tab and a reload cost nothing, and each shows what one heard", async ({ browser }, info) => {
    const context = await browser.newContext({ baseURL: info.project.use.baseURL, viewport: { width: 1280, height: 900 } });
    await signIn(context, me);
    const { beats } = await serve(context, (n) => (n === 1 ? ok(3, [ann, bob]) : ok(7, [ann, bob])));
    const first = await context.newPage();
    await first.goto("/");
    await expect(chipOf(first)).toHaveAccessibleName("3 readers online", { timeout: 15_000 });
    expect(beats).toHaveLength(1);
    const second = await context.newPage();
    await second.goto("/");
    await expect(chipOf(second)).toHaveAccessibleName("3 readers online");
    expect(await second.evaluate(() => document.visibilityState)).toBe("visible");
    await second.waitForTimeout(5_000);
    await first.reload();
    await expect(chipOf(first)).toBeVisible();
    await first.waitForTimeout(5_000);
    expect(beats).toHaveLength(1);
    // A beat from one tab (here, the reader hiding), and the other shows its answer.
    await first.bringToFront();
    await chipOf(first).click();
    await panelOf(first).getByRole("switch", { name: "Show me here" }).click();
    await expect.poll(() => beats.length).toBe(2);
    await expect(chipOf(second)).toHaveAccessibleName("7 readers online");
    await expect(chipOf(first)).toHaveAccessibleName("7 readers online");
    await context.close();
  });

  for (const width of [1280, 920, 390]) {
    test(`where presence is off, there is no chip and no more beats, at width ${width}`, async ({ browser, page: desktop }, info) => {
      const { page, close } = await opened(browser, desktop, info, width);
      const { beats } = await serve(page, () => ({ status: 404, json: { error: "Presence is off here" } }));
      await page.goto("/");
      await expect.poll(() => beats.length, { timeout: 15_000 }).toBe(1);
      await page.waitForTimeout(1_000);
      await expect(chipOf(page)).toHaveCount(0);
      await expect(page.getByRole("link", { name: "Home", exact: true })).toBeVisible();
      await page.reload();
      await expect(page.locator("#app .cast-card").first()).toBeVisible();
      await page.waitForTimeout(5_000);
      expect(beats).toHaveLength(1);
      await expect(chipOf(page)).toHaveCount(0);
      await close();
    });
  }

  test("the chip never meets the search box, and Home stays, at any width", async ({ browser, page }, info) => {
    for (const signedIn of [false, true]) {
      const context = await browser.newContext({ baseURL: info.project.use.baseURL });
      if (signedIn) await signIn(context, me);
      await serve(context, () => ok(1234));
      // A beat a moment ago, from another tab: the chip shows from the first frame.
      await kept(context, { online: 1234, readers: [], next: 240 });
      const tab = await context.newPage();
      for (const width of [1280, 1024, 1000, 960, 921, 920, 901, 900, 841, 700, 641, 640, 390, 360]) {
        await tab.setViewportSize({ width, height: 800 });
        if (width === 1280) await tab.goto("/");
        const chip = chipOf(tab);
        await expect(chip).toHaveCount(1);
        await settle(tab);
        const where = `${signedIn ? "signed in" : "signed out"} at ${width}`;
        await expect(tab.getByRole("link", { name: "Home", exact: true }), where).toBeVisible();
        await expect(chip, where).toBeVisible();
        await expect(chip).toHaveText("1.2k", { useInnerText: true });
        const laid = await tab.evaluate(() => {
          const r = (s: string) => document.querySelector(s)!.getBoundingClientRect();
          const searchWith = r("#app .search-box").width;
          // The search box without the chip: as wide.
          const chip = document.querySelector("#app .presence") as HTMLElement;
          chip.style.display = "none";
          const searchWithout = r("#app .search-box").width;
          chip.style.display = "";
          return {
            chip: r("#app .presence-chip"), home: r("#app .main-nav a"), search: r("#app .search-box"),
            searchWith, searchWithout, scroll: document.documentElement.scrollWidth,
          };
        });
        expect(laid.searchWith, where).toBe(laid.searchWithout);
        expect(laid.scroll, where).toBeLessThanOrEqual(width);
        expect(laid.chip.x, where).toBeGreaterThanOrEqual(laid.home.right + 4);
        if (width <= 640) expect(laid.chip.bottom + 8, where).toBeLessThanOrEqual(laid.search.y);
        else expect(laid.chip.right + 8, where).toBeLessThanOrEqual(laid.search.x);
      }
      await context.close();
    }
  });

  test("the early paint's header is the app's, chip and all", async ({ page }) => {
    // Hold the app's code until the early paint is measured.
    let release: () => void = () => {};
    let held = Promise.resolve();
    await page.route("**/{assets,wasm}/allthing-*.wasm", async (route) => {
      await held;
      await route.continue();
    });
    await serve(page, () => ok(9));
    await kept(page, { online: 9, readers: [], next: 240 });
    for (const width of [1280, 390]) {
      held = new Promise<void>((done) => (release = done));
      await page.setViewportSize({ width, height: 844 });
      await page.goto("/");
      const boot = await page.locator("#boot .site-header").boundingBox();
      expect(boot).not.toBeNull();
      release();
      await expect(page.locator("#boot")).toHaveCount(0);
      await expect(chipOf(page)).toBeVisible();
      expect(await page.locator("#app .site-header").boundingBox()).toEqual(boot);
    }
  });

  test("axe finds nothing in the header with the popover open", async ({ page }) => {
    await signIn(page, me);
    await serve(page, () => ok(14, [ann, bob, cat]));
    await page.goto("/");
    const chip = chipOf(page);
    await expect(chip).toBeVisible({ timeout: 15_000 });
    await settle(page);
    // At rest, its muted count reads at 4.5:1 or more on the header.
    const ratio = await chip.evaluate((el) => {
      const rgb = (css: string) => css.match(/\d+(\.\d+)?/g)!.slice(0, 3).map((v) => Number(v) / 255);
      const lum = (css: string) => {
        const [r, g, b] = rgb(css).map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
        return 0.2126 * r + 0.7152 * g + 0.0722 * b;
      };
      const text = getComputedStyle(el).color;
      const paper = getComputedStyle(document.documentElement).getPropertyValue("--paper");
      const probe = document.createElement("span");
      probe.style.color = paper;
      document.body.append(probe);
      const ground = getComputedStyle(probe).color;
      probe.remove();
      const [hi, lo] = [lum(text), lum(ground)].sort((a, b) => b - a);
      return (hi + 0.05) / (lo + 0.05);
    });
    expect(ratio).toBeGreaterThanOrEqual(4.5);
    await chip.click();
    await expect(panelOf(page)).toBeVisible();
    await settle(page);
    await page.evaluate(axe.source);
    const violations = await page.evaluate(async () => {
      // @ts-expect-error axe is the injected script's global
      const result = await window.axe.run("#app .site-header", { resultTypes: ["violations"] });
      return result.violations.map((v: { id: string; nodes: { target: string[] }[] }) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`);
    });
    expect(violations).toEqual([]);
  });

  test("dark mode asked for: the app stays light, and the chip as legible", async ({ browser }, info) => {
    for (const width of [1280, 920, 390]) {
      for (const signedIn of [false, true]) {
        const phone = width < 400;
        const context = await browser.newContext({
          baseURL: info.project.use.baseURL, viewport: { width, height: phone ? 844 : 900 }, isMobile: phone, hasTouch: phone, deviceScaleFactor: phone ? 2 : 1, colorScheme: "dark",
        });
        const page = await context.newPage();
        if (signedIn) await signIn(page, me);
        await serve(page, () => ok(12, [ann, bob]));
        await page.goto("/");
        const chip = chipOf(page);
        await expect(chip).toBeVisible({ timeout: 15_000 });
        const who = signedIn ? "signed-in" : "signed-out";
        await shoot(page, info, `chip-${width}-${who}-dark`, { x: 0, y: 0, width, height: 120 });
        await chip.click();
        await expect(panelOf(page)).toBeVisible();
        await shoot(page, info, `popover-${width}-${who}-dark`, { x: 0, y: 0, width, height: 300 });
        expect(await page.evaluate(() => matchMedia("(prefers-color-scheme: dark)").matches)).toBe(true);
        expect(await page.evaluate(() => getComputedStyle(document.documentElement).colorScheme)).toBe("light");
        await context.close();
      }
    }
  });
});

test("the static copy has no chip and sends no beat", async ({ page }) => {
  test.skip(!PAGES, "the static copy, under /Allthing");
  const beats: string[] = [];
  page.on("request", (request) => request.url().includes("/presence") && beats.push(request.url()));
  await page.route("https://haatz.quilibrium.com/**", (route) =>
    route.fulfill({ json: { casts: [] }, headers: { "access-control-allow-origin": "*" } }),
  );
  await page.goto("/Allthing/");
  await expect(page.getByRole("link", { name: "Home", exact: true })).toBeVisible();
  await page.waitForTimeout(5_000);
  await expect(page.locator(".presence-chip")).toHaveCount(0);
  expect(beats).toEqual([]);
});
