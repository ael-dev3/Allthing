// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page, type Request } from "@playwright/test";
import { signIn } from "./helpers/farcaster";

/*
 * The front page's first cards, drawn by web/paint.js from the early fetch
 * while the app's WebAssembly is on its way: the cards the app draws first,
 * in its order and with its markup, inert and hidden from screen readers.
 * A single image's tile has its box before the image comes, and no image is
 * asked for until the module is in. Then the app's own cards take their
 * places, in the same boxes, with no second entrance. Pages it doesn't draw,
 * payloads it can't trust and an app that can't load are here too. Signed-in
 * visits wait for the app to reconcile private reading history before any card.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no early fetch");

const now = Math.floor(Date.now() / 1000);
const HOUR = 3600;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const person = (fid: number, username: string) => ({
  fid,
  username,
  display_name: username[0].toUpperCase() + username.slice(1),
  pfp_url: `https://imagedelivery.net/fixture/pfp-${username}/original`,
});
const [ann, bob, cat, dee] = [person(1, "ann"), person(2, "bob"), person(3, "cat"), person(4, "dee")];

// Hot order, with bob a favorite: bob's, ann's, cat's, dee's, then the quote
// of a cast the payload doesn't carry (a link), then cat's. On a phone the
// early cards end after the fourth, where the app puts its poll (poll.rs).
const favorite = { hash: hex(1), author: bob, text: "Plain words.\n\nA second paragraph.", ts: now - 1.5 * HOUR, likes: 10, recasts: 0, replies: 0 };
const rich = {
  hash: hex(2),
  author: ann,
  text: "Morning @bob, the notes are at https://example.com/notes/today and more in /books :pepe_clap:",
  mentions: [{ fid: 2, username: "bob" }],
  ts: now - 1.25 * HOUR,
  likes: 20,
  recasts: 3,
  replies: 2,
  channel: { id: "books", label: "books" },
  embeds: [{ kind: "image", url: "https://imagedelivery.net/fixture/wide/original" }],
};
// A quote, as the front page often has among its first (the early cards
// used to stop at one, so none were drawn): a carried quote with a single
// image, a mention and a hidden quote of its own, and one the payload doesn't
// carry, a link.
const busy = {
  hash: hex(3),
  author: cat,
  text: "Worth a read",
  // Inside the 24 hours the front page shows, below ann's in hot.
  ts: now - 23.5 * HOUR,
  likes: 900,
  recasts: 100,
  replies: 1,
  embeds: [
    { kind: "link", url: "https://example.org/essay", title: "An essay", image_url: "https://imagedelivery.net/fixture/thumb/original" },
    { kind: "cast", fid: 3, hash: hex(50) },
    { kind: "cast", fid: 4, hash: hex(52) },
  ],
};
const pair = {
  hash: hex(4),
  author: dee,
  text: "Two of them",
  ts: now - 2.5 * HOUR,
  likes: 5,
  recasts: 0,
  replies: 0,
  // GIFs, one by its path and one by its host: their tiles are marked as the app's are.
  embeds: [{ kind: "image", url: "https://media.example/one.gif" }, { kind: "image", url: "https://media.tenor.com/fixtureAAAAe/two.webp" }],
};
const quoting = { hash: hex(5), author: ann, text: "Quoting", ts: now - 3.5 * HOUR, likes: 4, recasts: 0, replies: 0, embeds: [{ kind: "cast", fid: 2, hash: favorite.hash }] };
const quoted = {
  hash: hex(50),
  author: cat,
  text: "Quoted, with a picture, for @bob",
  mentions: [{ fid: 2, username: "bob" }],
  ts: now - 30 * HOUR,
  likes: 0,
  recasts: 0,
  replies: 0,
  embeds: [
    { kind: "image", url: "https://imagedelivery.net/fixture/tall/original" },
    { kind: "cast", fid: 2, hash: hex(51) },
  ],
};
const hiddenQuote = { hash: hex(51), author: bob, text: "", ts: now - 40 * HOUR, likes: 0, recasts: 0, replies: 0, mini_app: true };
const later = { hash: hex(6), author: cat, text: "Later on", ts: now - 4.5 * HOUR, likes: 3, recasts: 0, replies: 0 };
// Enough casts with a known app for the usage bar.
const rest = Array.from({ length: 20 }, (_, i) => ({ hash: hex(100 + i), author: dee, text: `Quiet ${i}`, ts: now - 8.5 * HOUR - i * 60, likes: 1, recasts: 0, replies: 0 }));
const pool = [later, quoting, pair, busy, rich, favorite, ...rest];
const first = [favorite, rich, busy, pair, quoting, later].map((c) => c.hash);
/** The early cards at a width: six, or four where the poll follows the fourth. */
const paintedAt = (width: number) => first.slice(0, width >= 1280 ? 6 : 4);
const painted = paintedAt(1280);
const origins = Object.fromEntries(pool.map((c, i) => [c.hash, { signer: `0x${"ab".repeat(32)}`, app_fid: i % 5 === 4 ? 2904 : 9152 }]));
const apps = {
  9152: { fid: 9152, username: "farcaster", display_name: "Farcaster" },
  2904: { fid: 2904, username: "wake", display_name: "wake" },
};
const apps24h = { to: now - 600, counted: now - 590, active: 120, apps: [{ app: 9152, active: 110 }, { app: 2904, active: 9 }], none: 1 };
const quotes = { [quoted.hash]: quoted, [hiddenQuote.hash]: hiddenQuote };
const payload = { pool, notes: { origins, apps, quotes }, meta, apps_24h: apps24h };

