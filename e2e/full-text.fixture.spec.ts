// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";
import { NOW } from "./fixture-time";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const long = `${"The first paragraph runs on well past where a title would stop. ".repeat(5)}\nA second line.\n\nThe last paragraph links https://example.com/page and ends here.`;
const casts = [
  { hash: `0x${"1".repeat(40)}`, author: { fid: 1, username: "fixture" }, text: long, ts: (NOW - 19), likes: 3 },
  { hash: `0x${"2".repeat(40)}`, author: { fid: 1, username: "fixture" }, text: "", ts: (NOW - 20), likes: 2,
    embeds: [{ kind: "image", url: "https://media.example/photo.png" }] },
];

test("feed cards show a cast's whole text, with its paragraphs, line breaks and links", async ({ page }) => {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home", (route) => route.fulfill({ json: { pool: casts, meta } }));
  await page.route("https://media.example/**", (route) => route.fulfill({ status: 404 }));
  await page.goto("/");
  const text = page.locator(`.cast-card[data-hash="${casts[0].hash}"] .cast-text`);
  await expect(text).toBeVisible();
  await expect(text.locator("p")).toHaveCount(2);
  await expect(text.locator("br")).toHaveCount(1);
  await expect(text).toContainText("A second line.");
  await expect(text).toContainText("and ends here.");
  expect(await text.innerText()).not.toContain("…");
  await expect(text.getByRole("link", { name: /example\.com\/page/ })).toBeVisible();
  // A cast with only media shows its media, with no placeholder text.
  const imageOnly = page.locator(`.cast-card[data-hash="${casts[1].hash}"]`);
  await expect(imageOnly.locator(".media-grid")).toBeVisible();
  await expect(imageOnly.locator(".cast-text")).toHaveCount(0);
  await expect(imageOnly).not.toContainText("[image]");
});
