import { expect, test, type Locator, type Page } from "@playwright/test";

/*
 * Live-data tests for the four page types. They run against the server
 * build, the static export, or a deployed URL (see playwright.config.ts),
 * and refresh the screenshots the docs link to.
 */

const STATIC = process.env.E2E_STATIC === "1";
const SHOTS = process.env.E2E_SCREENSHOT_DIR ?? "docs/screenshots";
const META_LINE = /^submitted (just now|\d+(m|h|d|mo|y) ago) by @\S+( to \/c\/\S+)? \| [\d,]+ comments?$/;
const POST_URL = /\/p\/(\?hash=)?0x[0-9a-f]{40}$/;
// A long-lived thread with nested replies, used to exercise the comment tree.
const NESTED_THREAD = "0xf454b8d3e5ee2c9252a0b7574ad4942e9ba502d2";

/** Paths relative to baseURL, in the URL scheme of the build under test. */
const paths = {
  home: (query = "") => (query ? `?${query}` : ""),
  channel: (id: string, query = "") =>
    STATIC
      ? `c/?id=${encodeURIComponent(id)}${query ? `&${query}` : ""}`
      : `c/${encodeURIComponent(id)}${query ? `?${query}` : ""}`,
  post: (hash: string) => (STATIC ? `p/?hash=${hash}` : `p/${hash}`),
  user: (name: string, query = "") =>
    STATIC
      ? `u/?name=${encodeURIComponent(name)}${query ? `&${query}` : ""}`
      : `u/${encodeURIComponent(name)}${query ? `?${query}` : ""}`,
};

function absolute(path: string, baseURL: string | undefined): string {
  return new URL(path, baseURL).href;
}

function listingRows(page: Page): Locator {
  return page.locator("#siteTable .thing.link");
}

async function timestamps(rows: Locator): Promise<number[]> {
  const values = await rows.locator(".tagline time").evaluateAll((els) => els.map((el) => el.getAttribute("datetime")));
  return values.map((value) => Date.parse(value ?? ""));
}

/** Wait for visible images so screenshots aren't full of empty thumbnails. */
async function settle(page: Page) {
  await page.waitForLoadState("networkidle", { timeout: 15_000 }).catch(() => {});
  await page
    .waitForFunction(
      () =>
        Array.from(document.images)
          .filter((img) => img.getBoundingClientRect().top < window.innerHeight)
          .every((img) => img.complete),
      null,
      { timeout: 15_000 },
    )
    .catch(() => {});
}

async function waitForKeyboardNav(page: Page) {
  await expect(page.locator("html")).toHaveAttribute("data-keyboard-nav", "ready");
}

test("front page lists current trending casts as old-reddit rows", async ({ page }) => {
  await page.goto(paths.home());
  const rows = listingRows(page);
  await expect(rows.first()).toBeVisible();
  expect(await rows.count()).toBeGreaterThanOrEqual(10);

  const first = rows.first();
  await expect(first.locator(".rank")).toHaveText("1");
  await expect(first.locator(".score")).toHaveText(/^\d+(\.\d)?[km]?$/);
  await expect(first.locator("a.title")).toHaveAttribute("href", POST_URL);
  for (const tagline of await rows.locator(".tagline").allTextContents()) {
    expect(tagline.replace(/\s+/g, " ").trim()).toMatch(META_LINE);
  }
  await expect(page.locator(".tabmenu li.selected")).toHaveText("hot");
  await expect(page.locator("#header")).toHaveCSS("background-color", "rgb(206, 227, 248)");
  await expect(page.locator("body")).toHaveCSS("font-family", /verdana/i);

  // Real and current: the newest cast on the page is less than two days old.
  const newest = Math.max(...(await timestamps(rows)));
  expect(Date.now() - newest).toBeLessThan(48 * 3_600_000);

  await settle(page);
  await page.screenshot({ path: `${SHOTS}/front-page.png` });
});

