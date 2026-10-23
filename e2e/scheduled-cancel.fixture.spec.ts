// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";
import { approvedKey, signIn } from "./helpers/farcaster";
import { NOW } from "./fixture-time";

/*
 * Cancelling a scheduled cast on /scheduled. "Yes, cancel it" takes the
 * cast off the list at once, in a short fold, while the Worker is asked,
 * and the count goes down with it; a polite notice says "Scheduled cast
 * cancelled" once the Worker has. If the Worker can't, the cast comes back
 * where it was, with why, and focus on it; if it had already gone out, it
 * becomes a posted one, with its link, not an error. With none left, the
 * list says where to schedule one. The confirm opens on Keep it, and Keep
 * it goes back to Cancel. The Worker is a stand-in that answers a cancel
 * only when the test says, so what shows before its answer is seen.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");
test.use({ timezoneId: "Europe/Berlin", locale: "en-GB" });

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const post = { hash: hex(1), author: alice, text: "What should I read next?", ts: (NOW - 19), likes: 3, replies: 0 };
const NODE = "https://haatz.quilibrium.com/v1";
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" };
const now = () => Math.floor(Date.now() / 1000);

type Kept = { id: number; at: number; hash: string; state: "waiting" | "posted" | "failed"; error?: string; draft: string };
type Answer = { status?: number; json: object };
type Asked = { id: number; answer: (a: Answer) => Promise<void> };

const waiting = (id: number, text: string, inHours = id + 1): Kept => ({
  id,
  at: now() + inHours * 3600,
  hash: hex(100 + id),
  state: "waiting",
  draft: JSON.stringify({ text }),
});

/** The Worker's scheduled casts, in memory; a cancel waits for the test's answer. */
async function serve(page: Page, list: Kept[]) {
  const asked: Asked[] = [];
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [post], notes: {}, meta } });
  });
  await page.route("**/api/v1/signer", (route) => route.fulfill({ json: { app_fid: 9152 } }));
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  await page.route("**/api/v1/scheduled", (route) => route.fulfill({ json: { casts: [...list].sort((a, b) => a.at - b.at) } }));
  await page.route("**/api/v1/scheduled/cancel", async (route) => {
    const { id } = route.request().postDataJSON();
    await new Promise<void>((done) =>
      asked.push({
        id,
        answer: async ({ status = 200, json }) => {
          const kept = list.findIndex((k) => k.id === id);
          if (status === 200 && "outcome" in json && json.outcome === "cancelled") list.splice(kept, 1);
          if (status === 200 && "outcome" in json && json.outcome === "posted") Object.assign(list[kept], { state: "posted", hash: (json as { hash: string }).hash });
          await route.fulfill({ status, json });
          done();
        },
      }),
    );
  });
  return {
    /** The next cancel the Worker was asked for, not answered yet. */
    next: async () => {
      await expect.poll(() => asked.length).toBeGreaterThan(0);
      return asked.shift()!;
    },
    count: () => asked.length,
  };
}

/** Signed in, with a key approved an hour ago, on /scheduled. */
async function ready(page: Page, list: Kept[]) {
  await signIn(page, me);
  const cancels = await serve(page, list);
  await page.goto("/");
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  await approvedKey(page, me.fid, 3_600_000);
  await page.goto("/scheduled");
  await expect(page.getByRole("heading", { level: 1, name: "Scheduled casts" })).toBeVisible();
  return cancels;
}

const group = (page: Page, name: RegExp) => page.locator(".scheduled-group", { has: page.getByRole("heading", { name }) });
const notice = (page: Page, text: string) => page.getByRole("status").filter({ hasText: text });

/** Cancel, then Yes, on `item`, by pointer. */
async function cancel(item: Locator, yes = "Yes, cancel it") {
  await item.getByRole("button", { name: /^(Cancel|Remove)$/ }).click();
  await item.getByRole("button", { name: yes }).click();
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL,
    viewport: { width: 390, height: 844 },
    isMobile: true,
    hasTouch: true,
    timezoneId: "Europe/Berlin",
    locale: "en-GB",
  });
  return { context, page: await context.newPage() };
}