const svg = (w: number, h: number) => `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}"><rect width="${w}" height="${h}" fill="#4f43d4"/></svg>`;

/** The fixture's endpoints and images; what images were asked for, in order. */
async function serve(page: Page, body: object = payload, headers: Record<string, string> = {}) {
  const images: string[] = [];
  const probes: string[] = [];
  page.on("request", (request: Request) => {
    const url = request.url();
    if (url.endsWith("format=json")) probes.push(url);
    else if (/^https:\/\/(imagedelivery\.net|media\.example|media\.tenor\.com)\//.test(url) || url.includes("/assets/emoji/")) images.push(url);
  });
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  for (const path of ["**/api/v1/home**", "**/api/v1/latest**", "**/api/v1/channel?*", "**/api/v1/user?*"]) {
    await page.route(path, (route) => route.fulfill({ json: body, headers }));
  }
  const cors = { "access-control-allow-origin": "*" };
  await page.route("https://imagedelivery.net/**", (route) => {
    const url = route.request().url();
    // Cloudflare Images describes a variant as JSON: the wide image is 1440 × 720.
    if (url.endsWith("/fit=scale-down,w=720,format=json")) return route.fulfill({ json: { width: 720, height: 360 }, headers: cors });
    const size = url.includes("/fit=scale-down,") ? svg(720, 360) : url.includes("w=140") ? svg(140, 140) : svg(64, 64);
    return route.fulfill({ body: size, contentType: "image/svg+xml", headers: cors });
  });
  await page.route(/^https:\/\/(media\.example|media\.tenor\.com)\//, (route) => route.fulfill({ body: svg(600, 600), contentType: "image/svg+xml" }));
  return { images, probes };
}

/** Holds the app's WebAssembly and JavaScript until each is let through (or fails). */
async function hold(page: Page) {
  const gate = async (pattern: string) => {
    let open: (how: "continue" | "abort") => void = () => {};
    const opened = new Promise<"continue" | "abort">((done) => (open = done));
    await page.route(pattern, async (route) => ((await opened) === "abort" ? route.abort("failed") : route.continue()));
    return open;
  };
  return { wasm: await gate("**/{assets,wasm}/allthing-*.wasm"), js: await gate("**/assets/allthing-*/allthing.js") };
}

const favorites = (page: Page) =>
  page.addInitScript(() => localStorage.setItem("allthing.favorites", JSON.stringify([{ fid: 2, username: "bob" }])));

/** The front page under `root` as it shows: each part's box and markup, its
 * attributes in order, without the entrance the hand-over turns off. */
const shown = (page: Page, root: string) =>
  page.evaluate((root) => {
    const markup = (node: Element) => {
      const copy = node.cloneNode(true) as Element;
      // Saving is an app-only private action. Compare the anonymous early
      // card's original content exactly; whole-card boxes still include the
      // mounted action, whose existence/accessibility is asserted separately.
      copy.querySelectorAll(".bookmark-action").forEach((action) => action.remove());
      const walker = document.createTreeWalker(copy, NodeFilter.SHOW_ALL);
      const drop: Node[] = [];
      for (let n: Node | null = walker.currentNode; n; n = walker.nextNode()) {
        if (n.nodeType === Node.COMMENT_NODE || (n.nodeType === Node.TEXT_NODE && n.textContent === "")) drop.push(n);
        if (n instanceof Element) {
          const attrs = [...n.attributes].map((a) => [a.name, a.value]).sort(([a], [b]) => a.localeCompare(b));
          for (const [name] of attrs) n.removeAttribute(name);
          // What only screen readers use, which the early cards are hidden
          // from: the app's popover ids, and each emoji's description.
          const unread = (name: string) => name === "aria-controls" || (name === "aria-description" && n.matches("picture.emoji img"));
          for (const [name, value] of attrs.filter(([name]) => !unread(name))) {
            const kept = name === "style" ? value.replace(/\s*animation-name: none;?/, "").trim().replace(/;$/, "") : value;
            if (name !== "style" || kept) n.setAttribute(name, kept);
          }
        }
      }
      drop.forEach((n) => n.parentNode?.removeChild(n));
      return copy.outerHTML;
    };
    const box = (node: Element) => {
      const r = node.getBoundingClientRect();
      return [r.x, r.y + scrollY, r.width, r.height].map(Math.round);
    };
    const parts = [...document.querySelectorAll(`${root} .listing-head, ${root} .client-share, ${root} .cast-list > .cast-card`)];
    return parts.map((part) => ({ hash: (part as HTMLElement).dataset.hash ?? part.className, box: box(part), markup: markup(part) }));
  }, root);

/** Once the entrances under `root` (the page's, the usage bar's, the
 * cards') have run: boxes measured during one are moved by its transform. */
const settled = (page: Page, root: string) =>
  expect
    .poll(() =>
      page.evaluate(
        (root) =>
          document
            .getAnimations()
            .filter((a) => a.effect?.getTiming().iterations !== Infinity && ((a.effect as KeyframeEffect).target as Element)?.closest(root))
            .every((a) => a.playState === "finished"),
        root,
      ),
    )
    .toBe(true);

/** Lets the module through, and waits for the early cards' images to be in. */
async function moduleIn(page: Page, held: Awaited<ReturnType<typeof hold>>) {
  const boot = page.locator("#boot");
  held.wasm("continue");
  await expect(boot.locator('img[src^="data:"]')).toHaveCount(0);
  await expect
    .poll(() => boot.locator("img").evaluateAll((imgs) => imgs.every((img) => (img as HTMLImageElement).complete && (img as HTMLImageElement).naturalWidth > 0)))
    .toBe(true);
}

test("future dates never appear in early paint or after the app takes over", async ({ page }) => {
  const held = await hold(page);
  const future = { ...favorite, hash: hex(900), ts: 2_893_718_638, likes: 1_000_000, embeds: [] };
  await serve(page, { ...payload, pool: [future, ...pool] });
  await favorites(page);
  await page.goto("/");
  const boot = page.locator("#boot");
  await expect.poll(() => boot.locator(".cast-card").evaluateAll((cards) => cards.map((card) => card.getAttribute("data-hash")))).toEqual(painted);
  await expect(boot.locator(`[data-hash="${future.hash}"]`)).toHaveCount(0);
  await moduleIn(page, held);
  held.js("continue");
  await expect(boot).toHaveCount(0);
  await expect(page.locator("#app .cast-card").first()).toBeVisible();
  await expect(page.locator(`#app [data-hash="${future.hash}"]`)).toHaveCount(0);
});

for (const width of [1280, 390]) {
  test(`the front page's first cards come before the app, as it draws them, and its own take their places at width ${width}`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 844 });
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const painted = paintedAt(width);
    const held = await hold(page);
    const { images, probes } = await serve(page);
    await favorites(page);
    await page.goto("/");

    // Before the app: its first cards, in its order, out of reach.
    const boot = page.locator("#boot");
    await expect(boot.locator(".cast-card")).toHaveCount(painted.length);
    expect(await boot.locator(".cast-card").evaluateAll((cards) => cards.map((c) => (c as HTMLElement).dataset.hash))).toEqual(painted);
    await expect(boot).toHaveAttribute("data-painted", "");
    await expect(boot).toHaveAttribute("aria-hidden", "true");
    expect(await boot.locator("main").evaluate((main) => (main as HTMLElement).inert)).toBe(true);
    await expect(page.locator("#app")).toBeEmpty();
    await expect(page.getByRole("img", { name: "One of your favorites" })).toHaveCount(0);
    // The single image's tile has its box from Cloudflare Images' description
    // of it, and no image is asked for while the module is on its way.
    const tile = boot.locator(`.cast-card[data-hash="${rich.hash}"] .tile-image`);
    await expect(tile).toHaveAttribute("data-sized", "");
    await expect(tile).not.toHaveClass(/(^| )gif( |$)/);
    await expect(boot.locator(`.cast-card[data-hash="${pair.hash}"] .tile-image.gif`)).toHaveCount(2);
    expect(probes).toEqual(["https://imagedelivery.net/fixture/wide/fit=scale-down,w=720,format=json", "https://imagedelivery.net/fixture/tall/fit=scale-down,w=720,format=json"]);
    // The quote's card, its image's tile sized, its hidden quote and the link to the one not carried.
    const quote = boot.locator(`.cast-card[data-hash="${busy.hash}"] > blockquote.quote`);
    await expect(quote.locator(".tile-image")).toHaveAttribute("data-sized", "");
    await expect(quote.locator("blockquote.quote .quote-hidden")).toHaveText("Quoted cast hidden: mini app");
    await expect(boot.locator(`.cast-card[data-hash="${busy.hash}"] > a.quote-chip`)).toHaveCount(1);
    await expect(boot.locator('img:not([src^="data:"]), source[srcset]')).toHaveCount(0);
    expect(images).toEqual([]);
    await settled(page, "#boot");
    const boxed = await tile.boundingBox();

    // The module is in: the images come, into their boxes.
    await moduleIn(page, held);
    expect(await tile.boundingBox()).toEqual(boxed);
    const early = await shown(page, "#boot");
    await page.screenshot({ path: info.outputPath(`early-paint-${width}.png`) });

    // The app: its own cards in the same boxes, the same markup, no second
    // entrance, and nothing left of the early ones.
    held.js("continue");
    await expect(boot).toHaveCount(0);
    await expect(page.locator("#app .cast-card").first()).toBeVisible();
    const app = await shown(page, "#app");
    expect(app.slice(0, early.length)).toEqual(early);
    for (const hash of painted) {
      await expect(page.locator(`#app .cast-list > .cast-card[data-hash="${hash}"]`).getByRole("button", { name: "Save post", exact: true })).toBeVisible();
    }
    const entrances = await page.evaluate(
      (n) => [...document.querySelectorAll("#app .page > *, #app .share-bar"), ...[...document.querySelectorAll("#app .cast-list > .cast-card")].slice(0, n)].map((e) => e.getAnimations().length),
      painted.length,
    );
    expect(entrances).toEqual(entrances.map(() => 0));
    expect(entrances.length).toBe(2 + painted.length);
    // The cards after them come in as they always do.
    await expect(page.locator(`#app .cast-card[data-hash="${rest[0].hash}"]`)).toHaveCount(1);
    await page.screenshot({ path: info.outputPath(`app-${width}.png`) });
    expect(errors).toEqual([]);
  });
}

