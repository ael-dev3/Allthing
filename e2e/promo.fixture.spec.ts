// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";

/*
 * Pictures for the casts of Allthing's Farcaster account, @allthingapp, at
 * /promo/: the launch cast embeds /promo/launch-v6.png. A cast embeds its
 * URL, so it must answer as the PNG itself (not the app's page), and be
 * cached for a year, as a new picture gets a new name.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the account's pictures are served from allthing.social, not the static copy");

test("the launch picture is a 1920 × 1080 PNG, cached for a year", async ({ request }) => {
  const file = await request.get("/promo/launch-v6.png");
  expect(file.status()).toBe(200);
  expect(file.headers()["content-type"]).toBe("image/png");
  expect(file.headers()["cache-control"]).toContain("max-age=31536000");
  const png = await file.body();
  // The PNG signature, then the IHDR chunk: width and height, big-endian.
  expect([...png.subarray(0, 8)]).toEqual([137, 80, 78, 71, 13, 10, 26, 10]);
  expect(png.toString("latin1", 12, 16)).toBe("IHDR");
  expect([png.readUInt32BE(16), png.readUInt32BE(20)]).toEqual([1920, 1080]);
});
