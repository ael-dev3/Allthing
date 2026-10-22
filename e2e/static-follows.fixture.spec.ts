// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

/*
 * The static copy builds an account's followers in the browser, from the
 * node itself: its pages hold each of its two shards' next links, one
 * shard after the other, and the list comes in time order across them, to
 * its end, under /Allthing.
 */

test.skip(!process.env.E2E_PAGES, "the static copy, under /Allthing");

const NODE = "https://haatz.quilibrium.com";
const cors = { "access-control-allow-origin": "*" };
const base = 180_000_000;
type Follow = { fid: number; ts: number };
// Two shards whose follows interleave in time: 30 and 130 of them, so
// the first ends on the first read and the second goes on for two more.
const shards: Follow[][] = [
  Array.from({ length: 30 }, (_, i) => ({ fid: 1000 + i, ts: base - i * 1993 })),
  Array.from({ length: 130 }, (_, i) => ({ fid: 2000 + i, ts: base - 400 - i * 811 })),
];
const all = [...shards[0], ...shards[1]].sort((a, b) => b.ts - a.ts || a.fid - b.fid);
const rawUser = (fid: number) => ({
  fid, username: `u${fid}`, display_name: `User ${fid}`, profile: { bio: { text: `Bio ${fid}` } }, follower_count: fid % 50, following_count: 3,
});

async function serve(page: Page) {
  const reads: string[] = [];
  await page.route(`${NODE}/**`, (route) => {
    const url = new URL(route.request().url());
    if (url.pathname.endsWith("/user/by_username")) {
      return route.fulfill({ headers: cors, json: { user: { ...rawUser(42), username: "alice", display_name: "Alice", follower_count: 160, following_count: 0 } } });
    }
    if (url.pathname.endsWith("/user/bulk")) {
      const fids = (url.searchParams.get("fids") ?? "").split(",").filter(Boolean).map(Number);
      return route.fulfill({ headers: cors, json: { users: fids.map(rawUser) } });
    }
    if (url.pathname.endsWith("/v1/linksByTargetFid")) {
      // The node's paging: a JSON array of each shard's key, base64; `[]`
      // reads nothing more of a shard.
      const size = Number(url.searchParams.get("pageSize"));
      const token = url.searchParams.get("pageToken");
      reads.push(token ?? "");
      const keys: (number[] | null)[] = token ? JSON.parse(atob(token)) : shards.map(() => [0]);
      const messages: object[] = [];
      const next: (number[] | null)[] = [];
      shards.forEach((shard, i) => {
        const key = keys[i];
        if (!key || key.length === 0) {
          next.push(null);
          return;
        }
        const page = shard.slice(key[0], key[0] + size);
        messages.push(...page.map((f) => ({ data: { type: "MESSAGE_TYPE_LINK_ADD", fid: f.fid, timestamp: f.ts, linkBody: { type: "follow", targetFid: 42 } } })));
        next.push(page.length === size ? [key[0] + size] : null);
      });
      return route.fulfill({ headers: cors, json: { messages, nextPageToken: btoa(JSON.stringify(next)) } });
    }
    return route.fulfill({ status: 404, headers: cors, json: {} });
  });
  return reads;
}

test("the static copy lists an account's followers in time order across the node's shards, to the end", async ({ page }) => {
  const reads = await serve(page);
  await page.goto("/Allthing/u/alice/followers");
  const rows = page.locator(".people-list:not(.waiting) > .person-row");
  await expect(rows.first()).toBeVisible();
  // As tall as every follower read so far, and in time order.
  for (let i = 0; i < 20; i++) {
    await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
    await page.waitForTimeout(200);
    const listed = await page.locator(".people-list:not(.waiting)").evaluate((list) =>
      Math.round(list.getBoundingClientRect().height / list.querySelector(".person-row")!.getBoundingClientRect().height));
    // Drawn to the end too: a page that came during the wait grew the list
    // under a scroll that was at the old end.
    const last = rows.last();
    if (listed === all.length && (await last.getAttribute("aria-setsize")) === String(all.length) && (await last.getAttribute("aria-posinset")) === String(all.length)) break;
  }
  await expect(rows.last()).toHaveAttribute("aria-setsize", String(all.length));
  await expect(rows.last()).toHaveAttribute("aria-posinset", String(all.length));
  await expect(rows.last().locator(".person-row-handle")).toHaveText(`@u${all[all.length - 1].fid}`);
  // Every row drawn now is the one its place says, in time order.
  const shown = await rows.evaluateAll((list) => list.map((row) => [Number(row.getAttribute("aria-posinset")), row.querySelector(".person-row-handle")!.textContent]));
  for (const [place, handle] of shown) expect(handle).toBe(`@u${all[Number(place) - 1].fid}`);
  // The first shard ended first: the last read left it out with `[]`.
  expect(JSON.parse(atob(reads[reads.length - 1]))[0]).toEqual([]);
  // The way back to the profile stays under /Allthing.
  await expect(page.getByRole("link", { name: /^Back to Alice/ })).toHaveAttribute("href", "/Allthing/u/alice");
});