test("with most of the casts' apps still being read, the early usage bar is the app's: the known as shares of all, the rest in grey", async ({ page }) => {
  const held = await hold(page);
  // The first five casts' apps are known; the other 21 are being read.
  const known = Object.fromEntries(pool.slice(0, 5).map((c) => [c.hash, origins[c.hash]]));
  await serve(page, { ...payload, notes: { ...payload.notes, origins: known, pending: pool.length - 5 } });
  await page.goto("/");
  const boot = page.locator("#boot");
  await expect(boot.locator(".cast-card")).toHaveCount(painted.length);
  const bar = boot.locator(".client-share");
  await expect(bar.locator(".client-legend li")).toHaveText([/^Farcaster\s*15%$/, /^wake\s*4%$/, /^21 not known yet$/]);
  await expect(bar.locator(".share-bar > .seg-unknown")).toHaveCount(1);
  await settled(page, "#boot");
  await moduleIn(page, held);
  const early = await shown(page, "#boot");
  held.js("continue");
  await expect(boot).toHaveCount(0);
  await expect(page.locator("#app .cast-card").first()).toBeVisible();
  const app = await shown(page, "#app");
  expect(app.slice(0, early.length)).toEqual(early);
});

test("top's first cards come in top's order, with the time range the address names", async ({ page }) => {
  const held = await hold(page);
  await serve(page);
  await favorites(page);
  await page.goto("/?sort=top&t=week");
  const boot = page.locator("#boot");
  // Top: cat's 14,746 points, bob's 10 (and one) counted 25 times, ann's 28.
  await expect(boot.locator(".cast-card")).toHaveCount(6);
  expect(await boot.locator(".cast-card").evaluateAll((cards) => cards.map((c) => (c as HTMLElement).dataset.hash))).toEqual([busy, favorite, rich, pair, quoting, later].map((c) => c.hash));
  await expect(boot.locator(".sort-tabs a.selected")).toHaveText("top");
  await expect(boot.locator(".time-filter option[selected]")).toHaveText("7 days");
  await moduleIn(page, held);
  await settled(page, "#boot");
  const early = await shown(page, "#boot");
  held.js("continue");
  await expect(boot).toHaveCount(0);
  expect((await shown(page, "#app")).slice(0, early.length)).toEqual(early);
});

