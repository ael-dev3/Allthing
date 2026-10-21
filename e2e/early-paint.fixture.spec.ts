// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page, type Request } from "@playwright/test";

/*
 * The front page's first cards, drawn by web/paint.js from the early fetch
 * while the app's WebAssembly is on its way: the cards the app draws first,
 * in its order and with its markup, inert and hidden from screen readers.
 * A single image's tile has its box before the image comes, and no image is
 * asked for until the module is in. Then the app's own cards take their
 * places, in the same boxes, with no second entrance. Pages it doesn't draw,
 * payloads it can't trust and an app that can't load are here too.
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
// that ends the early cards (the app draws a quoted cast; they don't).
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
const busy = {
  hash: hex(3),
  author: cat,
  text: "Worth a read",
  ts: now - 40.5 * HOUR,
  likes: 12_345,
  recasts: 1_200,
  replies: 1,
  embeds: [{ kind: "link", url: "https://example.org/essay", title: "An essay", image_url: "https://imagedelivery.net/fixture/thumb/original" }],
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
const later = { hash: hex(6), author: cat, text: "Later on", ts: now - 4.5 * HOUR, likes: 3, recasts: 0, replies: 0 };
// Enough casts with a known app for the usage bar.
const rest = Array.from({ length: 20 }, (_, i) => ({ hash: hex(100 + i), author: dee, text: `Quiet ${i}`, ts: now - 8.5 * HOUR - i * 60, likes: 1, recasts: 0, replies: 0 }));
const pool = [later, quoting, pair, busy, rich, favorite, ...rest];
const painted = [favorite, rich, busy, pair].map((c) => c.hash);
const origins = Object.fromEntries(pool.map((c, i) => [c.hash, { signer: `0x${"ab".repeat(32)}`, app_fid: i % 5 === 4 ? 2904 : 9152 }]));
const apps = {
  9152: { fid: 9152, username: "farcaster", display_name: "Farcaster" },
  2904: { fid: 2904, username: "wake", display_name: "wake" },
};
const apps24h = { to: now - 600, counted: now - 590, active: 120, apps: [{ app: 9152, active: 110 }, { app: 2904, active: 9 }], none: 1 };
const payload = { pool, notes: { origins, apps }, meta, apps_24h: apps24h };

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

for (const width of [1280, 390]) {
  test(`the front page's first cards come before the app, as it draws them, and its own take their places at width ${width}`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 844 });
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
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
    expect(probes).toEqual(["https://imagedelivery.net/fixture/wide/fit=scale-down,w=720,format=json"]);
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
    const entrances = await page.evaluate(
      (n) => [...document.querySelectorAll("#app .page > *, #app .share-bar"), ...[...document.querySelectorAll("#app .cast-list > .cast-card")].slice(0, n)].map((e) => e.getAnimations().length),
      painted.length,
    );
    expect(entrances).toEqual(entrances.map(() => 0));
    expect(entrances.length).toBe(2 + painted.length);
    // The cards after them come in as they always do.
    await expect(page.locator(`#app .cast-card[data-hash="${quoting.hash}"]`)).toBeVisible();
    await page.screenshot({ path: info.outputPath(`app-${width}.png`) });
    expect(errors).toEqual([]);
  });
}

test("top's first cards come in top's order, with the time range the address names", async ({ page }) => {
  const held = await hold(page);
  await serve(page);
  await favorites(page);
  await page.goto("/?sort=top&t=week");
  const boot = page.locator("#boot");
  // Top: cat's 14,746 points, bob's 10 counted 4 times, ann's 28.
  await expect(boot.locator(".cast-card")).toHaveCount(4);
  expect(await boot.locator(".cast-card").evaluateAll((cards) => cards.map((c) => (c as HTMLElement).dataset.hash))).toEqual([busy.hash, favorite.hash, rich.hash, pair.hash]);
  await expect(boot.locator(".sort-tabs a.selected")).toHaveText("top");
  await expect(boot.locator(".time-filter option[selected]")).toHaveText("Past week");
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
