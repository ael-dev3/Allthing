// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";
import { NOW } from "./fixture-time";

/*
 * The media viewer's caption is who posted the cast and the cast's words,
 * "@alice · Two photos from the walk", cut to two lines, beside "1 of 2"
 * when there are several. The generated alt text ("Image 1 of 2 posted by
 * @alice: …") stays on the image, for screen readers, and isn't shown.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = (NOW - 19);
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64");
const image = (name: string) => ({ kind: "image", url: `https://imagedelivery.net/fixture/${name}/original` });
const two = { hash: hex(1), author: alice, text: "Two photos from the walk", ts, likes: 1, replies: 0, embeds: [image("first"), image("second")] };
const long = {
  hash: hex(2), author: alice, ts, likes: 1, replies: 0, embeds: [image("long")],
  text: "A long caption that goes on and on about the walk, the hills, the weather and the dog, ".repeat(4).trim(),
};
const wordless = { hash: hex(3), author: alice, text: "", ts, likes: 1, replies: 0, embeds: [image("wordless")] };

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/cast?*", (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash");
    const cast = [two, long, wordless].find((c) => c.hash === hash)!;
    return route.fulfill({ json: { thread: { cast, children: [] }, notes: {}, meta } });
  });
  await page.route("https://imagedelivery.net/**", (route) => route.fulfill({ contentType: "image/png", body: png }));
}

async function open(page: Page, hash: string, tile = 0) {
  await page.goto(`/p/${hash}`);
  await page.locator(".post .tile-image").nth(tile).click();
  const viewer = page.getByRole("dialog", { name: "Media viewer", exact: true });
  await expect(viewer).toBeVisible();
  return viewer;
}

/** The caption's words as read on screen, and its lines. */
const caption = (page: Page) =>
  page.locator(".lb-caption").evaluate((caption) => {
    const text = caption.querySelector(".lb-text") as HTMLElement;
    const style = getComputedStyle(text);
    return {
      shown: (caption as HTMLElement).innerText.replace(/\s+/g, " ").trim(),
      lines: Math.round(text.getBoundingClientRect().height / parseFloat(style.lineHeight)),
      cut: text.scrollHeight > text.clientHeight + 1,
    };
  });

for (const width of [1280, 390]) {
  const at = async (browser: Browser, testInfo: TestInfo) => {
    const phone = width < 600;
    const context = await browser.newContext({
      baseURL: testInfo.project.use.baseURL, viewport: { width, height: phone ? 844 : 900 }, isMobile: phone, hasTouch: phone,
    });
    return { context, page: await context.newPage() };
  };

  test(`at ${width}, the caption is who posted it and its words, with "1 of 2" once`, async ({ browser }, testInfo) => {
    const { context, page } = await at(browser, testInfo);
    await serve(page);
    const viewer = await open(page, two.hash);
    await expect(viewer.locator(".lb-author")).toHaveText("@alice");
    const first = await caption(page);
    expect(first.shown).toBe("@alice · Two photos from the walk 1 of 2");
    // The alt text is the image's, not the caption's.
    await expect(viewer.locator("img.lb-media")).toHaveAttribute("alt", "Image 1 of 2 posted by @alice: Two photos from the walk");
    await expect(viewer.locator(".lb-caption")).not.toContainText("Image 1 of 2");
    await page.screenshot({ path: testInfo.outputPath(`lightbox-caption-${width}.png`) });
    // The next one: the same caption, its own counter and alt text.
    await page.keyboard.press("ArrowRight");
    await expect(viewer.locator(".lb-counter")).toHaveText("2 of 2");
    expect((await caption(page)).shown).toBe("@alice · Two photos from the walk 2 of 2");
    await expect(viewer.locator("img.lb-media")).toHaveAttribute("alt", "Image 2 of 2 posted by @alice: Two photos from the walk");
    await context.close();
  });

  test(`at ${width}, long words are cut short, two lines at most`, async ({ browser }, testInfo) => {
    const { context, page } = await at(browser, testInfo);
    await serve(page);
    await open(page, long.hash);
    const shown = await caption(page);
    expect(shown.shown.startsWith("@alice · A long caption that goes on")).toBe(true);
    // Cut at about 140 characters, ending with an ellipsis, and two lines at most.
    expect(shown.shown.length).toBeLessThan(170);
    expect(shown.shown).toContain("…");
    expect(shown.lines).toBeLessThanOrEqual(2);
    await page.screenshot({ path: testInfo.outputPath(`lightbox-caption-long-${width}.png`) });
    await context.close();
  });
}

test("a cast without words is captioned with who posted it alone", async ({ page }) => {
  await serve(page);
  const viewer = await open(page, wordless.hash);
  expect((await caption(page)).shown).toBe("@alice");
  await expect(viewer.locator(".lb-counter")).toHaveCount(0);
  await expect(viewer.locator("img.lb-media")).toHaveAttribute("alt", "Image posted by @alice");
});