/** Records even a transient boot card: signed-in history must never flash. */
async function observeSignedInBoot(page: Page) {
  await page.addInitScript(() => {
    const hashes: string[] = [];
    Object.defineProperty(window, "__signedInEarlyHashes", { value: hashes });
    const record = () => document.querySelectorAll("#boot .cast-card[data-hash]").forEach(card => hashes.push(card.getAttribute("data-hash")!));
    new MutationObserver(record).observe(document, { childList: true, subtree: true });
  });
}
const observedSignedInBoot = (page: Page) => page.evaluate(() => (window as Window & { __signedInEarlyHashes: string[] }).__signedInEarlyHashes);

for (const pausedAt of ["the Home response", "image sizing"]) {
  test(`an anonymous early response cannot paint after sign-in while waiting for ${pausedAt}`, async ({ page }) => {
    const held = await hold(page);
    await serve(page, { pool: [rich], notes: {}, meta });
    await observeSignedInBoot(page);
    let reached!: () => void, release!: () => void;
    const asked = new Promise<void>(resolve => { reached = resolve; });
    const waiting = new Promise<void>(resolve => { release = resolve; });
    const path = pausedAt === "the Home response" ? "**/api/v1/home**" : "**/fit=scale-down,w=720,format=json";
    await page.route(path, async route => {
      reached();
      await waiting;
      return pausedAt === "the Home response"
        ? route.fulfill({ json: { pool: [rich], notes: {}, meta } })
        : route.fulfill({ json: { width: 720, height: 360 }, headers: { "access-control-allow-origin": "*" } });
    });
    try {
      // paint.js has started for an anonymous visit, but the data/size it
      // awaits has not completed. The app remains held throughout this probe.
      await page.goto("/");
      await asked;
      await expect(page.locator("#boot .cast-card")).toHaveCount(0);
      await page.evaluate(() => {
        document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify({ fid: 1234, username: "reader", display_name: "Reader" }))}; Path=/; Secure; SameSite=Lax`;
      });
      const resumed = page.waitForResponse(response => pausedAt === "the Home response"
        ? new URL(response.url()).pathname === "/api/v1/home"
        : response.url().endsWith("/fit=scale-down,w=720,format=json"));
      release();
      await (await resumed).finished();
      await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
      await expect(page.locator("#boot .cast-card")).toHaveCount(0);
      await expect(page.locator("#boot")).not.toHaveAttribute("data-painted", "");
      expect(await observedSignedInBoot(page)).toEqual([]);
      await expect(page.locator("#app")).toBeEmpty();
    } finally {
      release(); held.wasm("continue"); held.js("continue");
    }
  });
}

test("sign-in from another tab removes already painted anonymous cards while the app is held", async ({ page }) => {
  const held = await hold(page);
  await serve(page, { pool: [favorite], notes: {}, meta });
  await observeSignedInBoot(page);
  try {
    await page.goto("/");
    await expect(page.locator("#boot .cast-card")).toHaveCount(1);
    expect([...new Set(await observedSignedInBoot(page))]).toEqual([favorite.hash]);
    await page.evaluate(() => {
      document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify({ fid: 1234, username: "reader", display_name: "Reader" }))}; Path=/; Secure; SameSite=Lax`;
      const channel = new BroadcastChannel("allthing-session");
      channel.postMessage("changed"); channel.close();
    });
    await expect(page.locator("#boot .cast-card")).toHaveCount(0);
    await expect(page.locator("#boot main")).toHaveCount(0);
    await expect(page.locator("#boot")).not.toHaveAttribute("data-painted", "");
    await expect(page.locator("#app")).toBeEmpty();
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    // The observer retains the anonymous card that preceded sign-in, but
    // no later early rendering may bring it back under the private reader.
    expect([...new Set(await observedSignedInBoot(page))]).toEqual([favorite.hash]);
  } finally {
    held.wasm("continue"); held.js("continue");
  }
});