test("each page type shows real content within 3 seconds", async ({ page }) => {
  const pages: [string, string, Locator][] = [
    ["front page", paths.home(), listingRows(page).first()],
    ["channel", paths.channel("farcaster"), listingRows(page).first()],
    ["thread", paths.post(NESTED_THREAD), page.locator(".commentarea details.comment").first()],
    ["profile", paths.user("dwr"), listingRows(page).first()],
  ];
  for (const [name, path, content] of pages) {
    const started = Date.now();
    await page.goto(path, { waitUntil: "commit" });
    await expect(content, `${name} content`).toBeVisible({ timeout: 3_000 });
    const elapsed = Date.now() - started;
    test.info().annotations.push({ type: "load time", description: `${name}: ${elapsed} ms` });
    console.log(`${name} rendered in ${elapsed} ms`);
    expect(elapsed, `${name} load time`).toBeLessThan(3_000);
  }
});

test("new sort orders casts newest first", async ({ page }) => {
  await page.goto(paths.home("sort=new"));
  const rows = listingRows(page);
  await expect(rows.first()).toBeVisible();
  await expect(page.locator(".tabmenu li.selected")).toHaveText("new");
  const times = await timestamps(rows);
  expect(times).toEqual([...times].sort((a, b) => b - a));
});

test("channel page works like a subreddit with hot/new/top", async ({ page, baseURL }) => {
  await page.goto(paths.channel("farcaster"));
  const rows = listingRows(page);
  await expect(rows.first()).toBeVisible();
  await expect(page.locator(".pagename")).toHaveText("farcaster");
  await expect(page.locator(".titlebox .subscribers .number")).toHaveText(/\d/);
  expect(await rows.count()).toBeGreaterThanOrEqual(5);
  for (const tagline of await rows.locator(".tagline").allTextContents()) {
    expect(tagline).toContain("to /c/farcaster");
  }
  await settle(page);
  await page.screenshot({ path: `${SHOTS}/channel-page.png` });

  await page.locator(".tabmenu a", { hasText: "top" }).click();
  await expect(page).toHaveURL(absolute(paths.channel("farcaster", "sort=top"), baseURL));
  await expect(page.locator(".tabmenu li.selected")).toHaveText("top");
  const scores = await page
    .locator("#siteTable .thing.link")
    .evaluateAll((els) =>
      els.map((el) => {
        const [likes, recasts] = (el.querySelector(".score")?.getAttribute("title") ?? "")
          .match(/\d[\d,]*/g)!
          .map((n) => Number(n.replace(/,/g, "")));
        return likes + 2 * recasts;
      }),
    );
  expect(scores).toEqual([...scores].sort((a, b) => b - a));
});

test("post page shows a threaded reply tree", async ({ page }) => {
  // From the listing: the most-discussed trending cast opens with its replies.
  await page.goto(paths.home("sort=top"));
  const rows = listingRows(page);
  await expect(rows.first()).toBeVisible();
  const counts = await rows
    .locator("a.comments")
    .evaluateAll((els) => els.map((el) => Number((el.textContent ?? "0").replace(/\D/g, ""))));
  const busiest = counts.indexOf(Math.max(...counts));
  await rows.nth(busiest).locator("a.comments").click();
  await expect(page).toHaveURL(POST_URL);
  await expect(page.locator(".commentarea details.comment").first()).toBeVisible();
  await expect(page.locator(".panestack-title")).toContainText("comment");

  // A known thread with nested replies; its author is flagged as the submitter.
  await page.goto(paths.post(NESTED_THREAD));
  await expect(page.locator(".commentarea .comment .child details.comment").first()).toBeVisible();
  await expect(page.locator(".comment .userattrs .submitter").first()).toHaveText("S");

  await page.locator(".commentarea").scrollIntoViewIfNeeded();
  await settle(page);
  await page.screenshot({ path: `${SHOTS}/post-page.png` });
  await page.evaluate(() => window.scrollTo(0, 0));
  await settle(page);
  await page.screenshot({ path: `${SHOTS}/post-page-top.png` });
});

