// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

/*
 * Counts in view stay current: every 30 s while the reader is active, the
 * cards and the post in view ask `/api/v1/counts` again, and their likes,
 * recasts and replies change in place, the card keeping its height. An open
 * post with replies its read doesn't list offers them as a button over the
 * page, and reads the post again when it is pressed.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const post = { hash: hex(1), author: alice, text: "Counting along", ts, likes: 3, recasts: 1, replies: 1 };
const reply = (n: number) => ({ cast: { hash: hex(100 + n), author: alice, text: `Reply ${n}`, ts: ts + n, parent_hash: post.hash }, children: [] });

for (const [name, viewport] of [["at 1280", { width: 1280, height: 900 }], ["at 390", { width: 390, height: 844 }]] as const) {
  test.describe(name, () => {
    test.use({ viewport });

    async function serve(page: Page) {
      const state = { likes: 3, replies: 1, asked: 0 };
      await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
      await page.route("**/api/v1/home**", (route) => route.fulfill({ json: { pool: [post], notes: {}, meta } }));
      await page.route("**/api/v1/cast?*", (route) => {
        const children = Array.from({ length: state.replies }, (_, i) => reply(i + 1));
        return route.fulfill({ json: { thread: { cast: { ...post, likes: state.likes, replies: state.replies }, children }, notes: {}, meta } });
      });
      await page.route("**/api/v1/counts?*", (route) => {
        state.asked += 1;
        const casts = new URL(route.request().url()).searchParams.get("casts")!.split(",");
        const counts = Object.fromEntries(
          casts.filter((h) => h === post.hash).map((h) => [h, { likes: state.likes, recasts: 1, replies: state.replies }]),
        );
        return route.fulfill({ json: { counts, meta } });
      });
      return state;
    }

    test("a feed card's counts change in place while it is in view", async ({ page }) => {
      await page.clock.install();
      const state = await serve(page);
      await page.goto("/");
      const card = page.locator(`.cast-card[data-hash="${post.hash}"]`);
      await expect(card.locator(".stat-likes .n")).toHaveText("3");
      const before = await card.boundingBox();
      // A page just read is as fresh: nothing is asked for at once.
      expect(state.asked).toBe(0);
      state.likes = 12;
      await page.mouse.move(10, 10);
      await page.clock.fastForward("00:31");
      await expect(card.locator(".stat-likes .n")).toHaveText("12");
      expect(state.asked).toBeGreaterThan(0);
      expect((await card.boundingBox())!.height).toBe(before!.height);
    });

    test("an open post's counts follow, and new replies wait for the reader", async ({ page }) => {
      await page.clock.install();
      const state = await serve(page);
      await page.goto(`/p/${post.hash}`);
      await expect(page.getByText("Reply 1")).toBeVisible();
      state.likes = 5;
      state.replies = 3;
      await page.mouse.move(10, 10);
      await page.clock.fastForward("00:31");
      const main = page.locator(".post").first();
      await expect(main.locator(".stat-likes .n")).toHaveText("5");
      const more = page.getByRole("button", { name: "2 new replies" });
      await expect(more).toBeVisible();
      // Offered, not pushed in.
      await expect(page.getByText("Reply 3")).toHaveCount(0);
      await more.click();
      await expect(page.getByText("Reply 3")).toBeVisible();
      await expect(more).toHaveCount(0);
    });

    test("an idle tab stops asking", async ({ page }) => {
      await page.clock.install();
      const state = await serve(page);
      await page.goto("/");
      await expect(page.locator(`.cast-card[data-hash="${post.hash}"]`)).toBeVisible();
      await page.clock.fastForward("03:00");
      const asked = state.asked;
      await page.clock.fastForward("05:00");
      expect(state.asked).toBe(asked);
    });
  });
}
