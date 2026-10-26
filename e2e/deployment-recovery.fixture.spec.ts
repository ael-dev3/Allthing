// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type APIRequestContext, type Page } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker deployment contract");

const recoveryQuery = "__allthing_boot";
const recoveryKey = "allthing-boot-recovery";
const stale = "0000000000";
const minimalWasm = Buffer.from([0, 97, 115, 109, 1, 0, 0, 0]);
const start = "/?sort=top&t=week&note=hello%20world&note=again#recovery";

type Fresh = "valid" | "same" | "mixed" | "external" | "oversized" | "redirect" | "missing";

// Serve the actual generated loader with an older build fingerprint, rather
// than replacing its recovery code. Only that old build's assets are absent;
// a successful recovery boots the real current app and its real Wasm.
async function staleDocument(page: Page, request: APIRequestContext, options: {
  asset?: "wasm" | "js";
  fresh?: Fresh;
  staleNavigation?: boolean;
} = {}) {
  const html = await (await request.get("/")).text();
  const hash = html.match(/\/wasm\/allthing-([0-9a-f]{10})\.wasm/)?.[1];
  const bootPath = html.match(/<script type="module" src="([^"]*\/boot-[0-9a-f]+\.js)"/)?.[1];
  expect(hash).toBeTruthy();
  expect(hash).not.toBe(stale);
  expect(bootPath).toBeTruthy();
  const boot = await (await request.get(bootPath!)).text();
  const oldHTML = html.replaceAll(hash!, stale);
  const oldBoot = boot.replaceAll(hash!, stale);
  const navigations: string[] = [];
  const probes: { url: string; cache: string | undefined }[] = [];
  const scripts: string[] = [];
  let old = true;
  let forceCurrent = false;
  await page.route("**/*", async (route) => {
    const resource = route.request();
    const url = new URL(resource.url());
    if (url.pathname.startsWith("/api/v1/")) {
      await route.fulfill({ json: { pool: [], meta: { backend: "hypersnap", fetched_at: Date.now() } } });
    } else if (url.pathname === "/" && resource.isNavigationRequest()) {
      navigations.push(url.href);
      old = !forceCurrent && (!url.searchParams.has(recoveryQuery) || Boolean(options.staleNavigation));
      await route.fulfill({ contentType: "text/html", body: old ? oldHTML : html });
    } else if (url.pathname === "/" && url.searchParams.has(recoveryQuery)) {
      probes.push({ url: url.href, cache: resource.headers()["cache-control"] });
      const fresh = options.fresh ?? "valid";
      if (fresh === "redirect") await route.fulfill({ status: 302, headers: { location: "/?redirected=1" } });
      else if (fresh === "missing") await route.fulfill({ status: 404, contentType: "text/html", body: html });
      else {
        const body = fresh === "same" ? oldHTML
          : fresh === "mixed" ? html.replace(`/assets/allthing-${hash}/allthing.js`, `/assets/allthing-${stale}/allthing.js`)
          : fresh === "external" ? html.replace(`/assets/allthing-${hash}/allthing.js`, `https://example.invalid/assets/allthing-${hash}/allthing.js`)
          : fresh === "oversized" ? `${html}${" ".repeat(256 * 1024)}` : html;
        await route.fulfill({ contentType: "text/html; charset=utf-8", body });
      }
    } else if (url.pathname === bootPath) {
      await route.fulfill({ contentType: "text/javascript", body: old ? oldBoot : boot });
    } else if (new RegExp(`^/(?:wasm|assets)/allthing-${stale}\\.wasm$`).test(url.pathname)) {
      if (options.asset === "js") await route.fulfill({ contentType: "application/wasm", body: minimalWasm });
      else if (url.pathname.startsWith("/wasm/")) await route.fulfill({ status: 404, contentType: "text/plain", body: "old module is gone" });
      else await route.fulfill({ contentType: "text/html", body: oldHTML });
    } else if (url.pathname === `/assets/allthing-${stale}/allthing.js`) {
      scripts.push(url.href);
      await route.fulfill(options.asset === "js"
        ? { contentType: "text/html", body: oldHTML }
        : { contentType: "text/javascript", body: 'export default function () { throw new Error("old app must not start"); }' });
    } else await route.continue();
  });
  return { hash: hash!, navigations, probes, scripts, current: () => { forceCurrent = true; } };
}

