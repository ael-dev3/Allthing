// SPDX-License-Identifier: AGPL-3.0-or-later
import { readFileSync } from "node:fs";
import path from "node:path";
import { expect, test, type Page, type Route } from "@playwright/test";

/*
 * Allthing end to end, against live Farcaster data (Hypersnap), either through
 * the Worker or, with E2E_PAGES=1, as the static GitHub Pages copy that reads
 * Hypersnap from the browser. Screenshots land in docs/screenshots (override
 * with E2E_SCREENSHOT_DIR).
 */

const SHOTS = process.env.E2E_SCREENSHOT_DIR ?? "docs/screenshots";
const LIVE = Boolean(process.env.E2E_BASE_URL);
const PAGES = Boolean(process.env.E2E_PAGES);
// The static copy is served under a sub-path, like https://ael-dev3.github.io/Allthing/.
const BASE_PATH = process.env.E2E_BASE_PATH ?? (PAGES ? "/Allthing" : "");
const at = (path: string) => BASE_PATH + path;

// Known casts. Casts are immutable, so these stay valid unless deleted.
const VIDEO_CAST = "0x3b6c0db881a351984ab7844cd56265d492431d79"; // @0xael.eth, Hyperion prologue (HLS video)
const YOUTUBE_CAST = "0x4717fecaeb0d48aedfb30bf1a2b2436e70d4b80c"; // @0xael.eth, a youtu.be link
const GALLERY_CAST = "0x4cc5b1c6f925c928694fef95643cd3bb24803b14"; // two images in /itookaphoto
const NESTED_THREAD = "0xf454b8d3e5ee2c9252a0b7574ad4942e9ba502d2"; // @balajis.eth, replies four levels deep

const firstCard = (page: Page) => page.locator(".cast-card:not(.skeleton)").first();

/** Data requests made during `action`: Worker API calls, or Hypersnap reads in the static copy. */
async function apiRequests(page: Page, action: () => Promise<void>): Promise<string[]> {
  const seen: string[] = [];
  const listener = (request: { url(): string; method(): string }) => {
    const url = new URL(request.url());
    if (url.pathname.startsWith("/api/") || url.hostname === "haatz.quilibrium.com") seen.push(`${request.method()} ${request.url()}`);
  };
  page.on("request", listener);
  await action();
  page.off("request", listener);
  return seen;
}

/** Screenshot the viewport once the images in view have loaded (or after 10 s). */
async function shoot(page: Page, name: string) {
  await page
    .waitForFunction(() => Array.from(document.images).every((img) => {
      const box = img.getBoundingClientRect();
      return img.complete || box.bottom < 0 || box.top > innerHeight || box.width === 0;
    }), undefined, { timeout: 10_000 })
    .catch(() => {});
  await page.screenshot({ path: `${SHOTS}/${name}.png`, animations: "disabled" });
}

/** Serve the VP9/Opus fixture in place of every Farcaster HLS stream. */
async function useFixtureStream(page: Page) {
  const dir = path.join(__dirname, "fixtures/hls");
  await page.route("https://stream.farcaster.xyz/v1/video/**", async (route: Route) => {
    const name = new URL(route.request().url()).pathname.split("/").pop() ?? "";
    const file = name.endsWith(".m3u8") && !name.startsWith("stream_") ? "master.m3u8" : name;
    const type = file.endsWith(".m3u8") ? "application/vnd.apple.mpegurl" : "video/mp4";
    await route.fulfill({ body: readFileSync(path.join(dir, file)), contentType: type, headers: { "access-control-allow-origin": "*" } });
  });
}