for (const width of [1280, 390]) {
  test(`a signed-in reader's For you waits for the app, which lifts followed accounts at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    const held = await hold(page);
    const { images, probes } = await serve(page, { ...payload, pool: pool.filter((c) => c.ts > now - 20 * HOUR) });
    // This legacy-service fixture checks app ordering separately. The real
    // private SDK cases advertise library:true and prove cold seen exclusion.
    await page.route("**/api/v1/status", route => route.fulfill({ json: { spacetime: { attest: false, library: false } } }));
    await page.route("**/api/v1/notifications**", (route) => route.fulfill({ json: { groups: [], meta }, headers: { "cache-control": "private, no-store" } }));
    await signIn(page, { fid: 1234, username: "reader", display_name: "Reader" });
    await observeSignedInBoot(page);
    let releaseSigner!: () => void;
    const signer = new Promise<void>(resolve => { releaseSigner = resolve; });
    await page.route("**/api/v1/signer", async route => {
      await signer;
      await route.fulfill({ status: 404, json: { error: "not set up" } });
    });
    try {
      await page.addInitScript(() => localStorage.setItem("allthing.followed", JSON.stringify({ reader: 1234, at: Date.now(), fids: [4] })));
      const response = page.waitForResponse(response => new URL(response.url()).pathname === "/api/v1/home");
      await page.goto("/");
      await response;
      const boot = page.locator("#boot");
      await expect(boot).toBeVisible();
      await expect(boot.locator(".cast-card")).toHaveCount(0);
      await expect(boot).not.toHaveAttribute("data-painted", "");
      await expect(page.locator("#app")).toBeEmpty();
      expect(images).toEqual([]);
      expect(probes).toEqual([]);
      // Downloading the module alone still cannot reveal unconfirmed cards.
      held.wasm("continue");
      await expect(boot.locator(".cast-card")).toHaveCount(0);
      expect(await observedSignedInBoot(page)).toEqual([]);
      held.js("continue");
      await expect(boot).toHaveCount(0);
      const app = page.locator("#app");
      // Hot, dee followed: the quiet pair's five points are weighted 20×.
      await expect(app.locator(".cast-list > .cast-card").first()).toHaveAttribute("data-hash", pair.hash);
      await expect(app.getByRole("link", { name: "For you", exact: true })).toHaveAttribute("aria-current", "page");
      await expect(app.locator(".feed-tabs a")).toHaveCount(2);
      const permissionsRead = page.waitForResponse("**/api/v1/signer");
      releaseSigner();
      await permissionsRead;
      await expect(app.locator(".cast-card").first().getByRole("button", { name: "Like", exact: true })).toHaveAttribute("aria-haspopup", "dialog");
      expect(await observedSignedInBoot(page)).toEqual([]);
    } finally {
      releaseSigner();
    }
  });
}

test("a signed-in reader's Following tab isn't drawn early: the app draws it", async ({ page }) => {
  const held = await hold(page);
  await serve(page);
  await page.route("**/api/v1/status", route => route.fulfill({ json: { spacetime: { attest: false, library: false } } }));
  await page.route("**/api/v1/following**", (route) => route.fulfill({ json: { pool: [pair], notes: {}, meta }, headers: { "cache-control": "private, no-store" } }));
  await signIn(page, { fid: 1234, username: "reader", display_name: "Reader" });
  await observeSignedInBoot(page);
  const early = page.waitForResponse((response) => response.url().includes("/api/v1/following"));
  await page.goto("/?feed=following");
  await early;
  await expect(page.locator("#boot .cast-card")).toHaveCount(0);
  held.wasm("continue");
  held.js("continue");
  await expect(page.locator("#boot")).toHaveCount(0);
  await expect(page.locator("#app .cast-list > .cast-card")).toHaveCount(1);
  await expect(page.locator("#app .cast-list > .cast-card")).toHaveAttribute("data-hash", pair.hash);
  await expect(page.getByRole("link", { name: "Following", exact: true })).toHaveAttribute("aria-current", "page");
  expect(await observedSignedInBoot(page)).toEqual([]);
});

test("an account seen less of is lower in the first cards too, as in the app", async ({ page }) => {
  const held = await hold(page);
  await serve(page);
  await page.addInitScript(() => localStorage.setItem("allthing.show-less", JSON.stringify([{ fid: 1, username: "ann" }])));
  await page.goto("/");
  const boot = page.locator("#boot");
  // Hot, seeing less of ann: her cast, second without it, is as if hours
  // older and goes below dee's quiet ones, 5th, as in the app.
  await expect(boot.locator(".cast-card")).toHaveCount(6);
  expect(await boot.locator(".cast-card").evaluateAll((cards) => cards.map((c) => (c as HTMLElement).dataset.hash))).toEqual(
    [busy, favorite, pair, later, rich, rest[0]].map((c) => c.hash),
  );
  await moduleIn(page, held);
  await settled(page, "#boot");
  const early = await shown(page, "#boot");
  held.js("continue");
  await expect(boot).toHaveCount(0);
  expect((await shown(page, "#app")).slice(0, early.length)).toEqual(early);
});

test("a card with Pepe reactions keeps its box: the early card has the row their chips come into", async ({ page }) => {
  const held = await hold(page);
  const reactions = { [favorite.hash]: [{ hash: hex(900), emoji: "pepe_no", fid: 7, ts: now - 600 }] };
  await serve(page, { ...payload, notes: { ...payload.notes, reactions } });
  await favorites(page);
  await page.goto("/");
  const boot = page.locator("#boot");
  await expect(boot.locator(".cast-card")).toHaveCount(painted.length);
  await expect(boot.locator(`.cast-card[data-hash="${favorite.hash}"] .reaction-chips.coming`)).toHaveCount(1);
  await expect(boot.locator(".reaction-chips")).toHaveCount(1);
  await moduleIn(page, held);
  await settled(page, "#boot");
  const boxes = async (root: string) => (await shown(page, root)).map(({ hash, box }) => ({ hash, box }));
  const early = await boxes("#boot");
  held.js("continue");
  await expect(boot).toHaveCount(0);
  // The chips themselves, in the row's place, with nothing moved.
  await expect(page.locator(`#app .cast-card[data-hash="${favorite.hash}"] .reaction-chip`)).toHaveCount(1);
  expect((await boxes("#app")).slice(0, early.length)).toEqual(early);
});