async function appLoaded(page: Page) {
  await expect(page.getByRole("combobox", { name: "Search casts, people and channels", exact: true })).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.locator("#boot")).toHaveCount(0);
  const url = new URL(page.url());
  expect(url.searchParams.get(recoveryQuery)).toBeNull();
  expect(url.searchParams.get("sort")).toBe("top");
  expect(url.searchParams.get("t")).toBe("week");
  expect(url.searchParams.getAll("note")).toEqual(["hello world", "again"]);
  expect(url.hash).toBe("#recovery");
}

for (const width of [1280, 390]) {
  for (const asset of ["wasm", "js"] as const) {
    test(`a missing old ${asset} recovers only to a matching new deployment at width ${width}`, async ({ page, request }) => {
      await page.setViewportSize({ width, height: 844 });
      const errors: string[] = [];
      page.on("pageerror", (error) => errors.push(error.message));
      const proof = await staleDocument(page, request, { asset });
      await page.goto(start);
      await appLoaded(page);
      expect(proof.navigations).toHaveLength(2);
      expect(proof.probes).toHaveLength(1);
      const revalidation = new URL(proof.probes[0].url);
      expect(revalidation.searchParams.get(recoveryQuery)).toBe(stale);
      expect(revalidation.searchParams.getAll("note")).toEqual(["hello world", "again"]);
      expect(proof.probes[0].cache).toContain("no-cache");
      if (asset === "js") expect(proof.scripts.length).toBeGreaterThanOrEqual(2); // import failure, then HTTP classification
      expect(await page.evaluate((key) => sessionStorage.getItem(key), recoveryKey)).toBeNull();
      expect(errors).toEqual([]);
    });
  }
}

test("an automatic recovery cannot chain through another stale document, and manual Reload still works", async ({ page, request }) => {
  const proof = await staleDocument(page, request, { staleNavigation: true });
  await page.goto(start);
  await expect.poll(() => proof.navigations.length).toBe(2);
  await expect(page.getByRole("alert")).toContainText("Allthing couldn’t load");
  await expect(page.getByRole("button", { name: "Reload", exact: true })).toBeVisible();
  // A second missing-asset boot would otherwise immediately revalidate and
  // navigate again. It must leave the first guard intact instead.
  await page.waitForTimeout(2200);
  expect(proof.probes).toHaveLength(1);
  expect(proof.navigations).toHaveLength(2);
  expect(await page.evaluate((key) => sessionStorage.getItem(key), recoveryKey)).toBe(stale);
  proof.current();
  await page.getByRole("button", { name: "Reload", exact: true }).click();
  await appLoaded(page);
  expect(proof.navigations).toHaveLength(3);
});

for (const fresh of ["same", "mixed", "external", "oversized", "redirect", "missing"] as const) {
  test(`missing assets do not reload for ${fresh} deployment evidence`, async ({ page, request }) => {
    const proof = await staleDocument(page, request, { fresh });
    await page.goto(start);
    await expect(page.getByRole("alert")).toContainText("Allthing couldn’t load");
    await expect.poll(() => proof.probes.length).toBe(1);
    await page.waitForTimeout(2200);
    expect(proof.navigations).toHaveLength(1);
    expect(await page.evaluate((key) => sessionStorage.getItem(key), recoveryKey)).toBeNull();
    await expect(page.getByRole("button", { name: "Reload", exact: true })).toBeVisible();
  });
}

test("disabled session storage keeps missing-asset recovery manual", async ({ page, request }) => {
  await page.addInitScript((key) => {
    const original = Storage.prototype.getItem;
    Storage.prototype.getItem = function (name) {
      if (name === key) throw new DOMException("Storage disabled", "SecurityError");
      return original.call(this, name);
    };
  }, recoveryKey);
  const proof = await staleDocument(page, request);
  await page.goto(start);
  await expect(page.getByRole("alert")).toContainText("Allthing couldn’t load");
  await page.waitForTimeout(2200);
  expect(proof.probes).toHaveLength(0);
  expect(proof.navigations).toHaveLength(1);
  await expect(page.getByRole("button", { name: "Reload", exact: true })).toBeVisible();
});