test.describe("front page", () => {
  test("loads trending casts fast, with the wordmark and honest engagement", async ({ page }) => {
    const started = Date.now();
    await page.goto(at("/"));
    await expect(firstCard(page)).toBeVisible();
    const elapsed = Date.now() - started;
    // The static copy builds the page in the browser (several upstream round trips, no edge cache).
    expect(elapsed, `first casts after ${elapsed} ms`).toBeLessThan(PAGES ? 6000 : LIVE ? 3000 : 5000);

    // Branding: the wordmark only, no logo mark.
    await expect(page.locator("header .wordmark")).toHaveText("Allthing");
    await expect(page.locator(".site-header svg, .site-header img, .logo-mark")).toHaveCount(0);

    // Likes, recasts and replies as three labeled counts; nothing to vote with.
    const card = firstCard(page);
    await expect(card.locator(".stat-likes .n")).toHaveText(/^\d[\d,.km]*$/);
    await expect(card.locator(".stat-recasts")).toContainText(/recasts?/);
    await expect(card.locator(".stat-replies")).toContainText(/repl(y|ies)/);
    await expect(page.locator('[aria-label*="downvote" i], .downvote, .arrow.down')).toHaveCount(0);
    await expect(page.getByRole("button", { name: /vote/i })).toHaveCount(0);

    // Who, when, where, scored, and via which app; read from Hypersnap.
    await expect(card.locator(".user-chip .avatar").first()).toBeVisible();
    await expect(card.locator("time")).toHaveAttribute("datetime", /^\d{4}-\d\d-\d\dT/);
    await expect(page.locator(".cast-card .score").first()).toHaveText(/^[01]\.\d\d$/);
    await expect(page.locator(".cast-card .origin").first()).toContainText("via ");
    await expect(card.locator(".backend")).toContainText("Hypersnap");

    // Current data: the newest cast in the pool is under two days old.
    const newest = await page.locator(".cast-card").evaluateAll((cards) =>
      Math.max(...cards.map((c) => Number(c.getAttribute("data-ts")))),
    );
    expect(Date.now() / 1000 - newest).toBeLessThan(2 * 86_400);
    await shoot(page, "home");
  });

  test("new and top re-rank the same pool without another API call", async ({ page }) => {
    await page.goto(at("/"));
    await expect(firstCard(page)).toBeVisible();
    const calls = await apiRequests(page, async () => {
      await page.locator(".sort-tabs a", { hasText: "new" }).click();
      await expect(page).toHaveURL(/sort=new/);
      await expect(firstCard(page)).toBeVisible();
    });
    expect(calls, "sorting is local").toHaveLength(0);
    const times = await page.locator(".cast-card").evaluateAll((c) => c.map((e) => Number(e.getAttribute("data-ts"))));
    expect(times).toEqual([...times].sort((a, b) => b - a));

    await page.locator(".sort-tabs a", { hasText: "top" }).click();
    await expect(page).toHaveURL(/sort=top/);
    const points = await page.locator(".cast-card").evaluateAll((c) => c.map((e) => Number(e.getAttribute("data-points"))));
    expect(points).toEqual([...points].sort((a, b) => b - a));
  });

  test("each page view costs one API call", async ({ page }) => {
    test.skip(PAGES, "the static copy has no API: the browser reads Hypersnap itself");
    const calls = await apiRequests(page, async () => {
      await page.goto(at("/c/farcaster"));
      await expect(firstCard(page)).toBeVisible();
    });
    expect(calls.filter((c) => c.includes("/api/v1/channel"))).toHaveLength(1);
    expect(calls.every((c) => c.startsWith("GET "))).toBe(true);
  });
});

