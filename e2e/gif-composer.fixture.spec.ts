// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";

/*
 * The composer and its GIF picker. KLIPY is a stand-in (no key is in the
 * repository): its answers follow its documented shape. This Worker endorses
 * no keys, so posting hands the draft to farcaster.xyz's compose link, which
 * is stubbed, so nothing leaves the test.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const bob = { fid: 7, username: "bob", display_name: "Bob" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const cast = { hash: hex(1), author: alice, text: "What's your favourite GIF?", ts, likes: 3, replies: 1 };
const reply = { hash: hex(2), author: bob, text: "This one", ts: ts - 60, likes: 0, replies: 0, parent_hash: cast.hash };
const KEY = "fixture-key";
const KLIPY = `https://api.klipy.com/api/v1/${KEY}/gifs`;
const COMPOSE = "https://farcaster.xyz/~/compose";
const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64");

/** A KLIPY result, in its documented shape. */
const gif = (slug: string, title: string) => {
  const base = `https://static.klipy.com/ii/fixture/${slug}`;
  const format = (size: string, w: number) => ({
    gif: { url: `${base}/${size}.gif`, width: w, height: w, size: 1000 },
    webp: { url: `${base}/${size}.webp`, width: w, height: w, size: 500 },
  });
  return { id: slug.length, slug, title, type: "gif", tags: [], file: { hd: format("hd", 498), md: format("md", 320), sm: format("sm", 200), xs: format("xs", 90) } };
};
const page = (prefix: string, n: number, more: boolean) => ({
  result: true,
  data: { data: Array.from({ length: n }, (_, i) => gif(`${prefix}-${i + 1}`, `${prefix} ${i + 1}`)), current_page: 1, per_page: 24, has_next: more },
});

type Calls = { key: number; klipy: string[] };

/** KLIPY's answer: it allows any origin, as a browser API must. */
const klipy = (json: object, status = 200) => ({ status, json, headers: { "access-control-allow-origin": "*" } });

