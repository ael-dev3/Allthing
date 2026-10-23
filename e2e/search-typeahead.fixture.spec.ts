// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { NOW } from "./fixture-time";
import { appCame } from "./helpers/app";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const people = [
  { fid: 2, username: "fixer", display_name: "Fixer One", pfp_url: "https://media.example/a.png", follower_count: 10 },
  { fid: 3, username: "fixture.eth", display_name: "Fixture Two", follower_count: 5 },
];
const channels = [{ id: "fixtures", name: "Fixtures", parent_url: "https://farcaster.xyz/~/channel/fixtures" }];
const casts = [{ hash: `0x${"c".repeat(40)}`, author: { fid: 2, username: "fixer" }, text: "A fixture cast about search", ts: (NOW - 19) }];

async function serve(page: Page) {
  const queries: string[] = [];
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home", (route) => route.fulfill({ json: { pool: [], meta } }));
  await page.route("**/api/v1/search?*", (route) => {
    const q = new URL(route.request().url()).searchParams.get("q")!;
    queries.push(q);
    return route.fulfill({ json: { query: q, users: people, channels, casts, meta } });
  });
  await page.route("https://media.example/**", (route) => route.fulfill({ status: 404 }));
  return queries;
}

/** Playwright's 1e12 timer IDs must survive the browser's WebIDL i32 ABI. */
async function browserClock(page: Page) {
  await page.clock.install({ time: Date.now() });
  await page.addInitScript(() => {
    // Native browser handles fit i32. Without aliases, web-sys truncates the
    // clock's 1e12 handles and cancellation silently leaves old timers live.
    // All timer families share this allocator so their aliases cannot collide.
    const target = window as unknown as Record<string, Function>;
    const handles = new Map<number, number>();
    let next = 1;
    for (const [schedule, cancel, once] of [
      ["setTimeout", "clearTimeout", true], ["setInterval", "clearInterval", false],
      ["requestAnimationFrame", "cancelAnimationFrame", true], ["requestIdleCallback", "cancelIdleCallback", true],
    ] as const) {
      const original = target[schedule];
      const clear = target[cancel];
      if (typeof original !== "function" || typeof clear !== "function") continue;
      target[schedule] = (callback: Function | string, ...args: unknown[]) => {
        if (next > 0x7fffffff) throw new Error("The fixture clock exhausted browser timer IDs");
        const id = next++;
        const task = typeof callback === "function" && once ? function (this: unknown, ...values: unknown[]) {
          handles.delete(id);
          return Reflect.apply(callback, this, values);
        } : callback;
        handles.set(id, Reflect.apply(original, window, [task, ...args]));
        return id;
      };
      target[cancel] = (id: number) => {
        const raw = handles.get(id) ?? id;
        handles.delete(id);
        return Reflect.apply(clear, window, [raw]);
      };
    }
  });
}

test("suggestions appear as you type: people, channels and posts", async ({ page }) => {
  const queries = await serve(page);
  // Install before the app binds any timer APIs.
  await browserClock(page);
  await page.goto("/");
  await appCame(page);
  const box = page.getByRole("combobox", { name: "Search casts, people and channels", exact: true });
  // Keep real keyboard/input events, but control the gaps: a busy host can
  // otherwise turn a requested 40 ms gap into a genuine 250 ms pause.
  await page.clock.pauseAt(await page.evaluate(() => Date.now() + 1000));
  const handles = await page.evaluate(() => {
    (window as any).__clockProbe = [];
    const note = (...args: unknown[]) => (window as any).__clockProbe.push(args);
    const timeout = setTimeout(note, 16, "cancelled timeout");
    clearInterval(timeout | 0);
    const interval = setInterval(note, 8, "cancelled interval");
    clearTimeout(interval | 0);
    const frame = requestAnimationFrame(() => note("cancelled frame"));
    cancelAnimationFrame(frame | 0);
    const live = setTimeout(note, 16, "kept", 7);
    return [timeout, interval, frame, live];
  });
  expect(new Set(handles).size).toBe(handles.length);
  expect(handles.every((id) => id > 0 && id === (id | 0))).toBe(true);
  await page.clock.runFor(15);
  expect(await page.evaluate(() => (window as any).__clockProbe)).toEqual([]);
  await page.clock.runFor(1);
  expect(await page.evaluate(() => (window as any).__clockProbe)).toEqual([["kept", 7]]);
  await box.evaluate((input: HTMLInputElement) => {
    (window as any).__searchInputs = [];
    input.addEventListener("input", (event) => {
      (window as any).__searchInputs.push({ value: input.value, trusted: event.isTrusted, at: Date.now() });
    });
  });
  for (const [i, character] of [..."fixt"].entries()) {
    if (i) await page.clock.runFor(40);
    await box.pressSequentially(character);
  }
  const inputs = await page.evaluate(() => (window as any).__searchInputs as { value: string; trusted: boolean; at: number }[]);
  expect(inputs.map((input) => input.value)).toEqual(["f", "fi", "fix", "fixt"]);
  expect(inputs.every((input) => input.trusted)).toBe(true);
  expect(inputs.slice(1).map((input, i) => input.at - inputs[i].at)).toEqual([40, 40, 40]);
  expect(queries).toEqual([]);
  await page.clock.runFor(249);
  expect(queries).toEqual([]);
  await page.clock.runFor(1);
  await expect.poll(() => queries).toEqual(["fixt"]);
  await page.clock.resume();
  const list = page.getByRole("listbox", { name: "Suggestions" });
  await expect(list).toBeVisible();
  // One look-up once typing pauses, not one per key.
  expect(queries).toEqual(["fixt"]);
  await expect(list.getByRole("group", { name: "People" }).getByRole("option")).toHaveCount(2);
  await expect(list.getByRole("option").first()).toContainText("Fixer One");
  await expect(list.getByRole("option").first()).toContainText("@fixer");
  await expect(list.getByRole("group", { name: "Channels" })).toContainText("/fixtures");
  await expect(list.getByRole("group", { name: "Posts" })).toContainText("A fixture cast about search");
  await expect(box).toHaveAttribute("aria-expanded", "true");

  // Arrow keys move through them; Enter opens the highlighted one.
  await box.press("ArrowDown");
  await box.press("ArrowDown");
  await expect(box).toHaveAttribute("aria-activedescendant", "suggestion-1");
  await expect(list.getByRole("option", { selected: true })).toContainText("Fixture Two");
  await box.press("Enter");
  await expect(page).toHaveURL(/\/u\/fixture\.eth$/);
  await expect(list).toBeHidden();
});

test("a click opens a suggestion, Enter alone opens all results, and Escape closes", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  const box = page.getByRole("combobox", { name: "Search casts, people and channels", exact: true });
  const list = page.getByRole("listbox", { name: "Suggestions" });
  await box.fill("fixture");
  await expect(list).toBeVisible();
  await box.press("Escape");
  await expect(list).toBeHidden();
  await expect(box).toHaveAttribute("aria-expanded", "false");

  await box.press("ArrowDown");
  await expect(list).toBeVisible();
  await list.getByRole("option", { name: /Fixtures/ }).click();
  await expect(page).toHaveURL(/\/c\/fixtures$/);

  // New words: the list may open under the pointer left on it, but nothing
  // is highlighted until the pointer moves, so Enter searches what is typed.
  await box.fill("fixture again");
  await expect(list).toBeVisible();
  await expect(list.getByRole("option", { selected: true })).toHaveCount(0);
  await box.press("Enter");
  await expect(page).toHaveURL(/\/search\?q=fixture%20again$/);
});
