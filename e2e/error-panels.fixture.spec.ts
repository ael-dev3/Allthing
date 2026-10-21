// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

/*
 * A page that can't load says what happened in a reader's words, never the
 * API's ("No such cast", "Farcaster data source is busy; backing off"), and
 * what they can do. A link to nothing and a search too long have no "Try
 * again", which couldn't help; a busy data source is tried again on its
 * own, three times in a row at most; offline or a failing server has a
 * button. The page's title says what happened too.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const cast = { hash: hex(1), author: alice, text: "A cast that loads in the end", ts: 1792235681, likes: 1, recasts: 0, replies: 0 };

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
}

const panel = (page: Page) => page.locator(".error-panel");

/** What a panel shows: its heading and line, its buttons, and the page's title. */
async function says(page: Page) {
  await expect(panel(page)).toBeVisible();
  return {
    heading: await panel(page).locator("h1").innerText(),
    line: await panel(page).locator("p").first().innerText(),
    buttons: await panel(page).getByRole("button").allInnerTexts(),
    title: await page.title(),
  };
}

test("a link to nothing says what's missing, with no Try again", async ({ page }) => {
  await serve(page);
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ status: 404, json: { error: "No such cast" } }));
  await page.route("**/api/v1/user?*", (route) => route.fulfill({ status: 404, json: { error: "No such user" } }));
  await page.route("**/api/v1/channel?*", (route) => route.fulfill({ status: 404, json: { error: "No such channel" } }));
  await page.goto(`/p/${hex(9)}`);
  expect(await says(page)).toEqual({
    heading: "No such cast", line: "It doesn't exist, or it was deleted.", buttons: [], title: "Cast not found · Allthing",
  });
  await page.goto("/u/nobody-here");
  expect(await says(page)).toEqual({
    heading: "No such account", line: "No Farcaster account goes by that name.", buttons: [], title: "Account not found · Allthing",
  });
  await page.goto("/c/nowhere");
  expect(await says(page)).toEqual({
    heading: "No such channel", line: "No Farcaster channel goes by that name.", buttons: [], title: "Channel not found · Allthing",
  });
  await expect(panel(page).getByRole("link", { name: "Back to the front page" })).toBeVisible();
});

test("a search too long says so plainly, with no Try again", async ({ page }) => {
  await serve(page);
  // The Worker itself turns it down, before asking anyone.
  let asked = 0;
  await page.route("**/api/v1/search?*", (route) => {
    asked += 1;
    return route.continue();
  });
  await page.goto(`/search?q=${"farcaster ".repeat(12).trim()}`);
  expect(await says(page)).toEqual({
    heading: "That search is too long", line: "Try it with fewer words.", buttons: [], title: "Search too long · Allthing",
  });
  await expect(panel(page)).not.toContainText("Something went wrong");
  await expect(panel(page)).not.toContainText("query");
  expect(asked).toBe(1);
});

test("a busy data source is tried again on its own, and the page loads", async ({ page }) => {
  await serve(page);
  let asked = 0;
  await page.route("**/api/v1/cast?*", (route) => {
    asked += 1;
    return asked === 1
      ? route.fulfill({ status: 503, headers: { "retry-after": "2" }, json: { error: "Farcaster data source is busy; backing off", retry_after: 2 } })
      : route.fulfill({ json: { thread: { cast, children: [] }, notes: {}, meta } });
  });
  await page.goto(`/p/${cast.hash}`);
  expect(await says(page)).toEqual({
    heading: "Farcaster's data is busy",
    line: "Many people are reading at once. Allthing tries again in a few seconds.",
    buttons: ["Try again now"],
    title: "Busy · Allthing",
  });
  await expect(panel(page)).not.toContainText("backing off");
  // No click: it comes on its own.
  await expect(page.locator(".post .post-text")).toHaveText(cast.text, { timeout: 6000 });
  expect(asked).toBe(2);
});

test("a data source that stays busy is tried three times, then left to the reader", async ({ page }) => {
  test.setTimeout(60_000);
  await serve(page);
  let asked = 0;
  await page.route("**/api/v1/cast?*", (route) => {
    asked += 1;
    return route.fulfill({ status: 503, headers: { "retry-after": "2" }, json: { error: "Farcaster data source is busy; backing off", retry_after: 2 } });
  });
  await page.goto(`/p/${cast.hash}`);
  // The first answer, and three tries on its own.
  await expect.poll(() => asked, { timeout: 20_000 }).toBe(4);
  await expect(panel(page).getByRole("button")).toHaveText("Try again");
  await expect(panel(page).locator("p").first()).toHaveText("Many people are reading at once. Try again in a minute.");
  await page.waitForTimeout(3000);
  expect(asked).toBe(4);
  // The reader's own try still asks.
  await panel(page).getByRole("button", { name: "Try again" }).click();
  await expect.poll(() => asked).toBe(5);
});

test("offline, or a failing server: a Try again that works once it can", async ({ page }) => {
  await serve(page);
  let mode: "offline" | "failing" | "fine" = "offline";
  await page.route("**/api/v1/cast?*", (route) => {
    if (mode === "offline") return route.abort("internetdisconnected");
    if (mode === "failing") return route.fulfill({ status: 502, json: { error: "upstream timed out", retry_after: 10 } });
    return route.fulfill({ json: { thread: { cast, children: [] }, notes: {}, meta } });
  });
  await page.goto(`/p/${cast.hash}`);
  expect(await says(page)).toEqual({
    heading: "Can't reach Allthing", line: "Check your connection, then try again.", buttons: ["Try again"], title: "Offline · Allthing",
  });
  await expect(panel(page)).not.toContainText("Network error");
  mode = "failing";
  await panel(page).getByRole("button", { name: "Try again" }).click();
  expect(await says(page)).toEqual({
    heading: "Something went wrong",
    line: "Allthing couldn't load this just now. Try again in a moment.",
    buttons: ["Try again"],
    title: "Couldn't load · Allthing",
  });
  await expect(panel(page)).not.toContainText("upstream");
  mode = "fine";
  await panel(page).getByRole("button", { name: "Try again" }).click();
  await expect(page.locator(".post .post-text")).toHaveText(cast.text);
});
