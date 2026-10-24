// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";
import { NOW } from "./fixture-time";
import { appCame } from "./helpers/app";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const hash = `0x${"a".repeat(40)}`;
type TrackedWindow = Window & {
  activeClickListeners: Set<EventListenerOrEventListenerObject>;
  initialClickListeners: Set<EventListenerOrEventListenerObject>;
  popoverClickListeners: EventListenerOrEventListenerObject[];
};

test.beforeEach(async ({ page }) => {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home", (route) => route.fulfill({ json: {
    pool: [{ hash, author: { fid: 1, username: "fixture" }, text: "Popover fixture", ts: (NOW - 19), likes: 10 }],
    meta: { backend: "hypersnap", fetched_at: Date.now() },
  } }));
  await page.route("**/api/v1/reactions?*", (route) => route.fulfill({ json: {
    hash, kind: "likes", users: [{ fid: 2, username: "reader" }], more: true,
    meta: { backend: "hypersnap", fetched_at: Date.now() },
  } }));
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ json: {
    user: { fid: 2, username: "reader" }, pool: [],
    meta: { backend: "hypersnap", fetched_at: Date.now() },
  } }));
});

test("Escape closes a keyboard popover and returns focus to its trigger", async ({ page }) => {
  await page.goto("/");
  const trigger = page.getByRole("button", { name: "10 likes: show who", exact: true });
  await page.keyboard.press("Tab");
  await trigger.focus();
  const panel = page.getByRole("dialog", { name: "Liked by", exact: true });
  await panel.getByRole("button", { name: "See everyone" }).focus();
  await page.keyboard.press("Escape");
  await expect(trigger).toBeFocused();
  await expect(trigger).toHaveAttribute("aria-expanded", "false", { timeout: 1500 });
  await expect(panel).toHaveCount(0, { timeout: 1500 });
});

test("navigating from an open popover removes its window click listener", async ({ page }, info) => {
  const consoleErrors: string[] = [];
  page.on("console", message => {
    if (message.type() === "error" && consoleErrors.length < 12) consoleErrors.push(message.text().slice(0, 8000));
  });
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.stack ?? error.message));
  await page.addInitScript(() => {
    const tracked = (window as TrackedWindow).activeClickListeners = new Set();
    const add = window.addEventListener.bind(window);
    const remove = window.removeEventListener.bind(window);
    window.addEventListener = ((type, listener, options) => {
      if (type === "click" && listener) tracked.add(listener);
      add(type, listener, options);
    }) as typeof window.addEventListener;
    window.removeEventListener = ((type, listener, options) => {
      if (type === "click" && listener) tracked.delete(listener);
      remove(type, listener, options);
    }) as typeof window.removeEventListener;
  });
  await page.goto("/");
  await appCame(page);
  const trigger = page.getByRole("button", { name: "10 likes: show who", exact: true });
  await expect(trigger).toBeVisible();
  const before = await page.evaluate(() => {
    const tracked = window as TrackedWindow;
    tracked.initialClickListeners = new Set(tracked.activeClickListeners);
    return tracked.activeClickListeners.size;
  });
  await trigger.hover();
  const panel = page.getByRole("dialog", { name: "Liked by", exact: true });
  await expect(panel.locator("a.who-link")).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as TrackedWindow).activeClickListeners.size)).toBe(before + 1);
  await page.evaluate(() => {
    const tracked = window as TrackedWindow;
    tracked.popoverClickListeners = [...tracked.activeClickListeners].filter((listener) => !tracked.initialClickListeners.has(listener));
  });
  const document = await page.locator("html").elementHandle();
  await panel.locator("a.who-link").click();
  await expect(page).toHaveURL(/\/u\/reader$/);
  expect(await document!.evaluate((root) => root.isConnected).catch(() => false)).toBe(true);
  await expect(trigger).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => {
    const tracked = window as TrackedWindow;
    return tracked.popoverClickListeners.filter((listener) => tracked.activeClickListeners.has(listener)).length;
  }), { timeout: 1500 }).toBe(0);
  await page.locator("body").click({ position: { x: 5, y: 5 } });
  if (errors.length) await info.attach("popover-panic-console", { contentType: "application/json", body: JSON.stringify(consoleErrors) });
  expect(errors).toEqual([]);
});

