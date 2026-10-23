// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { isoAgo, NOW } from "./fixture-time";

/*
 * Seen casts (seen.rs, allthing_core::seen): a front page card at least half
 * in view for a second is recorded in localStorage (`allthing.seen`), signed
 * out too. A later visit dims the casts seen before, with "Seen" in their
 * meta row, and in hot lists them after the rest; top and new keep their
 * order. Nothing is left out. The account menu's switch turns it off.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const user = (fid: number, username: string) => ({ fid, username, display_name: username });
const hash = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
// Cast 1 has the most points, so it leads hot and top until it's seen.
const cast = (n: number) => ({
  hash: hash(n), author: user(100 + n, `caster${n}`), text: `Fixture cast number ${n}`, ts: NOW - 60 - n * 60, timestamp: isoAgo(60 + n * 60), likes: 50 - n, recasts: 0, replies: 0,
});
const pool = [1, 2, 3, 4].map(cast);
const me = { fid: 1234, username: "reader", display_name: "Reader" };

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : pool, notes: {}, meta } });
  });
}

/** Seen before this visit: written before the app starts. */
async function seenBefore(page: Page, hashes: string[], marks = true) {
  await page.addInitScript(
    ([stored, on]) => {
      if (sessionStorage.getItem("seeded")) return;
      sessionStorage.setItem("seeded", "1");
      localStorage.setItem("allthing.seen", stored as string);
      if (!on) localStorage.setItem("allthing.seen-marks", "0");
    },
    [hashes.join(" "), marks] as const,
  );
}

const order = (page: Page) => page.locator(".cast-list .cast-card").evaluateAll((cards) => cards.map((c) => c.getAttribute("data-hash")));

const sizes = [
  { name: "1280", use: { viewport: { width: 1280, height: 900 } } },
  { name: "390", use: { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true } },
];

for (const size of sizes) {
  test.describe(size.name, () => {
    test.use(size.use);

    test("hot lists casts seen before after the rest, dimmed and marked, never hidden", async ({ page }, info) => {
      await serve(page);
      await seenBefore(page, [hash(1)]);
      await page.goto("/");
      const seen = page.locator(`.cast-card[data-hash="${hash(1)}"]`);
      await expect(seen).toBeVisible();
      await expect(seen).toHaveClass(/\bseen\b/);
      await expect(seen.locator(".seen-mark")).toHaveText("Seen");
      await expect.poll(() => order(page)).toEqual([hash(2), hash(3), hash(4), hash(1)]);
      await expect(page.locator(`.cast-card[data-hash="${hash(2)}"]`)).not.toHaveClass(/\bseen\b/);
      await expect(page.locator(".cast-card .seen-mark")).toHaveCount(1);
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(size.use.viewport.width);
      await page.screenshot({ path: info.outputPath(`seen-${size.name}.png`) });

      // Top keeps its order, and still marks it.
      await page.goto("/?sort=top");
      await expect.poll(() => order(page)).toEqual([hash(1), hash(2), hash(3), hash(4)]);
      await expect(page.locator(`.cast-card[data-hash="${hash(1)}"]`)).toHaveClass(/\bseen\b/);
    });

    test("cards in view for a second are recorded, and marked on the next visit", async ({ page }) => {
      await serve(page);
      await page.goto("/");
      const first = page.locator(`.cast-card[data-hash="${hash(1)}"]`);
      await expect(first).toBeVisible();
      // Not marked this visit: nothing moves while the reader is here.
      await expect(first).not.toHaveClass(/\bseen\b/);
      await expect
        .poll(() => page.evaluate(() => localStorage.getItem("allthing.seen") || ""), { timeout: 10_000 })
        .toContain(hash(1));
      await expect(first).not.toHaveClass(/\bseen\b/);
      await page.reload();
      await expect(page.locator(`.cast-card[data-hash="${hash(1)}"]`)).toHaveClass(/\bseen\b/);
    });

    test("turned off, nothing is marked, moved or recorded", async ({ page }) => {
      await serve(page);
      await seenBefore(page, [hash(1)], false);
      await page.goto("/");
      await expect(page.locator(".cast-card").first()).toBeVisible();
      await expect.poll(() => order(page)).toEqual([hash(1), hash(2), hash(3), hash(4)]);
      await expect(page.locator(".cast-card.seen")).toHaveCount(0);
      await page.waitForTimeout(3500);
      expect(await page.evaluate(() => localStorage.getItem("allthing.seen"))).toBe(hash(1));
    });
  });
}

test("signed in, the account menu has the switch", async ({ page }) => {
  await serve(page);
  await page.route("**/api/v1/following**", (route) => route.fulfill({ json: { casts: pool, notes: {}, meta } }));
  await seenBefore(page, [hash(1)]);
  await page.addInitScript((user) => {
    document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax`;
  }, me);
  await page.goto("/?feed=everyone");
  await expect(page.locator(`.cast-card[data-hash="${hash(1)}"]`)).toHaveClass(/\bseen\b/);
  const avatar = page.getByRole("link", { name: "Signed in as @reader: your profile" });
  await (test.info().project.name === "phone" ? avatar.tap() : avatar.hover());
  const toggle = page.getByRole("dialog", { name: "Your account" }).getByRole("switch", { name: "Mark seen casts" });
  await expect(toggle).toHaveAttribute("aria-checked", "true");
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-checked", "false");
  await expect(page.locator(".cast-card.seen")).toHaveCount(0);
  expect(await page.evaluate(() => localStorage.getItem("allthing.seen-marks"))).toBe("0");
});