test("a reply with children collapses and expands, by click and with x", async ({ page }) => {
  await page.goto(paths.post(NESTED_THREAD));
  await waitForKeyboardNav(page);
  // The reply with the most descendants, so the whole subtree is exercised.
  const parents = page.locator(".commentarea details.comment:has(> .child details.comment)");
  await expect(parents.first()).toBeVisible();
  const sizes = await parents.evaluateAll((els) => els.map((el) => el.querySelectorAll("details.comment").length));
  const parent = parents.nth(sizes.indexOf(Math.max(...sizes)));
  const descendants = parent.locator(".child details.comment");
  const total = Math.max(...sizes);
  expect(total).toBeGreaterThan(0);
  await expect(descendants).toHaveCount(total);
  await expect(descendants.first()).toBeVisible();
  const numChildren = parent.locator(":scope > summary .numchildren");
  const marker = () =>
    parent.locator(":scope > summary .expand").evaluate((el) => getComputedStyle(el, "::before").content);
  await expect(numChildren).toBeHidden();
  expect(await marker()).toBe('"[–]"');

  // Click [–]: the reply folds, every descendant disappears, and the header
  // reports how many replies are hidden.
  await parent.locator(":scope > summary .expand").click();
  await expect(parent).not.toHaveAttribute("open", "");
  expect(await marker()).toBe('"[+]"');
  for (let i = 0; i < total; i++) await expect(descendants.nth(i)).toBeHidden();
  await expect(numChildren).toBeVisible();
  await expect(numChildren).toHaveText(`(${total} ${total === 1 ? "child" : "children"})`);
  await expect(parent.locator(":scope > .entry")).toBeHidden();
  await page.screenshot({ path: `${SHOTS}/comment-collapsed.png` });

  // Click [+]: everything comes back.
  await parent.locator(":scope > summary .expand").click();
  await expect(parent).toHaveAttribute("open", "");
  for (let i = 0; i < total; i++) await expect(descendants.nth(i)).toBeVisible();
  await expect(numChildren).toBeHidden();
  expect(await marker()).toBe('"[–]"');

  // Keyboard: select the reply (clicking blank space in its button row, since
  // a click on the header itself would toggle it), then x folds and unfolds it.
  const buttons = parent.locator(":scope > .entry > .buttons");
  const box = (await buttons.boundingBox())!;
  await buttons.click({ position: { x: box.width - 4, y: box.height / 2 } });
  await expect(parent).toHaveClass(/kb-active/);
  await expect(parent).toHaveAttribute("open", "");
  await page.keyboard.press("x");
  await expect(parent).not.toHaveAttribute("open", "");
  await expect(descendants.first()).toBeHidden();
  await page.keyboard.press("x");
  await expect(parent).toHaveAttribute("open", "");
  await expect(descendants.first()).toBeVisible();
});

test("user profile has a header and lists their casts and replies", async ({ page, baseURL }) => {
  await page.goto(paths.user("dwr"));
  const rows = listingRows(page);
  await expect(rows.first()).toBeVisible();

  const header = page.locator(".profile-header");
  await expect(header.locator(".display-name")).toHaveText("Dan Romero");
  await expect(header.locator(".handle")).toHaveText("@dwr · fid 3");
  await expect(header.locator(".followers")).toHaveText(/^\d{1,3}(,\d{3})+ followers$/);
  await expect(header.locator(".following")).toHaveText(/^\d[\d,]* following$/);
  await expect(header.locator(".bio")).toHaveText(/\S/);
  await expect(header.locator("img.pfp")).toBeVisible();
  for (const tagline of await rows.locator(".tagline").allTextContents()) {
    expect(tagline).toContain("by @dwr");
  }
  await settle(page);
  await page.screenshot({ path: `${SHOTS}/user-page.png` });

  await page.locator(".tabmenu a", { hasText: "replies" }).click();
  await expect(page).toHaveURL(absolute(paths.user("dwr", "tab=replies"), baseURL));
  await expect(header.locator(".display-name")).toHaveText("Dan Romero");
  await expect(page.locator(".reply-entry").first()).toBeVisible();
});