for (const where of ["cast", "quote"]) {
  test(`shown mini app ${where} waits for the app instead of changing the early card`, async ({ page }) => {
    const held = await hold(page);
    await page.addInitScript(() => localStorage.setItem("allthing.show-mini-apps", "1"));
    const cast = where === "cast"
      ? { ...favorite, mini_app: true }
      : { ...favorite, embeds: [{ kind: "cast", hash: hiddenQuote.hash, fid: bob.fid }] };
    await serve(page, { ...payload, pool: [cast] });
    const response = page.waitForResponse((response) => response.url().includes("/api/v1/home"));
    await page.goto("/");
    await response;
    // Let paint.js consume the early response while the app stays held.
    await page.waitForTimeout(300);
    await expect(page.locator("#boot")).not.toHaveAttribute("data-painted", "");
    await expect(page.locator("#boot .cast-card")).toHaveCount(0);
    held.wasm("continue");
    held.js("continue");
    await expect(page.locator("#boot")).toHaveCount(0);
    const card = page.locator(`#app .cast-card[data-hash="${cast.hash}"]`);
    await expect(card).toBeVisible();
    await expect(card.locator(".mini-app-link")).toHaveCount(1);
  });
}

for (const path of ["/?sort=new", "/?client=9152", "/?t=month", "/?t=all", "/c/books", "/u/ann"]) {
  test(`nothing is drawn early for ${path}`, async ({ page }) => {
    const held = await hold(page);
    await serve(page);
    const early = page.waitForResponse((response) => response.url().includes("/api/v1/"));
    await page.goto(path);
    await early;
    await page.waitForTimeout(300);
    await expect(page.locator("#boot")).not.toHaveAttribute("data-painted", "");
    await expect(page.locator("#boot .cast-card")).toHaveCount(0);
    held.wasm("continue");
    held.js("continue");
    await expect(page.locator("#boot")).toHaveCount(0);
  });
}