test.describe("media", () => {
  test("images enlarge on hover and open an accessible lightbox", async ({ page }) => {
    await page.goto(at(`/p/${GALLERY_CAST}`));
    const tiles = page.locator(".post .tile-image");
    await expect(tiles).toHaveCount(2);

    // Hover preview: no click needed.
    await tiles.first().hover();
    const preview = page.locator(".hover-preview img");
    await expect(preview).toBeVisible();
    await expect(preview).toHaveAttribute("src", /imagedelivery\.net|https:/);
    await page.mouse.move(5, 5);
    await expect(page.locator(".hover-preview")).toHaveCount(0);

    // Lightbox: dialog, focus inside, alt text, keyboard and swipe navigation.
    await tiles.first().click();
    const dialog = page.getByRole("dialog", { name: "Media viewer" });
    await expect(dialog).toBeVisible();
    await expect(page.getByRole("button", { name: /Close media viewer/ })).toBeFocused();
    const image = dialog.locator("img.lb-media");
    await expect(image).toHaveAttribute("alt", /^Image 1 of 2 posted by @/);
    await expect(dialog.locator(".lb-alt")).toContainText("Image 1 of 2");
    await expect(dialog.locator(".lb-counter")).toHaveText("1 of 2");
    await expect.poll(() => image.evaluate((img: HTMLImageElement) => img.complete && img.naturalWidth > 0)).toBe(true);
    await shoot(page, "post-lightbox");

    await page.keyboard.press("ArrowRight");
    await expect(dialog.locator(".lb-counter")).toHaveText("2 of 2");
    await page.keyboard.press("ArrowLeft");
    await expect(dialog.locator(".lb-counter")).toHaveText("1 of 2");

    // Swipe left with a pointer drag.
    const box = (await dialog.locator(".lb-stage").boundingBox())!;
    const y = box.y + box.height / 2;
    await page.mouse.move(box.x + box.width * 0.7, y);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * 0.2, y, { steps: 6 });
    await page.mouse.up();
    await expect(dialog.locator(".lb-counter")).toHaveText("2 of 2");

    // Tab stays inside, Escape closes and returns focus to the image.
    for (let i = 0; i < 5; i++) await page.keyboard.press("Tab");
    expect(await page.evaluate(() => Boolean(document.activeElement?.closest(".lightbox")))).toBe(true);
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    await expect(tiles.first()).toBeFocused();
  });

  test("a Farcaster video by @0xael.eth plays inline, not as a link", async ({ page }) => {
    await page.goto(at(`/p/${VIDEO_CAST}`));
    const video = page.locator(".post .video video");
    await expect(video).toBeVisible();
    // The poster comes from the stream host itself, never through our Worker.
    await expect(video).toHaveAttribute("poster", /cloudflarestream\.com\/.+\/thumbnails\/thumbnail\.jpg/);
    await expect.poll(() => page.evaluate(() => {
      const src = document.querySelector<HTMLVideoElement>(".post .video video")?.poster;
      return new Promise<boolean>((resolve) => {
        const img = new Image();
        img.onload = () => resolve(img.naturalWidth > 0);
        img.onerror = () => resolve(false);
        img.src = src ?? "";
      });
    })).toBe(true);
    await expect(page.locator(".post").getByRole("button", { name: /^Play video:/ })).toBeVisible();
    // No raw playlist links or grey placeholders.
    await expect(page.locator(".post a[href*='.m3u8']")).toHaveCount(0);
    await expect(page.locator(".post")).not.toContainText(".m3u8");
    await shoot(page, "video-post");
  });

  test("the Rust HLS player streams segments: hover previews muted, click plays with controls", async ({ page }) => {
    await useFixtureStream(page);
    await page.goto(at(`/p/${VIDEO_CAST}`));
    const player = page.locator(".post .video").first();
    const video = player.locator("video");
    await expect(player).toBeVisible();
    await expect.poll(() => player.getAttribute("class")).not.toMatch(/loading/);

    await player.hover();
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.muted && !v.paused)).toBe(true);
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.currentTime), { timeout: 15_000 }).toBeGreaterThan(0.3);
    expect(await video.evaluate((v: HTMLVideoElement) => v.src)).toMatch(/^blob:/);

    await player.getByRole("button", { name: /^Play video:/ }).click();
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.controls && !v.muted)).toBe(true);
    const playedTo = await video.evaluate((v: HTMLVideoElement) => v.currentTime);
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.currentTime), { timeout: 15_000 }).toBeGreaterThan(playedTo + 0.5);
    expect(await video.getAttribute("data-error")).toBeNull();

    // Seeking past the buffer fetches the right segment.
    await video.evaluate((v: HTMLVideoElement) => { v.currentTime = 4.5; });
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.readyState >= 3 && v.currentTime >= 4.5), { timeout: 15_000 }).toBe(true);
  });

  test("a video opens in the lightbox with the full player", async ({ page }) => {
    await useFixtureStream(page);
    await page.goto(at(`/p/${VIDEO_CAST}`));
    await page.locator(".post .video").first().hover();
    await page.locator(".post").getByRole("button", { name: "Open in the media viewer" }).first().click();
    const dialog = page.getByRole("dialog", { name: "Media viewer" });
    const video = dialog.locator("video");
    await expect(video).toBeVisible();
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.controls && v.currentTime > 0.2), { timeout: 15_000 }).toBe(true);
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
  });

  test("plain video files play natively, straight from their host", async ({ page }) => {
    // The same cast, with its stream swapped for a plain .mp4 file.
    const clip = "https://media.example/clip.mp4";
    const dir = path.join(__dirname, "fixtures/hls");
    const body = Buffer.concat(["init_0.mp4", "seg_0_0.m4s", "seg_0_1.m4s", "seg_0_2.m4s"].map((f) => readFileSync(path.join(dir, f))));
    await page.route(clip, (route) => route.fulfill({ body, contentType: "video/mp4" }));
    // The Worker's payload, or (static copy) Hypersnap's conversation answer.
    await page.route(new RegExp(`/api/v1/cast\\?hash=${VIDEO_CAST}$|haatz\\.quilibrium\\.com/v2/farcaster/cast/conversation\\?`), async (route) => {
      const response = await route.fetch();
      const text = (await response.text()).replace(/https:\/\/stream\.farcaster\.xyz\/[^"]+/g, clip);
      await route.fulfill({ response, body: text });
    });
    await page.goto(at(`/p/${VIDEO_CAST}`));
    const player = page.locator(".post .video").first();
    const video = player.locator("video");
    await expect(video).toHaveAttribute("src", `${clip}#t=0.1`);
    await player.hover();
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.muted && !v.paused)).toBe(true);
    await player.getByRole("button", { name: /^Play video:/ }).click();
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.controls && !v.muted && v.currentTime > 0.5), { timeout: 15_000 }).toBe(true);
    await expect(player.locator(".video-note")).toHaveCount(0);
  });

  test("YouTube links get a privacy-friendly, click-to-load player", async ({ page }) => {
    // Keep the test hermetic: nothing from YouTube itself needs to load.
    await page.route(/youtube-nocookie\.com|ytimg\.com/, (route) => route.fulfill({ status: 204, body: "" }));
    await page.goto(at(`/p/${YOUTUBE_CAST}`));
    const facade = page.locator(".post .facade[data-provider=youtube]");
    await expect(facade).toBeVisible();
    await expect(facade.locator("iframe")).toHaveCount(0);
    await expect(facade.locator("img")).toHaveAttribute("src", "https://i.ytimg.com/vi/cBT6vU55Gkg/hqdefault.jpg");
    await facade.getByRole("button", { name: /Play YouTube video/ }).click();
    await expect(facade.locator("iframe")).toHaveAttribute("src", /^https:\/\/www\.youtube-nocookie\.com\/embed\/cBT6vU55Gkg\?autoplay=1/);
  });
});

