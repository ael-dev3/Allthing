// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { appCame } from "./helpers/app";
import { signIn } from "./helpers/farcaster";

// Public reaction history is a Worker stand-in; all rendering/navigation and
// account/cursor guards are the built Rust app. Private sync has its own real
// SDK protocol cases; these fixtures deliberately advertise no Library service.
const pages = Boolean(process.env.E2E_PAGES);
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const other = { fid: 5678, username: "other", display_name: "Other" };
const author = { fid: 88, username: "fixture", display_name: "Fixture Author" };
const hash = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const at = Math.floor(Date.now() / 1000);
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const cast = (n: number, text = `Library post ${n}.`) => ({ hash: hash(n), author, text, ts: at - n * 60, likes: 1, recasts: 1, replies: 0 });
const cards = (page: Page) => page.locator("#app .library-panel .cast-card[data-hash]");
type PublicPage = { fid: number; kind: string; entries: { hash: string; author_fid: number; at: number }[]; pool: ReturnType<typeof cast>[]; notes: object; next?: string; unavailable: string[]; filtered: string[]; meta: typeof meta };
const payload = (fid: number, kind: string, numbers: number[], extra: Partial<PublicPage> = {}): PublicPage => ({
  fid, kind, entries: numbers.map(n => ({ hash: hash(n), author_fid: author.fid, at: at - n })),
  pool: numbers.map(n => cast(n)), notes: {}, unavailable: [], filtered: [], meta, ...extra,
});

