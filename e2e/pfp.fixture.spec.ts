// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";

/*
 * The logo as a PNG at /pfp.png, for the picture of Allthing's Farcaster
 * account, @allthingapp: apps show an SVG picture as blank. It is square and
 * opaque, the logo's ground to its corners, with the "A" clear of the circle
 * apps crop a picture to, and cached for a year.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the account's picture is served from allthing.social, not the static copy");

const SIZE = 512;
const GROUND = [0xf6, 0xf4, 0xee];
const INK = [0x1b, 0x22, 0x33];

test("the account's picture is a 512 × 512 PNG, cached for a year", async ({ request }) => {
  const file = await request.get("/pfp.png");
  expect(file.status()).toBe(200);
  expect(file.headers()["content-type"]).toBe("image/png");
  expect(file.headers()["cache-control"]).toContain("max-age=31536000");
  const png = await file.body();
  // The PNG signature, then the IHDR chunk: width and height, big-endian.
  expect([...png.subarray(0, 8)]).toEqual([137, 80, 78, 71, 13, 10, 26, 10]);
  expect(png.toString("latin1", 12, 16)).toBe("IHDR");
  expect([png.readUInt32BE(16), png.readUInt32BE(20)]).toEqual([SIZE, SIZE]);
});

test("it is opaque, the ground to its corners, with the A in the middle and clear of the circle", async ({ page }) => {
  // The picture's own page, so a canvas may read it (same origin).
  await page.goto("/pfp.png");
  const seen = await page.evaluate(
    async ({ size, ground, ink }) => {
      const img = new Image();
      img.src = location.href;
      await img.decode();
      const ctx = new OffscreenCanvas(size, size).getContext("2d")!;
      ctx.drawImage(img, 0, 0);
      const data = ctx.getImageData(0, 0, size, size).data;
      const at = (x: number, y: number) => Array.from(data.subarray((y * size + x) * 4, (y * size + x) * 4 + 4));
      const near = (pixel: number[], colour: number[]) => colour.every((c, i) => Math.abs(pixel[i] - c) <= 2);
      let translucent = 0;
      let inkInMiddle = 0;
      let offGroundNearRim = 0;
      for (let y = 0; y < size; y++) {
        for (let x = 0; x < size; x++) {
          const pixel = at(x, y);
          if (pixel[3] !== 255) translucent++;
          // The middle 128 × 128 px, where the A's strokes and bar cross.
          if (Math.abs(x + 0.5 - size / 2) < size / 8 && Math.abs(y + 0.5 - size / 2) < size / 8 && near(pixel, ink)) inkInMiddle++;
          // Apps crop the picture to a circle: the A keeps 32 px (1/16) clear of its edge.
          if (Math.hypot(x + 0.5 - size / 2, y + 0.5 - size / 2) > size / 2 - size / 16 && !near(pixel, ground)) offGroundNearRim++;
        }
      }
      const corners = [at(0, 0), at(size - 1, 0), at(0, size - 1), at(size - 1, size - 1)];
      return {
        natural: [img.naturalWidth, img.naturalHeight],
        translucent,
        cornersOnGround: corners.every((pixel) => pixel[3] === 255 && near(pixel, ground)),
        inkInMiddle,
        offGroundNearRim,
      };
    },
    { size: SIZE, ground: GROUND, ink: INK },
  );
  expect(seen.natural).toEqual([SIZE, SIZE]);
  expect(seen.translucent).toBe(0);
  expect(seen.cornersOnGround).toBe(true);
  // The glyph: a good part of the middle is its ink.
  expect(seen.inkInMiddle).toBeGreaterThan((SIZE / 4) ** 2 / 10);
  expect(seen.offGroundNearRim).toBe(0);
});