test.describe("profiles", () => {
  test("show score, registration, counts, wallet activity by chain and the app label", async ({ page }) => {
    await page.goto(at("/u/0xael.eth"));
    const profile = page.locator(".profile");
    await expect(profile.locator(".profile-name")).toHaveText(/\S/);
    await expect(profile.locator(".handle")).toContainText("@0xael.eth");
    await expect(profile.locator(".handle")).toContainText("fid 539854");
    await expect(profile.locator(".counts")).toContainText(/[\d,]+ followers/);
    await expect(profile.locator(".counts")).toContainText(/[\d,]+ following/);
    await expect(profile.locator(".bio")).toHaveText(/\S/);
    await expect(profile.locator(".fact", { hasText: "Neynar score" }).locator("dd")).toHaveText(/^[01]\.\d\d$/);
    await expect(profile.locator(".fact", { hasText: "Registered" }).locator("dd")).toHaveText(/\d{4}/);
    await expect(profile.locator(".fact", { hasText: "Farcaster app label" }).locator("dd")).toHaveText(/no label|labeled|spam|Nerfed|unavailable/i);

    const wallets = page.locator(".wallets");
    await expect(wallets.locator(".share-bar")).toBeVisible();
    await expect(wallets.locator(".share-list li").first()).toContainText(/%/);
    await expect(wallets.locator(".share-list")).toContainText("Base");
    await expect(wallets.locator(".sources summary")).toContainText("How this is measured");
    await expect(page.locator(".side-box", { hasText: "Posts via" })).toContainText("%");
    await expect(firstCard(page)).toBeVisible();
    await shoot(page, "profile");
  });

  test("a flagged account shows the label and still shows every cast", async ({ page }) => {
    // Pretend the main client nerfed this account, via the static label shard.
    await page.route("**/data/labels/spam/meta.json", (route) =>
      route.fulfill({
        json: { source: "test", version: "test", newest_label: 1_780_000_000, built_at: 0, shard_size: 32768, shards: 120, counts: { likely_spam: 0, not_spam: 0, nerfed: 1 } },
      }),
    );
    await page.route("**/data/labels/spam/0016.txt", (route) =>
      route.fulfill({ body: "-".repeat(539854 % 32768) + "3" + "-".repeat(32768 - (539854 % 32768) - 1), contentType: "text/plain" }),
    );
    await page.goto(at("/u/0xael.eth"));
    await expect(page.locator(".spam-label")).toHaveText("Nerfed for malicious activity");
    await expect(page.locator(".label-note")).toContainText("never hides anyone's casts");
    await expect(firstCard(page)).toBeVisible();
    expect(await page.locator(".cast-card").count()).toBeGreaterThan(5);
    await shoot(page, "profile-labeled");
  });

  test("dotted ENS-style names and the old static-site links resolve", async ({ page }) => {
    await page.goto(at("/u/?name=balajis.eth"));
    await expect(page.locator(".profile .handle")).toContainText("@balajis.eth · fid 37");
    await page.goto(at("/u/balajis.eth"));
    await expect(page.locator(".profile .handle")).toContainText("@balajis.eth");
    await expect(firstCard(page).locator(".author")).toHaveText("@balajis.eth");
  });
});