test("Yes, cancel it takes the cast off at once, before the Worker answers; the count follows, and a polite notice says so once it has", async ({ page }) => {
  const list = [waiting(1, "First"), waiting(2, "Second"), { ...waiting(3, "Lost one", -2), state: "failed" as const, error: "Farcaster didn't take it: text is too long" }];
  const cancels = await ready(page, list);
  const items = group(page, /^Waiting/).locator(".scheduled-item");
  const heading = group(page, /^Waiting/).getByRole("heading");
  await expect(items).toHaveCount(2);
  await expect(heading).toHaveText("Waiting 2");
  // The account menu counts what's waiting, as read here, asking no one.
  const menu = async (name: string) => {
    await page.locator(".account-button").hover();
    await expect(page.locator(".account-menu").getByRole("link", { name: /^Scheduled casts/ })).toHaveAccessibleName(name);
    await page.mouse.move(640, 880);
    await expect(page.locator(".account-menu")).toHaveCount(0);
  };
  await menu("Scheduled casts, 2 waiting");
  const first = items.first();

  // The question opens on Keep it, the answer that changes nothing, and
  // Keep it goes back to Cancel.
  await first.getByRole("button", { name: "Cancel" }).click();
  const asking = first.getByRole("group", { name: "Cancel this cast? It won't go out." });
  await expect(asking.getByRole("button", { name: "Keep it" })).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(asking).toHaveCount(0);
  await expect(first.getByRole("button", { name: "Cancel" })).toBeFocused();
  // By keyboard: Enter asks; Escape is Keep it too; Shift+Tab reaches Yes.
  await page.keyboard.press("Enter");
  await expect(asking.getByRole("button", { name: "Keep it" })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(asking).toHaveCount(0);
  await expect(first.getByRole("button", { name: "Cancel" })).toBeFocused();
  await page.keyboard.press("Enter");
  await page.keyboard.press("Shift+Tab");
  await expect(asking.getByRole("button", { name: "Yes, cancel it" })).toBeFocused();
  await page.keyboard.press("Enter");

  // At once, while the Worker hasn't answered: off the list, counted, and
  // focus on the next cast.
  const asked = await cancels.next();
  expect(asked.id).toBe(1);
  await expect(heading).toHaveText("Waiting 1");
  await expect(items).toHaveCount(1);
  await expect(items.locator(".scheduled-text")).toHaveText("Second");
  await expect(items.first()).toBeFocused();
  await expect(notice(page, "Scheduled cast cancelled")).toHaveCount(0);
  // Said, politely, once the Worker has done it.
  await asked.answer({ json: { outcome: "cancelled" } });
  const said = notice(page, "Scheduled cast cancelled");
  await expect(said).toBeVisible();
  await expect(said).toHaveAttribute("aria-live", "polite");
  expect(cancels.count()).toBe(0);
  await menu("Scheduled casts, 1 waiting");

  // One that didn't go out comes off the same way, as removed.
  const lost = group(page, /^Didn't go out/).locator(".scheduled-item");
  await cancel(lost, "Yes, remove it");
  await expect(group(page, /^Didn't go out/)).toHaveCount(0);
  await (await cancels.next()).answer({ json: { outcome: "cancelled" } });
  await expect(notice(page, "Removed from the list")).toBeVisible();
  await expect(items).toHaveCount(1);
});

test("the last one cancelled: the list says where to schedule one, and its button opens the composer with the clock on", async ({ page }) => {
  const cancels = await ready(page, [waiting(1, "Only one")]);
  await cancel(group(page, /^Waiting/).locator(".scheduled-item"));
  const empty = page.locator(".scheduled-empty");
  // Before the Worker has answered.
  await expect(empty).toContainText("Nothing scheduled.");
  await expect(empty).toContainText("Write a cast, and the clock beside its GIF button picks when it goes out.");
  await expect(group(page, /^Waiting/)).toHaveCount(0);
  const schedule = empty.getByRole("button", { name: "Schedule a cast" });
  await expect(schedule).toBeFocused();
  await (await cancels.next()).answer({ json: { outcome: "cancelled" } });
  await expect(notice(page, "Scheduled cast cancelled")).toBeVisible();
  await schedule.click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await expect(dialog.getByRole("button", { name: "Schedule for later" })).toHaveAttribute("aria-pressed", "true");
  await expect(dialog.getByLabel("Goes out")).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Schedule", exact: true })).toBeVisible();
  // Closed, focus is back on the button that opened it.
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(schedule).toBeFocused();
});

test("a cancel the Worker can't make: the cast comes back where it was, with why, and focus on it", async ({ page }) => {
  const cancels = await ready(page, [waiting(1, "Before"), waiting(2, "Middle"), waiting(3, "After")]);
  const items = group(page, /^Waiting/).locator(".scheduled-item");
  const heading = group(page, /^Waiting/).getByRole("heading");
  await cancel(items.nth(1));
  await expect(heading).toHaveText("Waiting 2");
  await expect(items).toHaveCount(2);
  const asked = await cancels.next();
  await asked.answer({ status: 503, json: { error: "Scheduled casts can't be read or kept just now; try again shortly" } });
  await expect(items).toHaveCount(3);
  await expect(heading).toHaveText("Waiting 3");
  await expect(items.locator(".scheduled-text")).toHaveText(["Before", "Middle", "After"]);
  const middle = items.nth(1);
  await expect(middle.getByRole("alert")).toHaveText("Scheduled casts can't be read or kept just now; try again shortly");
  const again = middle.getByRole("button", { name: "Cancel" });
  await expect(again).toBeFocused();
  await expect(again).toHaveAccessibleDescription("Scheduled casts can't be read or kept just now; try again shortly");
  await expect(notice(page, "cancelled")).toHaveCount(0);
  // Tried again, it goes, and the error with it.
  await page.keyboard.press("Enter");
  await page.keyboard.press("Shift+Tab");
  await page.keyboard.press("Enter");
  await (await cancels.next()).answer({ json: { outcome: "cancelled" } });
  await expect(items.locator(".scheduled-text")).toHaveText(["Before", "After"]);
  await expect(notice(page, "Scheduled cast cancelled")).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("a cast that went out before the cancel came becomes a posted one, with View cast, not an error", async ({ page }) => {
  const cancels = await ready(page, [waiting(1, "Going now"), waiting(2, "Later")]);
  const items = group(page, /^Waiting/).locator(".scheduled-item");
  await cancel(items.first());
  await expect(items).toHaveCount(1);
  await (await cancels.next()).answer({ json: { outcome: "posted", hash: hex(21) } });
  const posted = group(page, /^Posted this week/);
  await expect(posted.locator(".scheduled-item")).toHaveCount(1);
  await expect(posted.locator(".scheduled-text")).toHaveText("Going now");
  await expect(posted.locator(".scheduled-item")).toContainText("It had already gone out, so it wasn't cancelled.");
  const view = posted.getByRole("link", { name: "View cast" });
  await expect(view).toHaveAttribute("href", `/p/${hex(21)}`);
  await expect(view).toBeFocused();
  await expect(view).toHaveAccessibleDescription("It had already gone out, so it wasn't cancelled.");
  await expect(group(page, /^Waiting/).getByRole("heading")).toHaveText("Waiting 1");
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(notice(page, "cancelled")).toHaveCount(0);
});

test("with reduced motion, a cast goes at once; otherwise it folds away", async ({ page }) => {
  const cancels = await ready(page, [waiting(1, "One"), waiting(2, "Two"), waiting(3, "Three")]);
  const items = group(page, /^Waiting/).locator(".scheduled-item");
  /** Presses Yes on the first cast; how many casts there are two frames on, and whether the one pressed was folding. */
  const yes = () =>
    page.evaluate(async () => {
      const first = document.querySelector(".scheduled-item")!;
      [...first.querySelectorAll("button")].find((b) => b.textContent?.trim() === "Yes, cancel it")!.click();
      await new Promise(requestAnimationFrame);
      const folding = first.isConnected && first.getAnimations().some((a) => a.playState === "running");
      await new Promise(requestAnimationFrame);
      return { left: document.querySelectorAll(".scheduled-item").length, folding };
    });
  await items.first().getByRole("button", { name: "Cancel" }).click();
  expect(await yes()).toEqual({ left: 3, folding: true });
  await expect(items).toHaveCount(2);
  await (await cancels.next()).answer({ json: { outcome: "cancelled" } });

  await page.emulateMedia({ reducedMotion: "reduce" });
  await items.first().getByRole("button", { name: "Cancel" }).click();
  expect(await yes()).toEqual({ left: 1, folding: false });
  await (await cancels.next()).answer({ json: { outcome: "cancelled" } });
  await expect(notice(page, "Scheduled cast cancelled")).toBeVisible();
});

test("at 390, the question fits, a tap takes the cast off at once, and the notice shows on screen", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  const cancels = await ready(page, [waiting(1, "A long one ".repeat(20)), waiting(2, "Second")]);
  const items = group(page, /^Waiting/).locator(".scheduled-item");
  const first = items.first();
  await first.getByRole("button", { name: "Cancel" }).tap();
  const asking = first.getByRole("group", { name: "Cancel this cast? It won't go out." });
  const box = (await asking.boundingBox())!;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(390);
  const answers = [];
  for (const name of ["Yes, cancel it", "Keep it"]) {
    const button = (await asking.getByRole("button", { name }).boundingBox())!;
    expect(button.x + button.width).toBeLessThanOrEqual(390);
    // A fingertip's tap: a band 44 px tall, as wide as the button.
    const band = await asking.getByRole("button", { name }).evaluate((el) => parseFloat(getComputedStyle(el, "::after").height));
    expect(band).toBeGreaterThanOrEqual(44);
    answers.push(button);
  }
  // The two answers side by side, under the question.
  expect(Math.abs(answers[0].y - answers[1].y)).toBeLessThan(2);
  expect(answers[0].y).toBeGreaterThan((await asking.locator(".muted").boundingBox())!.y);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await asking.getByRole("button", { name: "Yes, cancel it" }).tap();
  await expect(items).toHaveCount(1);
  await expect(group(page, /^Waiting/).getByRole("heading")).toHaveText("Waiting 1");
  await (await cancels.next()).answer({ json: { outcome: "cancelled" } });
  const said = notice(page, "Scheduled cast cancelled");
  await expect(said).toBeVisible();
  // Wait for it to come in before measuring it.
  await expect.poll(() => said.evaluate((el) => getComputedStyle(el).opacity)).toBe("1");
  const toast = (await said.boundingBox())!;
  expect(toast.x).toBeGreaterThanOrEqual(0);
  expect(toast.x + toast.width).toBeLessThanOrEqual(390);
  expect(toast.y + toast.height).toBeLessThanOrEqual(844);
  await context.close();
});