/** The page's stand-ins: the feed, a post, the GIF key (`key`: null for unset), KLIPY and farcaster.xyz. */
async function serve(target: Page, { key = KEY as string | null, signedIn = true } = {}) {
  const calls: Calls = { key: 0, klipy: [] };
  await target.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await target.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [cast], notes: {}, meta } });
  });
  await target.route("**/api/v1/cast?*", (route) =>
    route.fulfill({ json: { thread: { cast, children: [{ cast: reply, children: [] }] }, notes: {}, meta } }),
  );
  await target.route("**/api/v1/channel?*", (route) =>
    route.fulfill({ json: { channel: { id: "dev", name: "Dev", parent_url: "chain://eip155:1/erc721:0xdev", follower_count: 10 }, pool: [cast], notes: {}, meta } }),
  );
  await target.route("**/api/v1/gifs/key", (route) => {
    calls.key += 1;
    // As the Worker answers: the key goes only to a session.
    if (!signedIn) return route.fulfill({ status: 401, json: { error: "Sign in to search GIFs" } });
    return key ? route.fulfill({ json: { key } }) : route.fulfill({ status: 404, json: { error: "GIF search isn't set up" } });
  });
  await target.route(`${KLIPY}/**`, (route) => {
    const url = new URL(route.request().url());
    calls.klipy.push(`${url.pathname.split("/").pop()} ${url.searchParams.get("q") ?? ""} ${url.searchParams.get("page")}`.trim());
    const q = url.searchParams.get("q");
    const n = Number(url.searchParams.get("page"));
    if (q === "nothing") return route.fulfill(klipy({ result: true, data: { data: [], has_next: false } }));
    if (q === "limit") return route.fulfill(klipy({ result: false, errors: { message: ["Too many requests"] } }, 429));
    const body = q ? page(`${q}${n > 1 ? `-p${n}` : ""}`, n > 1 ? 6 : 24, n === 1) : page("trending", 24, true);
    return route.fulfill(klipy(body));
  });
  await target.route("https://static.klipy.com/**", (route) => route.fulfill({ body: png, contentType: "image/png" }));
  await target.route("https://farcaster.xyz/**", (route) => route.fulfill({ body: "<title>Farcaster</title>", contentType: "text/html" }));
  if (signedIn) {
    await target.addInitScript((user) => {
      document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax`;
    }, me);
  }
  return calls;
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true,
  });
  return { context, page: await context.newPage() };
}

/** The farcaster.xyz page "Post in Farcaster" opens, as a URL: on a computer, through the hand-off. */
async function handOff(page: Page) {
  const opened = page.context().waitForEvent("page");
  await page.getByRole("button", { name: "Post in Farcaster" }).click();
  await page.getByRole("dialog", { name: "Post in Farcaster" }).getByRole("link", { name: /Open on farcaster\.xyz/ }).click();
  const tab = await opened;
  await tab.waitForLoadState("domcontentloaded");
  const url = new URL(tab.url());
  await tab.close();
  return url;
}

test("signed out, the composer is there, and its draft goes to the Farcaster app, with no GIF button", async ({ page }) => {
  const calls = await serve(page, { signedIn: false });
  await page.goto("/");
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  await expect(page.getByRole("button", { name: "Sign in with Farcaster" })).toBeVisible();
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await page.keyboard.type("gm, signed out");
  // GIFs take a session (KLIPY's key is handed to one); emoji don't. The
  // check waits for the composer's ask for the key, which is refused.
  await expect(dialog.getByRole("button", { name: "Add an emoji" })).toBeVisible();
  await expect.poll(() => calls.key).toBeGreaterThan(0);
  await expect(dialog.getByRole("button", { name: "Add a GIF" })).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: "Schedule for later" })).toHaveCount(0);
  const url = await handOff(page);
  expect(url.origin + url.pathname).toBe(COMPOSE);
  expect(url.searchParams.get("text")).toBe("gm, signed out");
  await expect(dialog).toHaveCount(0);
  // Reply, under a post, too.
  await page.goto(`/p/${cast.hash}`);
  await expect(page.locator(".post").getByRole("button", { name: "Reply to @alice" })).toBeVisible();
});

test("a cast with a GIF: trending, a search, more, a pick, and the hand-off", async ({ page }) => {
  const calls = await serve(page);
  await page.goto("/");
  const write = page.getByRole("button", { name: "Write a cast" });
  await write.hover();
  await write.click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("textbox", { name: "What's happening?" })).toBeFocused();
  await page.keyboard.type("gm with a GIF");
  // The picker: what's trending, in KLIPY's order, under its required placeholder.
  await dialog.getByRole("button", { name: "Add a GIF" }).click();
  const picker = dialog.getByRole("region", { name: "GIFs from KLIPY" });
  const search = picker.getByRole("searchbox", { name: "Search GIFs" });
  await expect(search).toHaveAttribute("placeholder", "Search KLIPY");
  await expect(search).toBeFocused();
  const tiles = picker.locator(".gif-tile:not(.placeholder)");
  await expect(tiles).toHaveCount(24);
  await expect(tiles.first()).toHaveAccessibleName("trending 1");
  await expect(tiles.nth(23)).toHaveAccessibleName("trending 24");
  await expect(tiles.first().locator("img")).toHaveAttribute("src", "https://static.klipy.com/ii/fixture/trending-1/sm.webp");
  await expect(picker).toContainText("Powered by KLIPY");
  // Typing: one search, once typing pauses.
  await search.pressSequentially("cats", { delay: 60 });
  await expect(tiles.first()).toHaveAccessibleName("cats 1");
  await expect(picker).toContainText("GIFs for “cats”");
  // More: KLIPY's next page, after the first.
  await picker.getByRole("button", { name: "More GIFs" }).click();
  await expect(tiles).toHaveCount(30);
  await expect(tiles.nth(24)).toHaveAccessibleName("cats-p2 1");
  await expect(picker.getByRole("button", { name: "More GIFs" })).toHaveCount(0);
  // Back to an earlier search costs nothing.
  await search.fill("");
  await expect(tiles.first()).toHaveAccessibleName("trending 1");
  expect(calls.klipy).toEqual(["trending  1", "search cats 1", "search cats 2"]);
  // A pick attaches it and closes the picker.
  await tiles.nth(2).click();
  await expect(picker).toHaveCount(0);
  const attached = dialog.locator(".compose-gif img");
  await expect(attached).toHaveAttribute("alt", "trending 3");
  await expect(dialog.getByRole("textbox")).toBeFocused();
  // The hand-off: farcaster.xyz's compose link with the text and the GIF.
  const url = await handOff(page);
  expect(`${url.origin}${url.pathname}`).toBe(COMPOSE);
  expect(url.searchParams.get("text")).toBe("gm with a GIF");
  expect(url.searchParams.getAll("embeds[]")).toEqual(["https://static.klipy.com/ii/fixture/trending-3/md.gif"]);
  expect(url.searchParams.has("parentCastHash")).toBe(false);
  expect(url.searchParams.has("channelKey")).toBe(false);
  // The draft went with it.
  await expect(dialog).toHaveCount(0);
  await write.click();
  await expect(page.getByRole("dialog", { name: "New cast" }).getByRole("textbox")).toHaveValue("");
  // The key was read once, ahead of the first click.
  expect(calls.key).toBe(1);
});

test("without a GIF key there is no GIF button, and a cast still hands off", async ({ page }) => {
  const calls = await serve(page, { key: null });
  await page.goto("/");
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await expect(dialog.getByRole("textbox")).toBeFocused();
  await expect(dialog.getByRole("button", { name: "Post in Farcaster" })).toBeDisabled();
  // This Worker endorses no keys: the Farcaster app posts it, and says so.
  await expect(dialog.locator(".compose-note")).toHaveText("Allthing doesn't post: this opens the Farcaster app with your cast ready to post.");
  await page.keyboard.type("Just words");
  await page.waitForTimeout(400);
  await expect(dialog.getByRole("button", { name: "Add a GIF" })).toHaveCount(0);
  const url = await handOff(page);
  expect(url.searchParams.get("text")).toBe("Just words");
  expect(url.searchParams.getAll("embeds[]")).toEqual([]);
  expect(calls.klipy).toEqual([]);
});

test("Cast is a serif pill at the top right of the feed and of a channel, and nowhere else", async ({ page }) => {
  await serve(page);
  await page.goto("/");
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  // Not in the header any more, and only one: the floating one is for phones.
  await expect(page.locator(".site-header").getByRole("button", { name: "Write a cast" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Write a cast" })).toHaveCount(1);
  const head = page.locator(".listing-head");
  const pill = head.getByRole("button", { name: "Write a cast" });
  await expect(pill).toHaveText("Cast");
  // At the top right of the feed, level with the sort tabs.
  const [box, row, tabs] = await Promise.all([pill.boundingBox(), head.boundingBox(), page.locator(".sort-tabs").boundingBox()]);
  expect(Math.abs(box!.x + box!.width - (row!.x + row!.width))).toBeLessThan(1);
  expect(Math.abs(box!.y + box!.height / 2 - (tabs!.y + tabs!.height / 2))).toBeLessThan(4);
  // Allthing's serif, and its indigo accent turning to the portal cyan, not
  // Farcaster's purple.
  const look = await pill.evaluate((el) => ({ font: getComputedStyle(el).fontFamily, fill: getComputedStyle(el).backgroundImage }));
  expect(look.font).toContain("Instrument Serif");
  expect(look.fill).toContain("rgb(79, 67, 212)");
  expect(look.fill).toContain("rgb(11, 124, 145)");
  expect(look.fill).not.toMatch(/rgb\((124, 101, 193|133, 93, 205|138, 99, 210)\)/);
  // A channel's: at the top right, beside its name.
  await page.goto("/c/dev");
  const title = page.locator(".channel-title-row");
  const channelPill = title.getByRole("button", { name: "Write a cast" });
  await expect(channelPill).toBeVisible();
  const [p, h, r] = await Promise.all([channelPill.boundingBox(), title.locator("h1").boundingBox(), title.boundingBox()]);
  expect(Math.abs(p!.x + p!.width - (r!.x + r!.width))).toBeLessThan(1);
  expect(Math.abs(p!.y + p!.height / 2 - (h!.y + h!.height / 2))).toBeLessThan(6);
  expect(p!.x).toBeGreaterThan(h!.x + h!.width);
  // A post has none: Reply and Quote are there instead.
  await page.goto(`/p/${cast.hash}`);
  await expect(page.locator(".post")).toBeVisible();
  await expect(page.getByRole("button", { name: "Write a cast" })).toHaveCount(0);
});

test("Reply answers the post, or a reply, with its hash", async ({ page }) => {
  await serve(page);
  await page.goto(`/p/${cast.hash}`);
  await page.getByRole("button", { name: "Reply to @alice" }).click();
  let dialog = page.getByRole("dialog", { name: "Reply" });
  await expect(dialog).toContainText("Replying to @alice");
  await expect(dialog).toContainText("What's your favourite GIF?");
  await page.keyboard.type("This!");
  let url = await handOff(page);
  expect(url.searchParams.get("parentCastHash")).toBe(cast.hash);
  expect(url.searchParams.get("text")).toBe("This!");
  expect(url.searchParams.has("channelKey")).toBe(false);
  await page.getByRole("button", { name: "Reply to @bob" }).click();
  dialog = page.getByRole("dialog", { name: "Reply" });
  await expect(dialog).toContainText("Replying to @bob");
  await page.keyboard.type("Agreed");
  url = await handOff(page);
  expect(url.searchParams.get("parentCastHash")).toBe(reply.hash);
});

test("a cast from a channel page goes to that channel", async ({ page }) => {
  await serve(page);
  await page.goto("/c/dev");
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await expect(dialog).toContainText("In /dev");
  await page.keyboard.type("Hello, devs");
  const url = await handOff(page);
  expect(url.searchParams.get("channelKey")).toBe("dev");
});

test("Escape closes the picker, then the composer, which keeps its draft", async ({ page }) => {
  const calls = await serve(page);
  await page.goto("/");
  const write = page.getByRole("button", { name: "Write a cast" });
  await write.click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await page.keyboard.type("Draft");
  await dialog.getByRole("button", { name: "Add a GIF" }).click();
  await expect(dialog.locator(".gif-tile:not(.placeholder)")).toHaveCount(24);
  await page.keyboard.press("Escape");
  await expect(dialog.getByRole("region", { name: "GIFs from KLIPY" })).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: "Add a GIF" })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(write).toBeFocused();
  await write.click();
  await expect(page.getByRole("dialog", { name: "New cast" }).getByRole("textbox")).toHaveValue("Draft");
  // Trending again, from what this visit kept.
  await page.getByRole("button", { name: "Add a GIF" }).click();
  await expect(page.locator(".gif-tile:not(.placeholder)")).toHaveCount(24);
  expect(calls.klipy).toEqual(["trending  1"]);
});

test("a search with no results says so; a failed one offers a retry", async ({ page }) => {
  await serve(page);
  let fail = true;
  await page.route(`${KLIPY}/search?q=boom*`, (route) =>
    route.fulfill(fail ? klipy({ result: false }, 500) : klipy(page_("boom"))),
  );
  await page.goto("/");
  await page.getByRole("button", { name: "Write a cast" }).click();
  await page.getByRole("button", { name: "Add a GIF" }).click();
  const picker = page.getByRole("region", { name: "GIFs from KLIPY" });
  const search = picker.getByRole("searchbox");
  await search.fill("nothing");
  await expect(picker).toContainText("No GIFs found.");
  await search.fill("limit");
  await expect(picker).toContainText("Too many GIF searches on Allthing just now.");
  await search.fill("boom");
  await expect(picker).toContainText("GIFs couldn't load just now.");
  fail = false;
  await picker.getByRole("button", { name: "Try again" }).click();
  await expect(picker.locator(".gif-tile:not(.placeholder)").first()).toHaveAccessibleName("boom 1");
});
const page_ = (q: string) => page(q, 24, false);

test("on a phone, the composer and its picker fit the screen", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await serve(page);
  await page.goto("/");
  const write = page.getByRole("button", { name: "Write a cast" });
  await expect(write).toBeVisible();
  // A round button floating over the feed at the bottom right, in reach of
  // a thumb; the pill and the header have none.
  const fab = (await write.boundingBox())!;
  expect([Math.round(fab.width), Math.round(fab.height)]).toEqual([56, 56]);
  expect(Math.abs(fab.x + fab.width - (390 - 16))).toBeLessThan(1);
  expect(Math.abs(fab.y + fab.height - (844 - 16))).toBeLessThan(1);
  expect(await write.evaluate((el) => getComputedStyle(el).position)).toBe("fixed");
  await expect(page.locator(".listing-head .cast-pill")).toBeHidden();
  await expect(page.locator(".site-header").getByRole("button", { name: "Write a cast" })).toHaveCount(0);
  await write.tap();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await expect(dialog).toBeInViewport({ ratio: 1 });
  await dialog.getByRole("button", { name: "Add a GIF" }).tap();
  const tiles = dialog.locator(".gif-tile:not(.placeholder)");
  await expect(tiles).toHaveCount(24);
  // Three to a row, each a comfortable tap.
  const boxes = await Promise.all([0, 1, 2, 3].map((i) => tiles.nth(i).boundingBox()));
  expect(boxes[0]!.y).toBeCloseTo(boxes[2]!.y, 0);
  expect(boxes[3]!.y).toBeGreaterThan(boxes[0]!.y + boxes[0]!.height - 1);
  expect(boxes[0]!.width).toBeGreaterThanOrEqual(88);
  await tiles.nth(1).tap();
  await expect(dialog.locator(".compose-gif img")).toBeVisible();
  await expect(dialog.getByRole("link", { name: /Post in Farcaster/ })).toBeInViewport();
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  expect(overflow).toBeLessThanOrEqual(0);
  // A channel's feed has it too; a post doesn't (Reply and Quote are there).
  await page.keyboard.press("Escape");
  await page.goto("/c/dev");
  await expect(page.getByRole("button", { name: "Write a cast" })).toBeInViewport();
  await page.goto(`/p/${cast.hash}`);
  await expect(page.locator(".post")).toBeVisible();
  await expect(page.getByRole("button", { name: "Write a cast" })).toHaveCount(0);
  await context.close();
});

test("the Worker keeps the GIF key from anyone not signed in", async ({ request }) => {
  const anon = await request.get("/api/v1/gifs/key");
  expect(anon.status()).toBe(401);
  expect(anon.headers()["cache-control"]).toBe("no-store");
  const forged = await request.get("/api/v1/gifs/key", { headers: { cookie: "__Host-allthing-session=1234.9999999999.forged" } });
  expect(forged.status()).toBe(401);
});