test.describe("threads and navigation", () => {
  test("replies collapse and expand with animation, by click and with x", async ({ page }) => {
    await page.goto(at(`/p/${NESTED_THREAD}`));
    const parent = page.locator(".comments > .comment").filter({ has: page.locator(":scope > .comment-body .children .comment") }).first();
    await expect(parent).toBeVisible();
    const toggle = parent.locator(":scope > .comment-head .toggle");
    const child = parent.locator(":scope > .comment-body .children .comment").first();
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
    await expect(child).toBeVisible();

    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await expect(parent).toHaveClass(/collapsed/);
    await expect(parent.locator(":scope > .comment-head .hidden-count")).toContainText(/\d+ repl(y|ies) hidden/);
    await expect.poll(() => parent.locator(":scope > .comment-body").evaluate((e) => e.getBoundingClientRect().height)).toBeLessThan(2);
    await shoot(page, "thread-collapsed");

    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
    await expect(child).toBeVisible();

    // Keyboard: select the reply by clicking its header, then x.
    await parent.locator(":scope > .comment-head .sep").click();
    await expect(parent).toHaveClass(/selected/);
    await page.keyboard.press("x");
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await page.keyboard.press("x");
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
  });

  test("j/k move between casts and o opens one", async ({ page }) => {
    await page.goto(at("/"));
    await expect(firstCard(page)).toBeVisible();
    await page.keyboard.press("j");
    await expect(page.locator(".cast-card").nth(0)).toHaveClass(/selected/);
    await page.keyboard.press("j");
    await expect(page.locator(".cast-card").nth(1)).toHaveClass(/selected/);
    await page.keyboard.press("k");
    await expect(page.locator(".cast-card").nth(0)).toHaveClass(/selected/);
    const hash = await page.locator(".cast-card").nth(0).getAttribute("data-hash");
    await page.keyboard.press("o");
    await expect(page).toHaveURL(new RegExp(`/p/${hash}$`));
    await expect(page.locator(".post")).toBeVisible();
  });

  test("provenance names the signing app and the backend", async ({ page }) => {
    await page.goto(at(`/p/${VIDEO_CAST}`));
    const box = page.locator(".side-box", { hasText: "Provenance" });
    await expect(box.locator("dd").first()).toContainText(/Warpcast|Farcaster|app fid/);
    await expect(box).toContainText(/0x[0-9a-f]{4}…[0-9a-f]{4}/);
    await expect(box).toContainText("Hypersnap");
    await expect(box).toContainText("not recorded");
  });

  test("channels and the directory render", async ({ page }) => {
    await page.goto(at("/channels"));
    await expect(page.locator(".channel-card").first()).toBeVisible();
    await page.locator(".channel-card a", { hasText: "/farcaster" }).click();
    await expect(page).toHaveURL(/\/c\/farcaster/);
    await expect(page.locator(".page-title")).toHaveText("/farcaster");
    await expect(firstCard(page)).toBeVisible();
  });

  test("read-only: no sign-in, no writes", async ({ page }) => {
    const writes: string[] = [];
    page.on("request", (r) => {
      if (r.method() !== "GET" && r.method() !== "HEAD" && new URL(r.url()).origin === new URL(page.url() || "http://x").origin) writes.push(r.url());
    });
    await page.goto(at("/"));
    await expect(firstCard(page)).toBeVisible();
    await expect(page.locator("header, .side, footer").getByText(/sign in|log in|connect wallet/i)).toHaveCount(0);
    expect(writes).toHaveLength(0);
    if (!PAGES) {
      const response = await page.request.post("/api/v1/home", { data: {} });
      expect(response.status()).toBe(405);
    }
  });

  test("unknown things say so", async ({ page }) => {
    await page.goto(at("/u/no-such-user-zz9-plural-z-alpha"));
    await expect(page.locator(".error-panel")).toContainText("Nothing here");
    await page.goto(at("/p/0x0000000000000000000000000000000000000000"));
    await expect(page.locator(".error-panel")).toContainText("Nothing here");
    await page.goto(at("/definitely/not/a/page"));
    await expect(page.locator(".error-panel")).toContainText("Page not found");
  });
});

test.describe("hosting", () => {
  test.skip(!LIVE || PAGES, "only against the Cloudflare deployment");

  test("www redirects to the apex domain", async ({ page }) => {
    const base = new URL(process.env.E2E_BASE_URL!);
    await page.goto(`${base.protocol}//www.${base.host}/u/dwr`);
    await expect(page).toHaveURL(`${base.origin}/u/dwr`);
    await expect(page.locator(".profile .handle")).toContainText("@dwr");
  });

  test("static assets are long-cached and the API is edge-cached", async ({ request }) => {
    const index = await request.get("/");
    const asset = /\/assets\/allthing-[0-9a-f]+\.wasm/.exec(await index.text())?.[0];
    expect(asset).toBeTruthy();
    const wasm = await request.get(asset!);
    expect(wasm.headers()["cache-control"]).toContain("immutable");
    await request.get("/api/v1/home");
    const second = await request.get("/api/v1/home");
    expect(second.headers()["x-allthing-cache"]).toMatch(/fresh|revalidating/);
  });
});
