// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

for (const parent of ["https://example.com/a%2Fb?tag=100%25", "https://example.com/100%25"]) {
  test(`channel navigation preserves parent URL ${parent}`, async ({ page }) => {
    const ids: string[] = [];
    await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
    await page.route("**/api/v1/channel?*", (route) => {
      ids.push(new URL(route.request().url()).searchParams.get("id")!);
      return route.fulfill({ json: {
        channel: { id: parent, name: "Encoded parent", parent_url: parent }, pool: [],
        meta: { backend: "hypersnap", fetched_at: Date.now() },
      } });
    });
    await page.goto(`/c?id=${encodeURIComponent(parent)}`);
    await expect(page.getByRole("heading", { name: "/Encoded parent", exact: true })).toBeVisible();
    expect(ids).toEqual([parent]);
    await page.getByRole("navigation", { name: "Sort", exact: true }).getByRole("link", { name: "new", exact: true }).click();
    await expect(page).toHaveURL(/sort=new/);
    expect(new URL(page.url()).searchParams.get("id")).toBe(parent);
    expect(ids).toEqual([parent]);
  });
}

for (const query of ["literal%2Fpath", "100%25", "C++ & Rust"]) {
  test(`search preserves the literal query ${query}`, async ({ page }) => {
    const queries: string[] = [];
    await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
    await page.route("**/api/v1/search?*", (route) => {
      queries.push(new URL(route.request().url()).searchParams.get("q")!);
      return route.fulfill({ json: {
        query, users: [], channels: [], casts: [], meta: { backend: "hypersnap", fetched_at: Date.now() },
      } });
    });
    await page.goto("/search");
    const input = page.getByRole("combobox", { name: "Search casts, people and channels", exact: true });
    await input.fill(query);
    await input.press("Enter");
    await expect(page).toHaveURL(/\/search\?q=/);
    expect(new URL(page.url()).searchParams.get("q")).toBe(query);
    await expect.poll(() => queries).toEqual([query]);
    await expect(input).toHaveValue(query);
    await expect(page.getByRole("heading", { level: 1 })).toHaveText(`Results for “${query}”`);
  });
}

test("channel-directory search preserves literal percent escapes", async ({ page }) => {
  const query = "art%20club";
  const queries: string[] = [];
  await page.route("**/api/v1/channels?*", (route) => {
    queries.push(new URL(route.request().url()).searchParams.get("q")!);
    return route.fulfill({ json: { channels: [], meta: { backend: "hypersnap", fetched_at: Date.now() } } });
  });
  await page.goto(`/channels?q=${encodeURIComponent(query)}`);
  await expect(page.getByRole("searchbox", { name: "Search channels", exact: true })).toHaveValue(query);
  expect(queries).toEqual([query]);
});