async function serve(page: Page) {
  await page.route("**/api/v1/**", route => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/status", route => route.fulfill({ json: { spacetime: { attest: false, library: false } } }));
  await page.route("**/api/v1/home**", route => route.fulfill({ json: { pool: [cast(1)], notes: {}, meta } }));
  await page.route("**/api/v1/cast?*", route => {
    const n = Number.parseInt(new URL(route.request().url()).searchParams.get("hash")!.slice(2), 16);
    return route.fulfill({ json: { thread: { cast: cast(n), children: [] }, ancestors: [], notes: {}, meta } });
  });
  await page.route("https://haatz.quilibrium.com/**", route => route.fulfill({ headers: { "access-control-allow-origin": "*" }, json: { messages: [] } }));
}

async function switchReader(page: Page, user: typeof me | null) {
  await page.evaluate(user => {
    document.cookie = user
      ? `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax`
      : "__Host-allthing-user=; Path=/; Secure; Max-Age=0";
    const channel = new BroadcastChannel("allthing-session");
    channel.postMessage("changed");
    channel.close();
  }, user);
}

test.describe("Library public pages and actions", () => {
  test.skip(pages, "private account routes use the Worker; static behavior is checked separately");

  test("URL tabs and Unicode filters apply only to loaded posts without another history request", async ({ page }, info) => {
    await signIn(page, me);
    await serve(page);
    const reads: { kind: string; cursor: string | null }[] = [];
    await page.route("**/api/v1/library?*", route => {
      const url = new URL(route.request().url());
      const kind = url.searchParams.get("kind")!;
      reads.push({ kind, cursor: url.searchParams.get("cursor") });
      return route.fulfill({ json: payload(me.fid, kind, [1, 2], { pool: [cast(1, "Café in Белград"), cast(2, "Another saved post")] }) });
    });
    await page.goto("/library?tab=likes");
    await expect(cards(page)).toHaveCount(2);
    await expect(page.getByRole("tab", { name: "Likes", exact: true })).toHaveAttribute("aria-selected", "true");
    await expect(page.getByText("This filters the pages you've loaded. Load more to include older posts.", { exact: true })).toBeVisible();
    await page.locator(".library-page").evaluate(node => Promise.all(node.getAnimations({ subtree: true }).filter(animation => animation.effect?.getTiming().iterations !== Infinity).map(animation => animation.finished.catch(() => {}))));
    await page.screenshot({ path: info.outputPath("library-desktop.png"), fullPage: true });
    await page.setViewportSize({ width: 390, height: 844 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath("library-phone390.png"), fullPage: true });
    await page.setViewportSize({ width: 1280, height: 900 });
    await page.getByRole("searchbox", { name: "Filter loaded posts", exact: true }).fill("белград");
    await page.getByRole("button", { name: "Filter", exact: true }).click();
    await expect(page).toHaveURL(/tab=likes.*q=/);
    await expect(cards(page)).toHaveCount(1);
    await expect(cards(page)).toContainText("Café in Белград");
    expect(reads).toEqual([{ kind: "likes", cursor: null }]);
    await page.getByRole("link", { name: "Clear", exact: true }).click();
    await expect(cards(page)).toHaveCount(2);
    await page.getByRole("tab", { name: "Likes", exact: true }).focus();
    await page.keyboard.press("ArrowRight");
    await expect(page).toHaveURL(/tab=recasts/);
    await expect(page.getByRole("tab", { name: "Recasts", exact: true })).toBeFocused();
    await expect(cards(page)).toHaveCount(2);
    expect(reads.map(read => read.kind)).toEqual(["likes", "recasts"]);
  });

  test("a failed later page keeps cards and its cursor; explicit retry deduplicates the overlap", async ({ page }) => {
    await signIn(page, me);
    await serve(page);
    let more = 0;
    const cursors: (string | null)[] = [];
    await page.route("**/api/v1/library?*", route => {
      const cursor = new URL(route.request().url()).searchParams.get("cursor");
      cursors.push(cursor);
      if (!cursor) return route.fulfill({ json: payload(me.fid, "likes", [1, 2], { next: "cGFnZTI=" }) });
      expect(cursor).toBe("cGFnZTI=");
      if (++more === 1) return route.fulfill({ status: 503, headers: { "cache-control": "no-store" }, json: { error: "Library fixture temporarily unavailable" } });
      return route.fulfill({ json: payload(me.fid, "likes", [2, 3]) });
    });
    await page.goto("/library?tab=likes");
    await expect(cards(page)).toHaveCount(2);
    const first = await cards(page).first().elementHandle();
    await page.getByRole("button", { name: "Load more", exact: true }).click();
    await expect(page.getByRole("alert")).toContainText("Library fixture temporarily unavailable");
    await expect(cards(page)).toHaveCount(2);
    await page.getByRole("button", { name: "Try again", exact: true }).click();
    await expect(cards(page)).toHaveCount(3);
    expect(await first!.evaluate(node => node.isConnected)).toBe(true);
    expect(await cards(page).evaluateAll(nodes => nodes.map(node => node.getAttribute("data-hash")))).toEqual([1, 2, 3].map(hash));
    expect(cursors).toEqual([null, "cGFnZTI=", "cGFnZTI="]);
    await expect(page.getByRole("button", { name: "Load more", exact: true })).toHaveCount(0);
  });

  test("unavailable and content-filtered history is explained instead of reported as an empty or deleted archive", async ({ page }) => {
    await signIn(page, me);
    await serve(page);
    await page.route("**/api/v1/library?*", route => route.fulfill({ json: payload(me.fid, "likes", [1, 2], { pool: [], unavailable: [hash(1)], filtered: [hash(2)] }) }));
    await page.goto("/library?tab=likes");
    await expect(page.getByText("This post was not returned by Farcaster. It may be unavailable.", { exact: true })).toBeVisible();
    await expect(page.getByText("1 loaded posts are hidden by content filters.", { exact: true })).toBeVisible();
    await expect(page.getByText("No likes were returned by Farcaster.", { exact: true })).toHaveCount(0);
    await expect(page.getByRole("link", { name: "Try opening the post", exact: true })).toHaveAttribute("href", `/p/${hash(1)}`);
  });

  test("a post opened deep in loaded reaction pages returns to the same Library cards and scroll anchor", async ({ page }) => {
    await signIn(page, me);
    await serve(page);
    const reads: string[] = [];
    await page.route("**/api/v1/library?*", route => {
      const cursor = new URL(route.request().url()).searchParams.get("cursor") ?? "";
      reads.push(cursor);
      const from = cursor ? 26 : 1;
      return route.fulfill({ json: payload(me.fid, "likes", Array.from({ length: 25 }, (_, n) => from + n), cursor ? {} : { next: "cGFnZTI=" }) });
    });
    await page.goto("/library?tab=likes");
    await expect(cards(page)).toHaveCount(25);
    await page.getByRole("button", { name: "Load more", exact: true }).click();
    const target = page.locator(`.library-panel .cast-card[data-hash="${hash(40)}"]`);
    await target.scrollIntoViewIfNeeded();
    await expect(target).toBeVisible();
    await expect(cards(page)).toHaveCount(50);
    await target.evaluate(node => Promise.all(node.getAnimations().filter(animation => animation.effect?.getTiming().iterations !== Infinity).map(animation => animation.finished.catch(() => {}))));
    // The link's actionability check can scroll after the card was brought
    // into view. Keep the place actually left at click time, as ScrollMemory
    // does, rather than an earlier position the browser may have moved from.
    const departure = await target.evaluateHandle(node => {
      const position = { top: null as number | null };
      node.addEventListener("click", () => { position.top = node.getBoundingClientRect().top; }, { capture: true, once: true });
      return position;
    });
    await target.locator("a.when").click();
    await expect(page).toHaveURL(new RegExp(`/p/${hash(40)}$`));
    const before = await departure.evaluate(position => {
      if (position.top === null) throw new Error("The departing Library card never received its click.");
      return position.top;
    });
    await departure.dispose();
    await page.goBack({ waitUntil: "commit" });
    await expect(target).toBeVisible();
    await expect(cards(page)).toHaveCount(50);
    await expect.poll(async () => Math.abs((await target.evaluate(node => node.getBoundingClientRect().top)) - before)).toBeLessThanOrEqual(2);
    expect(reads).toEqual(["", "cGFnZTI="]);
    await expect(page.getByRole("tab", { name: "Likes", exact: true })).toHaveAttribute("aria-selected", "true");
  });

  test("a delayed prior-account reaction page cannot enter the next reader's Library", async ({ page }) => {
    await signIn(page, me);
    await serve(page);
    let arrived!: () => void, release!: () => void;
    const asked = new Promise<void>(resolve => { arrived = resolve; });
    const hold = new Promise<void>(resolve => { release = resolve; });
    const errors: string[] = [];
    page.on("pageerror", error => errors.push(error.message));
    await page.route("**/api/v1/library?*", async route => {
      const fid = Number(new URL(route.request().url()).searchParams.get("fid"));
      if (fid === me.fid) { arrived(); await hold; }
      return route.fulfill({ json: payload(fid, "likes", [fid === me.fid ? 1 : 2]) });
    });
    await page.goto("/library?tab=likes");
    await asked;
    await switchReader(page, other);
    await expect(cards(page)).toContainText("Library post 2.");
    release();
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    await expect(cards(page)).toHaveCount(1);
    await expect(cards(page)).not.toContainText("Library post 1.");
    expect(errors).toEqual([]);
    await switchReader(page, null);
    await expect(cards(page)).toHaveCount(0);
    await expect(page.getByText("Sign in to see your Library.", { exact: true })).toBeVisible();
  });

  test("saving while signed out opens sign-in rather than a public Farcaster reaction or write-key approval", async ({ page }) => {
    await serve(page);
    const writes: string[] = [];
    page.on("request", request => {
      if (request.method() === "POST" && (/submitMessage|auth\/signer/.test(request.url()))) writes.push(request.url());
    });
    await page.route("**/api/v1/auth/start", route => route.fulfill({ status: 503, json: { error: "Fixture sign-in service unavailable" } }));
    await page.goto("/");
    await appCame(page);
    await page.locator("#app .cast-card").first().getByRole("button", { name: "Save post", exact: true }).click();
    await expect(page.getByRole("dialog", { name: "Sign in with Farcaster", exact: true })).toBeVisible();
    await expect(page.getByRole("heading", { name: "Approve Allthing to write from here", exact: true })).toHaveCount(0);
    expect(writes).toEqual([]);
    expect(new URL(page.url()).pathname).toBe("/");
  });

  test("a fresh unconfigured private Library reports unavailable without claiming the history is empty", async ({ page }) => {
    await signIn(page, me);
    await serve(page);
    let reads = 0;
    await page.route("**/api/v1/library**", route => { reads++; return route.fulfill({ status: 500, json: {} }); });
    await page.goto("/library");
    await expect(page.getByRole("tab", { name: "Bookmarks", exact: true })).toHaveAttribute("aria-selected", "true");
    await expect(page.getByText("Your private Library is unavailable until sync reconnects. No empty history is assumed.", { exact: true })).toBeVisible();
    await expect(page.getByText("No bookmarks yet. Save a post to find it here.", { exact: true })).toHaveCount(0);
    expect(reads).toBe(0);
  });
});

test.describe("Library static boundary", () => {
  test.skip(!pages, "public-only static copy");
  test("the static Library explains its sign-in boundary and makes no private SDK or API calls", async ({ page }) => {
    const privateRequests: string[] = [];
    page.on("request", request => { if (/maincloud\.spacetimedb|\/auth\/attest|\/api\/v1\/library/.test(request.url())) privateRequests.push(request.url()); });
    await page.goto("/Allthing/library");
    await expect(page.getByRole("heading", { name: "Library", exact: true })).toBeVisible();
    await expect(page.getByText("Your Library needs a sign-in on Allthing's server.", { exact: true })).toBeVisible();
    await expect(page.getByRole("link", { name: "Open your Library on allthing.social", exact: true })).toHaveAttribute("href", "https://allthing.social/library");
    await expect(page.getByRole("button", { name: "Save post", exact: true })).toHaveCount(0);
    expect(privateRequests).toEqual([]);
  });
});