test("a stalled revalidation body is aborted and leaves manual recovery usable", async ({ page, request }) => {
  const proof = await staleDocument(page, request);
  await page.addInitScript((query) => {
    const original = window.fetch;
    (window as Window & { recoveryAborted?: boolean }).recoveryAborted = false;
    window.fetch = async (input, init) => {
      const url = new URL(input instanceof Request ? input.url : String(input), location.href);
      if (url.searchParams.has(query) && url.pathname === "/") {
        init?.signal?.addEventListener("abort", () => {
          (window as Window & { recoveryAborted?: boolean }).recoveryAborted = true;
        });
        // Headers arrive; the body never closes. This exercises the loader's
        // whole-operation deadline independently of fetch's header timeout.
        return new Response(new ReadableStream({ start(controller) { controller.enqueue(new TextEncoder().encode("<!doctype html>")); } }),
          { headers: { "content-type": "text/html" } });
      }
      return original(input, init);
    };
  }, recoveryQuery);
  await page.goto(start);
  await expect(page.getByRole("alert")).toContainText("Allthing couldn’t load");
  await expect.poll(() => page.evaluate(() => (window as Window & { recoveryAborted?: boolean }).recoveryAborted)).toBe(true);
  expect(proof.navigations).toHaveLength(1);
  expect(proof.probes).toHaveLength(0);
  await expect(page.getByRole("button", { name: "Reload", exact: true })).toBeVisible();
});

for (const failure of ["corrupt-wasm", "js-evaluation", "app-runtime", "unauthorized", "forbidden", "limited", "unavailable"] as const) {
  test(`${failure} is an app or service error, never a deployment reload`, async ({ page, request }) => {
    const html = await (await request.get("/")).text();
    const navigations: string[] = [];
    const probes: string[] = [];
    await page.route("**/*", async (route) => {
      const resource = route.request();
      const url = new URL(resource.url());
      if (resource.isNavigationRequest()) {
        navigations.push(url.href);
        await route.fulfill({ contentType: "text/html", body: html });
      } else if (url.pathname === "/" && url.searchParams.has(recoveryQuery)) {
        probes.push(url.href);
        // There really is a different, matching deployment here. The failure
        // must be classified before its existence can authorize any reload.
        await route.fulfill({ contentType: "text/html", body: html.replace(/allthing-[0-9a-f]{10}/g, `allthing-${stale}`) });
      } else if (/\/(?:wasm|assets)\/allthing-[0-9a-f]{10}\.wasm$/.test(url.pathname)) {
        const statuses = { unauthorized: 401, forbidden: 403, limited: 429, unavailable: 503 };
        const status = failure in statuses ? statuses[failure as keyof typeof statuses] : 200;
        await route.fulfill({ status, contentType: "application/wasm", body: failure === "corrupt-wasm" || status !== 200 ? Buffer.from("invalid module") : minimalWasm });
      } else if (/\/assets\/allthing-[0-9a-f]{10}\/allthing\.js$/.test(url.pathname)) {
        await route.fulfill({ contentType: "text/javascript", body: failure === "js-evaluation"
          ? 'throw new Error("module evaluation failed"); export default function () {}'
          : 'export default function () { throw new Error("app startup failed"); }' });
      } else if (url.pathname.startsWith("/api/v1/")) {
        await route.fulfill({ json: { pool: [], meta: { backend: "hypersnap", fetched_at: Date.now() } } });
      } else await route.continue();
    });
    await page.goto(start);
    await expect(page.getByRole("alert")).toContainText("Allthing couldn’t load");
    await page.waitForTimeout(2200);
    expect(navigations).toHaveLength(1);
    expect(probes).toHaveLength(0);
    expect(await page.evaluate((key) => sessionStorage.getItem(key), recoveryKey)).toBeNull();
    await expect(page.getByRole("button", { name: "Reload", exact: true })).toBeVisible();
  });
}
