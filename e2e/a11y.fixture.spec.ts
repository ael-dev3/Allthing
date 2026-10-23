// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";
import { NOW } from "./fixture-time";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const hash = `0x${"1".repeat(40)}`;
const cast = { hash, author: { fid: 2, username: "fixture", display_name: "Fixture Author" }, text: "A post", ts: (NOW - 19), likes: 3 };

test.beforeEach(async ({ page }) => {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: {
    thread: { cast },
    notes: {
      origins: { [hash]: { signer: `0x${"a".repeat(64)}`, app_fid: 9152 } },
      apps: { "9152": { fid: 9152, username: "warpcast", display_name: "Farcaster" } },
    },
    meta,
  } }));
});

/** WCAG contrast ratio of two CSS colors as the page computes them. */
const contrast = (fg: string, bg: string) => {
  const lum = (css: string) => {
    const [r, g, b] = css.match(/\d+(\.\d+)?/g)!.slice(0, 3).map((v) => Number(v) / 255);
    const f = (c: number) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
    return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
  };
  const [hi, lo] = [lum(fg), lum(bg)].sort((a, b) => b - a);
  return (hi + 0.05) / (lo + 0.05);
};

test("a post page has one level-one heading, naming the author", async ({ page }) => {
  await page.goto(`/p/${hash}`);
  await expect(page.locator(".post .post-text")).toHaveText("A post");
  await expect(page.locator("h1")).toHaveCount(1);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Cast by Fixture Author");
});

test("muted text is readable on every surface it sits on", async ({ page }) => {
  await page.goto(`/p/${hash}`);
  await expect(page.locator(".post")).toBeVisible();
  const colors = await page.evaluate(() => {
    const style = getComputedStyle(document.documentElement);
    const resolve = (value: string) => {
      const probe = document.createElement("span");
      probe.style.color = value;
      document.body.append(probe);
      const color = getComputedStyle(probe).color;
      probe.remove();
      return color;
    };
    return {
      muted: resolve(style.getPropertyValue("--ink-3")),
      surfaces: ["--card", "--paper", "--paper-2"].map((name) => resolve(style.getPropertyValue(name))),
    };
  });
  for (const surface of colors.surfaces) expect(contrast(colors.muted, surface)).toBeGreaterThanOrEqual(4.5);
});

test("the posting app's link is underlined, not told apart by color alone", async ({ page }) => {
  await page.goto(`/p/${hash}`);
  const link = page.locator(".post-via a").first();
  await expect(link).toBeVisible();
  expect(await link.evaluate((a) => getComputedStyle(a).textDecorationLine)).toContain("underline");
  // The posting app is named once: the feed's "via" chip isn't repeated here.
  expect((await page.locator(".post .post-meta").innerText()).match(/\bvia\b/g)).toHaveLength(1);
});
