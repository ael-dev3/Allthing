// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

async function searches(page: Page) {
  const requests: string[] = [];
  let release!: () => void;
  const pending = new Promise<void>((resolve) => { release = resolve; });
  const meta = { backend: "hypersnap", fetched_at: Date.now() };
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture" } }));
  await page.route("**/api/v1/home**", (route) => route.fulfill({ json: { pool: [], meta } }));
  await page.route("**/api/v1/search?*", async (route) => {
    const q = new URL(route.request().url()).searchParams.get("q")!;
    requests.push(q);
    if (q === "beta") await pending;
    await route.fulfill({ json: {
      query: q, users: [{ fid: 1, username: q, display_name: `${q} person` }], channels: [], casts: [], meta,
    } });
  });
  await page.goto("/");
  const box = page.getByRole("combobox", { name: "Search casts, people and channels", exact: true });
  const list = page.getByRole("listbox", { name: "Suggestions" });
  await box.fill("alpha");
  await expect(list).toBeVisible();
  await expect(list.getByRole("option").first()).toContainText("alpha person");
  await box.fill("beta");
  await expect.poll(() => requests.includes("beta")).toBe(true);
  return { box, list, release };
}

for (const width of [1280, 390]) {
  test(`typing different words hides suggestions for the previous search at width ${width}`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 844 });
    const { list, release } = await searches(page);
    try {
      await page.screenshot({ path: info.outputPath(`search-pending-${width}.png`) });
      await expect(list).toBeHidden({ timeout: 2000 });
    } finally { release(); }
  });

  test(`Enter cannot choose a previous query while the next lookup is pending at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    const { box, release } = await searches(page);
    try {
      await box.press("ArrowDown");
      await box.press("Enter");
      await expect(page).toHaveURL(/\/search\?q=beta$/, { timeout: 2000 });
    } finally { release(); }
  });

  test(`Escape dismisses an in-flight lookup at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    const { box, list, release } = await searches(page);
    await box.press("Escape");
    await expect(list).toBeHidden();
    release();
    // Let the actual response settle before checking that it stayed closed.
    await expect.poll(() => page.evaluate(() => performance.getEntriesByType("resource")
      .some((entry) => entry.name.includes("/api/v1/search?q=beta")))).toBe(true);
    await page.waitForTimeout(100);
    await expect(box).toHaveValue("beta");
    await expect(list).toBeHidden({ timeout: 2000 });
    await expect(box).toHaveAttribute("aria-expanded", "false");
    // An explicit return to the box can ask again and reuse the cached
    // response, without an old request reopening the dismissed list.
    await page.getByRole("link", { name: "Allthing, home", exact: true }).focus();
    await box.focus();
    await expect(list.getByRole("option").first()).toContainText("beta person");
  });
}
