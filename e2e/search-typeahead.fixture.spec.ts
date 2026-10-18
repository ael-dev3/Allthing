// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const people = [
  { fid: 2, username: "fixer", display_name: "Fixer One", pfp_url: "https://media.example/a.png", follower_count: 10 },
  { fid: 3, username: "fixture.eth", display_name: "Fixture Two", follower_count: 5 },
];
const channels = [{ id: "fixtures", name: "Fixtures", parent_url: "https://farcaster.xyz/~/channel/fixtures" }];
const casts = [{ hash: `0x${"c".repeat(40)}`, author: { fid: 2, username: "fixer" }, text: "A fixture cast about search", ts: 1792235681 }];

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

test("suggestions appear as you type: people, channels and posts", async ({ page }) => {
  const queries = await serve(page);
  await page.goto("/");
  const box = page.getByRole("combobox", { name: "Search casts, people and channels", exact: true });
  await box.pressSequentially("fixt", { delay: 40 });
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
