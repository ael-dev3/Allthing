// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses viewport prefetches in the Worker build");

for (const fail of [false, true]) {
  test(`queued profile facts resume when foreground requests ${fail ? "fail" : "finish"}`, async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 900 });
    const meta = { backend: "hypersnap", fetched_at: Date.now() };
    const user = { fid: 42, username: "alice", display_name: "Alice", bio: "Reading conversations.",
      follower_count: 100, following_count: 20, eth_addresses: ["0x1234"] };
    const pending: (() => Promise<void>)[] = [];
    const facts: string[] = [];
    await page.route("**/api/v1/**", route => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
    await page.route("**/api/v1/user?*", route => route.fulfill({ json: { user, pool: [], notes: {}, meta } }));
    for (const kind of ["vote", "wallets"]) {
      await page.route(`**/api/v1/${kind}?*`, route => {
        const fid = new URL(route.request().url()).searchParams.get("fid");
        const fulfill = () => route.fulfill({ json: kind === "vote" ? { fid: Number(fid), onchain: "stay", meta }
          : { fid: Number(fid), addresses: [], chains: [], total: 0, sources: [], fetched_at: Date.now() } });
        if (fid === "42") pending.push(() => fail ? route.abort() : fulfill());
        else { facts.push(`${kind}:${fid}`); return fulfill(); }
      });
    }
    await page.route("**/api/v1/home**", route => route.fulfill({ json: { meta, notes: {}, pool: [{
      hash: `0x${"a".repeat(40)}`, author: { fid: 43, username: "bob", display_name: "Bob" },
      text: "A quiet cast whose author card will be ready.", ts: Math.floor(Date.now() / 1000),
      likes: 0, recasts: 0, replies: 0,
    }] } }));
    await page.goto("/u/alice");
    await expect.poll(() => pending.length).toBe(2);
    await page.getByRole("link", { name: "Allthing, home", exact: true }).click();
    await expect(page.locator(".cast-card").first()).toBeVisible();
    // Let the card's observer enqueue both facts while foreground reads hold
    // the slots. The card's zero counts request no engagement batch.
    await page.waitForTimeout(1200);
    expect(facts).toEqual([]);
    await Promise.all(pending.map(send => send()));
    await expect.poll(() => facts.sort(), { timeout: 3000 }).toEqual(["vote:43", "wallets:43"]);
  });
}
