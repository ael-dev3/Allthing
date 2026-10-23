// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";
import { NOW } from "./fixture-time";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const hash = `0x${"a".repeat(40)}`;
const meta = { backend: "hypersnap", fetched_at: Date.now() };

test.beforeEach(async ({ page }) => {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home", (route) => route.fulfill({ json: {
    pool: [{ hash, author: { fid: 1, username: "fixture" }, text: "Popover fixture", ts: (NOW - 19), likes: 10 }], meta,
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

test("on a post page the likes popover sits above the replies, with no native tooltip over it", async ({ page, hasTouch }) => {
  test.skip(hasTouch, "hover popover");
  const reply = (n: number) => ({ cast: { hash: `0x${String(n).repeat(40)}`, author: { fid: 3, username: "replier" }, text: `Reply ${n} `.repeat(30), ts: (NOW - 10) } });
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
    thread: { cast: { hash, author: { fid: 1, username: "fixture" }, text: "Popover fixture", ts: (NOW - 19), likes: 10, replies: 3 }, children: [reply(1), reply(2), reply(3)] },
    meta,
  } }));
  await page.goto(`/p/${hash}`);
  await expect(page.locator(".replies .comment").first()).toBeVisible();
  const foot = page.locator(".post .engagement");
  expect(await foot.getAttribute("title")).toBeNull();
  await page.getByRole("button", { name: "10 likes: show who", exact: true }).hover();
  const panel = page.getByRole("dialog", { name: "Liked by", exact: true });
  await expect(panel.getByRole("link")).toBeVisible();
  // Solid, with no blur, so nothing behind shows through.
  const bg = await panel.evaluate((p) => getComputedStyle(p).backgroundColor);
  expect(bg).toBe("rgb(255, 255, 255)");
  // Open below the post's footer, over the replies: the panel is what's on top.
  const box = (await panel.boundingBox())!;
  const top = await page.evaluate(([x, y]) => document.elementFromPoint(x, y)?.closest(".popover-panel") !== null, [box.x + box.width / 2, box.y + box.height - 6]);
  expect(top).toBe(true);
});