test("unmounting an open popover removes its window click listener", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.stack ?? error.message));
  await page.goto("/channels");
  await page.getByRole("banner").getByRole("link", { name: "Home", exact: true }).click();
  await page.getByRole("button", { name: "10 likes: show who", exact: true }).hover();
  await expect(page.getByRole("dialog", { name: "Liked by", exact: true })).toBeVisible();
  await page.goBack();
  await expect(page).toHaveURL(/\/channels$/);
  await page.locator("body").click({ position: { x: 5, y: 5 } });
  expect(errors).toEqual([]);
});

// Background tabs can delay the closing-animation timer. Returning to a
// kept page must not restore a fading panel or its nested profile preview.
test("Back after leaving nested previews does not wait for their closing animations", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.stack ?? error.message));
  await page.addInitScript(() => {
    const schedule = window.setTimeout.bind(window);
    const clear = window.clearTimeout.bind(window);
    const held = new Map<number, () => void>();
    window.setTimeout = ((handler: TimerHandler, delay?: number, ...args: unknown[]) => {
      if (delay !== 120 || typeof handler !== "function") return schedule(handler, delay, ...args);
      const id = schedule(() => { held.delete(id); handler(...args); }, 60_000);
      held.set(id, () => { clear(id); handler(...args); });
      return id;
    }) as typeof window.setTimeout;
    window.clearTimeout = ((id?: number) => { if (id !== undefined) held.delete(id); clear(id); }) as typeof window.clearTimeout;
    (window as Window & { finishHeldClosings: () => void }).finishHeldClosings = () => {
      const callbacks = [...held.values()];
      held.clear();
      for (const callback of callbacks) callback();
    };
  });
  await page.goto("/");
  await page.getByRole("button", { name: "10 likes: show who", exact: true }).hover();
  const likes = page.getByRole("dialog", { name: "Liked by", exact: true });
  const reader = likes.locator("a.who-link");
  await reader.hover();
  await expect(page.getByRole("dialog", { name: "Profile: reader (@reader)", exact: true })).toBeVisible();
  await reader.click();
  await expect(page).toHaveURL(/\/u\/reader$/);
  await expect(page.getByRole("region", { name: "Profile", exact: true })).toBeVisible();
  await page.goBack();
  await expect(page.getByRole("button", { name: "10 likes: show who", exact: true })).toBeVisible();
  await expect(page.locator(".popover-panel")).toHaveCount(0);
  await page.evaluate(() => (window as Window & { finishHeldClosings: () => void }).finishHeldClosings());
  await page.locator("body").click({ position: { x: 5, y: 5 } });
  expect(errors).toEqual([]);
});

// A close already queued by pointer leave can be delivered after navigation
// has cancelled its timer. Retain those callbacks to exercise that ordering.
test("a queued nested-preview close delivered after navigation is harmless", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.stack ?? error.message));
  await page.addInitScript(() => {
    const schedule = window.setTimeout.bind(window);
    const clear = window.clearTimeout.bind(window);
    const pending: { id: number; deliver: () => void }[] = [];
    window.setTimeout = ((handler: TimerHandler, delay?: number, ...args: unknown[]) => {
      if (delay !== 180 || typeof handler !== "function") return schedule(handler, delay, ...args);
      const id = schedule(() => handler(...args), 60_000);
      pending.push({ id, deliver: () => { clear(id); handler(...args); } });
      return id;
    }) as typeof window.setTimeout;
    (window as Window & { queuedPreviewClosings: () => number; deliverPreviewClosings: () => void }).queuedPreviewClosings = () => pending.length;
    (window as Window & { deliverPreviewClosings: () => void }).deliverPreviewClosings = () => {
      for (const callback of pending.splice(0)) callback.deliver();
    };
  });
  await page.goto("/");
  await page.getByRole("button", { name: "10 likes: show who", exact: true }).hover();
  const row = page.getByRole("dialog", { name: "Liked by", exact: true }).locator("a.who-link");
  await row.hover();
  await expect(page.getByRole("dialog", { name: "Profile: reader (@reader)", exact: true })).toBeVisible();
  await page.mouse.move(5, 5);
  await expect.poll(() => page.evaluate(() => (window as Window & { queuedPreviewClosings: () => number }).queuedPreviewClosings())).toBeGreaterThanOrEqual(2);
  await row.click();
  await expect(page).toHaveURL(/\/u\/reader$/);
  await expect(page.getByRole("region", { name: "Profile", exact: true })).toBeVisible();
  await page.evaluate(() => (window as Window & { deliverPreviewClosings: () => void }).deliverPreviewClosings());
  await page.locator("body").click({ position: { x: 5, y: 5 } });
  expect(errors).toEqual([]);
});
