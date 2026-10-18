// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, type Locator } from "@playwright/test";

// Locator click/tap may scroll before input even when a control is visible.
// Dispatch at its visible point when the test needs to preserve the viewport.
export async function activateVisible(control: Locator, touch = false) {
  await expect(control).toBeVisible();
  const box = await control.boundingBox();
  const page = control.page();
  const viewport = page.viewportSize();
  expect(box).not.toBeNull();
  expect(viewport).not.toBeNull();
  const x = box!.x + box!.width / 2, y = box!.y + box!.height / 2;
  expect(x).toBeGreaterThanOrEqual(0);
  expect(x).toBeLessThan(viewport!.width);
  expect(y).toBeGreaterThanOrEqual(0);
  expect(y).toBeLessThan(viewport!.height);
  if (touch) await page.touchscreen.tap(x, y);
  else await page.mouse.click(x, y);
}
