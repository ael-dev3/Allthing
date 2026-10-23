// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { NOW } from "./fixture-time";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = (NOW - 19);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const author = (fid: number) => ({ fid, username: `u${fid}`, display_name: `User ${fid}` });
// 0x01 quotes 0x02, which quotes 0x03, which quotes 0x04, which quotes 0x05.
const quoting = (hash: number, quoted: number, fid: number) => ({
  hash: hex(hash), author: author(fid), text: `cast ${hash}`, ts,
  embeds: [{ kind: "cast", hash: hex(quoted), fid: fid + 1 }],
});

async function serve(page: Page) {
  const top = { hash: hex(1), author: author(1), text: "top", ts, embeds: [{ kind: "cast", hash: hex(2), fid: 2 }] };
  const quotes = { [hex(2)]: quoting(2, 3, 2), [hex(3)]: quoting(3, 4, 3), [hex(4)]: quoting(4, 5, 4), [hex(5)]: { hash: hex(5), author: author(5), text: "deepest", ts } };
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [top], notes: { users: {}, quotes }, meta } });
  });
}

test("a quote of a quote shows inline, to three levels, then a link", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  const card = page.locator(".cast-card:not(.skeleton)").first();
  await expect(card).toBeVisible();
  // Three nested quote cards inside the post, the third holding a link.
  await expect(card.locator(".quote")).toHaveCount(3);
  await expect(card.locator(".quote .quote .quote .quote")).toHaveCount(0);
  await expect(card.locator(".quote .quote .quote .quote-chip")).toHaveText("↳ quotes a cast");
});
