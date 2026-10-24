// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { appCame } from "./helpers/app";

/*
 * The front page is kept while another page shows (kept.rs): Back shows it
 * at once, as it was (the same cards, where they were, in the order and
 * the sort it had), with no request and nothing drawn again, and nothing of
 * it is in the other page.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = Math.floor(Date.now() / 1000) - 600;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const author = (n: number) => ({ fid: 100 + (n % 7), username: `author${n % 7}`, display_name: `Author ${n % 7}` });
const feed = Array.from({ length: 30 }, (_, i) => ({
  hash: hex(i + 1), author: author(i), text: `Cast number ${i + 1}, long enough to take a line or two of the card.`,
  ts: ts - i * 300, likes: 30 - i, replies: 1,
}));

/** A frame, as the test samples each: the page shown, its height, the card's top and the scroll. */
type Frame = { path: string; height: number; top: number | null; y: number };

async function serve(page: Page) {
  const asked: string[] = [];
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    asked.push(later ? "home later" : "home");
    return route.fulfill({ json: { pool: later ? [] : feed, notes: {}, meta } });
  });
  await page.route("**/api/v1/latest**", (route) => {
    asked.push("latest");
    return route.fulfill({ json: { pool: [...feed].reverse(), notes: {}, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash");
    const cast = feed.find((c) => c.hash === hash) ?? feed[0];
    return route.fulfill({ json: { thread: { cast, children: [] }, notes: {}, meta } });
  });
  return asked;
}

for (const size of [
  { name: "at 1280", use: { viewport: { width: 1280, height: 900 } } },
  { name: "at 390", use: { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true } },
]) {
  test.describe(size.name, () => {
    test.use(size.use);

    test("Back shows the front page as it was: the same cards, where they were, with no request", async ({ page }) => {
      const asked = await serve(page);
      await page.goto("/");
      await appCame(page);
      const cards = page.locator("#app .cast-card[data-hash]");
      await expect(cards.first()).toBeVisible();
      // Down the feed, to a card under the header, once the cards have
      // risen into place (drawn as the app came, the last of them rise for
      // up to 450 ms).
      await page.evaluate(() => window.scrollTo(0, 1400));
      await page.waitForTimeout(400);
      await expect.poll(() => page.evaluate(() => [...document.querySelectorAll("#app .cast-card")].some((c) => c.getAnimations().length > 0))).toBe(false);
      const before = await page.evaluate(() => {
        // The first card wholly under the header.
        const header = document.querySelector("#app .site-header")!.getBoundingClientRect().bottom;
        const card = [...document.querySelectorAll("#app .cast-card[data-hash]")]
          .find((c) => c.getBoundingClientRect().top > header + 8) as HTMLElement & { kept?: boolean };
        card.kept = true;
        return { hash: card.dataset.hash!, top: card.getBoundingClientRect().top, count: document.querySelectorAll("#app .cast-card").length };
      });
      const requests = asked.length;
      // Each frame from here on: the page shown, how tall it is, where the
      // card is, and how far the page is scrolled.
      await page.evaluate((hash) => {
        const w = window as Window & { __frames?: Frame[] };
        w.__frames = [];
        const frame = () => {
          const card = document.querySelector(`#app .cast-card[data-hash="${hash}"]`);
          w.__frames!.push({ path: location.pathname, height: document.documentElement.scrollHeight, top: card?.getBoundingClientRect().top ?? null, y: scrollY });
          requestAnimationFrame(frame);
        };
        requestAnimationFrame(frame);
      }, before.hash);
      const frames = () => page.evaluate(() => (window as Window & { __frames?: Frame[] }).__frames!);
      // The sampler must have the page being left, even when navigation
      // can complete before its next frame on a fast retained route.
      await expect.poll(async () => (await frames()).some((f) => f.path === "/" && f.top !== null),
        { timeout: 3000, message: "a frame of the front page before leaving" }).toBe(true);
      await page.locator(`#app .cast-card[data-hash="${before.hash}"] .cast-text`).click();
      await expect(page.locator("#app .post")).toBeVisible();
      // None of the front page is in the post's page.
      await expect(page.locator("#app .cast-list .cast-card")).toHaveCount(0);
      // DOM visibility can precede a paint. Record the post before Back,
      // rather than letting both route changes fit between sampled frames.
      await expect.poll(async () => (await frames()).findIndex((f) => f.path === `/p/${before.hash}` && f.top === null),
        { timeout: 3000, message: "a frame of the post before Back" }).toBeGreaterThan(0);
      await page.goBack();
      await expect(page).toHaveURL(/\/$/);
      const after = () => page.evaluate((hash) => {
        const card = document.querySelector(`#app .cast-card[data-hash="${hash}"]`) as HTMLElement & { kept?: boolean };
        return { same: card.kept === true, top: card.getBoundingClientRect().top, count: document.querySelectorAll("#app .cast-card").length };
      }, before.hash);
      // Where the reader left the card: its last frame before the post. Not
      // where it was before the click, which can scroll the page first: a
      // card not yet still as Playwright checks it is scrolled into view
      // again, to the bottom of the screen (on CI, 646 px up).
      const sampled = await frames();
      const left = sampled.findIndex((f) => f.path !== "/");
      expect(left, "a frame of the post").toBeGreaterThan(0);
      const there = sampled[left - 1];
      // Back where it was, within the frames the place takes to come back.
      try {
        await expect.poll(async () => Math.abs((await after()).top - there.top!), { timeout: 3000 }).toBeLessThanOrEqual(2);
      } catch (error) {
        const all = await frames();
        const back = all.findIndex((f, i) => i > left && f.path === "/");
        throw new Error(`${(error as Error).message}\nthe frames leaving the front page, and from Back: ${JSON.stringify([...all.slice(Math.max(0, left - 2), left + 1), ...all.slice(back, back + 30)])}`);
      }
      // As tall as it was, from its first frame back: each card the height
      // it had, so nothing moved under the reader as it was put back.
      // URL and DOM restoration can finish before the sampler's next rAF.
      // Wait for that frame, then still inspect its first restored height.
      await expect.poll(async () => (await frames()).findIndex((f, i) => i > left && f.path === "/" && f.top !== null),
        { timeout: 3000, message: "a frame of the front page again" }).toBeGreaterThan(left);
      const shown = await frames();
      const returned = shown.findIndex((f, i) => i > left && f.path === "/" && f.top !== null);
      expect(shown[returned].height, `its height in its first frames back: ${JSON.stringify(shown.slice(returned, returned + 6))}`).toBe(there.height);
      const back = await after();
      expect(back.same, "the card is the one that was on the page").toBe(true);
      expect(back.count).toBe(before.count);
      expect(asked.length, "nothing asked for again").toBe(requests);
      await expect(page).toHaveTitle("Allthing");
    });
  });
}

test("the front page kept keeps its sort, and another page's address doesn't redraw it", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  const asked = await serve(page);
  await page.goto("/?sort=new");
  await appCame(page);
  const first = page.locator("#app .cast-list .cast-card[data-hash]").first();
  await expect(first).toHaveAttribute("data-hash", feed[0].hash);
  await first.evaluate((card) => ((card as HTMLElement & { kept?: boolean }).kept = true));
  const requests = asked.length;
  await first.locator(".cast-text").click();
  await expect(page.locator("#app .post")).toBeVisible();
  await page.goBack();
  await expect(page).toHaveURL(/\/\?sort=new$/);
  await expect(page.locator("#app .sort-tabs a.selected")).toHaveText("new");
  expect(await first.evaluate((card) => (card as HTMLElement & { kept?: boolean }).kept)).toBe(true);
  await expect(page).toHaveTitle("new · Allthing");
  expect(asked.length).toBe(requests);
});

// Observe real deliveries, or hold a later one to exercise the interval
// between a size change and its observer callback. No measurements are timed.
async function retentionProbe(page: Page, unsupported: boolean) {
  await page.addInitScript((unsupported) => {
    const state = {
      armed: false, hold: false, observed: new WeakMap<Element, { height: number; width: number; style: string | null; class: string | null }>(), unobserved: new WeakSet<Element>(),
      deliveries: new WeakMap<Element, number>(), content: new WeakMap<Element, number>(),
      styles: [] as string[], pending: [] as Array<() => void>, expectedHeight: 0,
    };
    (window as any).__retention = state;
    const style = window.getComputedStyle.bind(window);
    window.getComputedStyle = ((element, pseudo) => {
      if (state.armed && element.matches(".kept-front .cast-card") && state.styles.length < 80) {
        state.styles.push(element.getAttribute("data-hash")!);
      }
      return style(element, pseudo);
    }) as typeof window.getComputedStyle;
    if (unsupported) {
      Object.defineProperty(window, "ResizeObserver", { value: undefined, configurable: true });
      return;
    }
    const NativeObserver = window.ResizeObserver;
    window.ResizeObserver = class extends NativeObserver {
      constructor(callback: ResizeObserverCallback) {
        super((entries, observer) => {
          const deliver = () => {
            callback(entries, observer);
            for (const entry of entries) if (entry.contentRect.height > 0) {
              const border = entry.borderBoxSize[0];
              state.observed.set(entry.target, { height: border.blockSize, width: border.inlineSize,
                style: entry.target.getAttribute("style"), class: entry.target.getAttribute("class") });
              state.deliveries.set(entry.target, (state.deliveries.get(entry.target) ?? 0) + 1);
              state.content.set(entry.target, entry.contentRect.height);
            }
          };
          if (state.hold) state.pending.push(deliver); else deliver();
        });
      }
      unobserve(element: Element) {
        state.unobserved.add(element);
        super.unobserve(element);
      }
    };
  }, unsupported);
}

for (const mode of ["observed", "late fractional change", "content change within a fixed border", "late padding change within a fixed border", "without ResizeObserver"] as const) {
  test(`Home retention preserves exact sizes with fewer navigation reads: ${mode}`, async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 900 });
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await retentionProbe(page, mode === "without ResizeObserver");
    const asked = await serve(page);
    await page.route("**/api/v1/home**", (route) => {
      const later = new URL(route.request().url()).searchParams.has("offset");
      asked.push(later ? "home later" : "home");
      return route.fulfill({ json: { pool: later ? [] : feed.slice(0, 3).map((cast) => ({ ...cast, ts: cast.ts - 7200 })), notes: {}, meta } });
    });
    // This deliberately short Hot feed exhausts the existing ten-page
    // source budget while its sentinel is in view. Snapshot requests only
    // after that real background work has finished, before testing Back.
    const lastPage = page.waitForResponse((response) => {
      const url = new URL(response.url());
      return url.pathname === "/api/v1/home" && url.searchParams.get("offset") === "900";
    });
    await page.goto("/");
    await appCame(page);
    await lastPage;
    await expect(page.locator("#app .cast-list[aria-busy='true']")).toHaveCount(0);
    const cards = page.locator("#app .cast-list .cast-card");
    await expect(cards).toHaveCount(3);
    await cards.evaluateAll((cards) => {
      (window as any).__retainedCards = cards;
      for (const card of cards) (card as HTMLElement).style.height = "350.125px";
    });
    await expect.poll(() => cards.evaluateAll((cards) => cards.every((card) => card.getAnimations().length === 0))).toBe(true);
    if (mode !== "without ResizeObserver") {
      await expect.poll(() => cards.evaluateAll((cards) => cards.every((card) => {
        const observed = (window as any).__retention.observed.get(card);
        const rect = card.getBoundingClientRect();
        return observed?.height === 350.125 && observed.width === rect.width
          && observed.style === card.getAttribute("style") && observed.class === card.getAttribute("class");
      }))).toBe(true);
    }
    // The retention cache is private: hydrated cards retain the same
    // markup as early paint, including while observations are active.
    await expect(page.locator("#app [data-kept-size]")).toHaveCount(0);
    let expectedContent: number | undefined;
    if (mode === "content change within a fixed border") {
      const delivered = await cards.first().evaluate((card) => (window as any).__retention.deliveries.get(card) as number);
      await cards.first().evaluate((card) => {
        const padding = parseFloat(getComputedStyle(card).paddingTop);
        (card as HTMLElement).style.paddingTop = `${padding + 8}px`;
      });
      // The fixed outer height stays the same; the cached content height
      // changes, so content-box observation must deliver another value.
      await expect.poll(() => cards.first().evaluate((card) => (window as any).__retention.deliveries.get(card) as number)).toBeGreaterThan(delivered);
      expectedContent = await cards.first().evaluate((card) => (window as any).__retention.content.get(card) as number);
    }
    if (mode === "late padding change within a fixed border") {
      expectedContent = await cards.first().evaluate((card) => (window as any).__retention.content.get(card) - 8);
    }
    const requests = asked.length;
    await page.evaluate((mode) => {
      const state = (window as any).__retention;
      state.hold = mode.startsWith("late ");
      document.addEventListener("click", () => {
        const first = (window as any).__retainedCards[0] as HTMLElement;
        if (mode === "late fractional change") first.style.height = "350.375px";
        if (mode === "late padding change within a fixed border") first.style.paddingTop = `${parseFloat(getComputedStyle(first).paddingTop) + 8}px`;
        state.expectedHeight = document.documentElement.scrollHeight;
        state.armed = true;
      }, { capture: true, once: true });
    }, mode);
    await cards.first().locator(".cast-text").click();
    await expect(page.locator("#app .post .post-text")).toBeVisible();
    const read = await page.evaluate(() => (window as any).__retention.styles as string[]);
    expect(read).toEqual(mode === "without ResizeObserver" ? feed.slice(0, 3).map((cast) => cast.hash)
      : mode.startsWith("late ") ? [feed[0].hash] : []);
    await page.goBack();
    await expect(page).toHaveURL(/\/$/);
    await expect(cards).toHaveCount(3);
    const restored = await cards.evaluateAll((cards) => ({
      same: cards.every((card, i) => card === (window as any).__retainedCards[i]),
      heights: cards.map((card) => card.getBoundingClientRect().height),
      height: document.documentElement.scrollHeight, expected: (window as any).__retention.expectedHeight,
    }));
    expect(restored.same).toBe(true);
    await expect(page.locator("#app [data-kept-size]")).toHaveCount(0);
    expect(restored.heights).toEqual([mode === "late fractional change" ? 350.375 : 350.125, 350.125, 350.125]);
    expect(restored.height).toBe(restored.expected);
    if (expectedContent !== undefined) {
      const retained = await cards.first().evaluate((card) => parseFloat((card as HTMLElement).style.containIntrinsicHeight.replace(/^auto /, "")));
      expect(retained).toBe(expectedContent);
    }
    expect(asked.length).toBe(requests);
    expect(errors).toEqual([]);
    if (mode === "observed") {
      // Retention writes changed the inline style. A second trip must
      // still reuse valid observations, with no CSS fallback or new nodes.
      await page.evaluate(() => {
        const state = (window as any).__retention;
        state.armed = false;
        state.styles = [];
        document.addEventListener("click", () => {
          state.expectedHeight = document.documentElement.scrollHeight;
          state.armed = true;
        }, { capture: true, once: true });
      });
      await cards.first().locator(".cast-text").click();
      await expect(page.locator("#app .post .post-text")).toBeVisible();
      expect(await page.evaluate(() => (window as any).__retention.styles)).toEqual([]);
      await page.goBack();
      await expect(page).toHaveURL(/\/$/);
      await expect(cards).toHaveCount(3);
      const again = await cards.evaluateAll((cards) => ({
        same: cards.every((card, i) => card === (window as any).__retainedCards[i]),
        heights: cards.map((card) => card.getBoundingClientRect().height),
        height: document.documentElement.scrollHeight, expected: (window as any).__retention.expectedHeight,
      }));
      expect(again.same).toBe(true);
      await expect(page.locator("#app [data-kept-size]")).toHaveCount(0);
      expect(again.heights).toEqual([350.125, 350.125, 350.125]);
      expect(again.height).toBe(again.expected);
      expect(asked.length).toBe(requests);
    }
    if (mode !== "without ResizeObserver") {
      // Taking Home out preserves its owners; replacing its list disposes
      // them and must release their actual observer registrations.
      expect(await page.evaluate(() => (window as any).__retainedCards.some((card: Element) => (window as any).__retention.unobserved.has(card)))).toBe(false);
      await page.getByRole("combobox", { name: "Time range" }).selectOption({ label: "Past hour" });
      await expect(cards).toHaveCount(0);
      await expect.poll(() => page.evaluate(() => (window as any).__retainedCards.every((card: Element) =>
        (window as any).__retention.unobserved.has(card)))).toBe(true);
      await expect(page.locator("#app [data-kept-size]")).toHaveCount(0);
    }
  });
}
