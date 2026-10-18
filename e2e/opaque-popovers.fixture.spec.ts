// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const hash = `0x${"a".repeat(40)}`;
const meta = { backend: "hypersnap", fetched_at: Date.now() };

test.beforeEach(async ({ page }) => {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home", (route) => route.fulfill({ json: {
    pool: [{ hash, author: { fid: 1, username: "fixture" }, text: "Popover fixture", ts: 1792235681, likes: 10 }], meta,
  } }));
  await page.route("**/api/v1/reactions?*", (route) => route.fulfill({ json: {
    hash, kind: "likes", users: [{ fid: 2, username: "reader" }], more: false, meta,
  } }));
});

test("popovers are solid, so nothing behind them shows through", async ({ page, hasTouch }) => {
  await page.goto("/");
  const trigger = page.getByRole("button", { name: "10 likes: show who", exact: true });
  if (hasTouch) await trigger.tap();
  else await trigger.hover();
  const panel = page.getByRole("dialog", { name: "Liked by", exact: true });
  await expect(panel.getByRole("link")).toBeVisible();
  // A see-through, blurred background let Chrome draw an image from the
  // card above over the list.
  const style = await panel.evaluate((p) => {
    const s = getComputedStyle(p);
    return { background: s.backgroundColor, backdrop: s.backdropFilter };
  });
  expect(style).toEqual({ background: "rgb(255, 255, 255)", backdrop: "none" });
});