for (const [name, status, headers] of [
  ["a copy the Worker says is stale", 200, { "x-allthing-stale": "1" }],
  ["an error", 503, {}],
] as const) {
  test(`nothing is drawn early from ${name}`, async ({ page }) => {
    const held = await hold(page);
    await serve(page, status === 200 ? payload : { error: "Upstream unavailable" }, headers);
    if (status !== 200) await page.route("**/api/v1/home**", (route) => route.fulfill({ status, json: { error: "Upstream unavailable" } }));
    const early = page.waitForResponse((response) => response.url().includes("/api/v1/home"));
    await page.goto("/");
    await early;
    await page.waitForTimeout(300);
    await expect(page.locator("#boot .cast-card")).toHaveCount(0);
    held.wasm("continue");
    held.js("continue");
    await expect(page.locator("#boot")).toHaveCount(0);
  });
}

test("if the app can't load, its panel shows instead of the early cards", async ({ page }) => {
  const held = await hold(page);
  await serve(page);
  held.js("continue");
  await page.goto("/");
  await expect(page.locator("#boot .cast-card")).toHaveCount(painted.length);
  held.wasm("abort");
  await expect(page.getByRole("alert")).toContainText("Allthing couldn’t load");
  await expect(page.getByRole("button", { name: "Reload", exact: true })).toBeVisible();
  await expect(page.locator("#boot .cast-card")).toHaveCount(0);
  await expect(page.locator("#boot")).not.toHaveAttribute("data-painted", "");
});