test("a .eth profile loads by URL with its header", async ({ page }) => {
  await page.goto(paths.user("balajis.eth"));
  const header = page.locator(".profile-header");
  await expect(header.locator(".handle")).toHaveText("@balajis.eth · fid 37");
  await expect(header.locator(".display-name")).toHaveText(/\S/);
  await expect(header.locator(".followers")).toHaveText(/^\d{1,3}(,\d{3})+ followers$/);
  await expect(header.locator(".following")).toHaveText(/^\d[\d,]* following$/);
  await expect(header.locator(".bio")).toHaveText(/\S/);
  await expect(page.locator(".error-page")).toHaveCount(0);

  const rows = listingRows(page);
  await expect(rows.first()).toBeVisible();
  for (const tagline of await rows.locator(".tagline").allTextContents()) {
    expect(tagline).toContain("by @balajis.eth");
  }
  await settle(page);
  await page.screenshot({ path: `${SHOTS}/ens-profile.png` });
});

test("author links with dotted usernames open the profile", async ({ page, baseURL }) => {
  // Prefer a dotted author on the live front page (how the bug was found);
  // fall back to a thread that is always authored by @balajis.eth.
  await page.goto(paths.home());
  await expect(listingRows(page).first()).toBeVisible();
  let link = page.locator("#siteTable a.author", { hasText: "." }).first();
  if ((await link.count()) === 0) {
    await page.goto(paths.post(NESTED_THREAD));
    await expect(listingRows(page).first()).toBeVisible();
    link = page.locator("#siteTable a.author", { hasText: "." }).first();
  }
  const handle = (await link.textContent())!.trim();
  expect(handle).toMatch(/^@\S+\.\S+$/);

  await link.click();
  await expect(page).toHaveURL(absolute(paths.user(handle.slice(1)), baseURL));
  await expect(page.locator(".profile-header .handle")).toContainText(handle);
  await expect(page.locator(".error-page")).toHaveCount(0);
});

test("j/k keyboard navigation moves between rows and o opens one", async ({ page }) => {
  await page.goto(paths.home());
  const rows = listingRows(page);
  await expect(rows.first()).toBeVisible();
  await waitForKeyboardNav(page);

  await page.keyboard.press("j");
  await expect(rows.nth(0)).toHaveClass(/kb-active/);
  await page.keyboard.press("j");
  await page.keyboard.press("j");
  await expect(rows.nth(2)).toHaveClass(/kb-active/);
  await expect(rows.nth(0)).not.toHaveClass(/kb-active/);
  await page.keyboard.press("k");
  await expect(rows.nth(1)).toHaveClass(/kb-active/);
  await page.screenshot({ path: `${SHOTS}/keyboard-nav.png` });

  const href = await rows.nth(1).locator("a.title").getAttribute("href");
  await page.keyboard.press("o");
  await expect(page).toHaveURL(new URL(href!, page.url()).href);
  await expect(page.locator(".commentarea")).toBeVisible();
});

test("the site is read-only: no sign-in, no forms that write", async ({ page }) => {
  await page.goto(paths.home());
  await expect(listingRows(page).first()).toBeVisible();
  // Only the site's own chrome: cast text can mention logging in.
  const chrome = page.locator("#header, .side, .footer");
  await expect(chrome.getByRole("link", { name: /sign in|sign up|log ?in|connect wallet/i })).toHaveCount(0);
  await expect(chrome.getByRole("button", { name: /sign in|sign up|log ?in|connect wallet/i })).toHaveCount(0);
  await expect(page.locator("form[method='post' i], input[type='password']")).toHaveCount(0);
});

test("unknown users, channels, casts and pages render not found", async ({ page }) => {
  for (const path of [
    paths.user("zzqqnotarealuser9"),
    paths.channel("zzqqnotachannel9"),
    paths.post("0x0000000000000000000000000000000000000001"),
    "no-such-page/",
  ]) {
    await page.goto(path);
    await expect(page.locator(".error-page h1"), path).toHaveText("page not found");
  }
});
