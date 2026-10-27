// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

// The module comes from the Worker at /wasm/ and, failing that, from
// /assets/; its JavaScript from the build's folder in /assets/.
const paths = { wasm: "**/{assets,wasm}/allthing-*.wasm", js: "**/assets/allthing-*/allthing.js" };
const savedEntryDraft = { drafts: [{ key: "cast:home", draft: { text: "Keep my unsent entry recovery draft" }, updated_at: Math.floor(Date.now() / 1000) }] };
const savedEntryFavorites = [{ fid: 1234, username: "reader" }];

// An entry-script failure happens before boot.js can install its own recovery.
// Keep the real document/CSP and every successful entry/app response intact.
async function entryFailure(page: Page, missing: (attempt: number) => boolean, delayHome = false) {
  const entries: string[] = [];
  const navigations: string[] = [];
  const home: string[] = [];
  const errors: string[] = [];
  let releaseHome = () => {};
  const homeReady = delayHome ? new Promise<void>((resolve) => { releaseHome = resolve; }) : Promise.resolve();
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("request", (request) => {
    if (request.isNavigationRequest() && request.frame() === page.mainFrame()) navigations.push(request.url());
  });
  await page.addInitScript(() => {
    (window as Window & { scriptPolicyViolations?: string[] }).scriptPolicyViolations = [];
    document.addEventListener("securitypolicyviolation", (event) => {
      if (event.effectiveDirective.startsWith("script-src")) {
        (window as Window & { scriptPolicyViolations: string[] }).scriptPolicyViolations.push(event.blockedURI);
      }
    });
  });
  await page.addInitScript(({ draft, favorites }) => {
    localStorage.setItem("allthing-drafts", JSON.stringify(draft));
    localStorage.setItem("allthing.favorites", JSON.stringify(favorites));
    sessionStorage.setItem("allthing-entry-fixture-reader", "keep this reader session");
  }, { draft: savedEntryDraft, favorites: savedEntryFavorites });
  await page.route(/\/assets\/boot-[0-9a-f]+\.js(?:\?.*)?$/, (route) => {
    entries.push(route.request().url());
    return missing(entries.length)
      ? route.fulfill({ status: 404, contentType: "text/plain", body: "entry asset not yet available" })
      : route.continue();
  });
  await page.route("**/api/v1/**", async (route) => {
    if (new URL(route.request().url()).pathname !== "/api/v1/home") {
      return route.fulfill({ status: 404, json: { error: "Fixture endpoint" } });
    }
    home.push(route.request().url());
    await homeReady;
    // A normal 100-cast first page keeps this bootstrap check away from the
    // infinite-scroll boundary. Delay the same drawable page to test paint.js.
    const pool = Array.from({ length: 100 }, (_, i) => ({
      hash: `0x${(i + 1).toString(16).padStart(40, "0")}`,
      author: { fid: 42, username: "late-payload", display_name: "Late payload" },
      text: `Entry recovery fixture ${i + 1}. A delayed Home response must leave Reload usable.`,
      ts: Math.floor(Date.now() / 1000) - i * 60, likes: 100 - i, recasts: 0, replies: 0,
    }));
    return route.fulfill({ json: { pool, notes: {}, meta: { backend: "hypersnap", fetched_at: Date.now() } } });
  });
  return { entries, navigations, home, errors, releaseHome };
}

async function entryLoaded(page: Page) {
  await expect(page.getByRole("combobox", { name: "Search casts, people and channels", exact: true })).toBeVisible();
  await expect(page.locator("#boot")).toHaveCount(0);
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(await page.evaluate(() => (window as Window & { scriptPolicyViolations: string[] }).scriptPolicyViolations)).toEqual([]);
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem("allthing-drafts")!))).toEqual(savedEntryDraft);
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem("allthing.favorites")!))).toEqual(savedEntryFavorites);
  expect(await page.evaluate(() => sessionStorage.getItem("allthing-entry-fixture-reader"))).toBe("keep this reader session");
}

for (const width of [1280, 390]) {
  test(`the first missing boot script recovers before any loader can run at width ${width}`, async ({ page, baseURL }) => {
    await page.setViewportSize({ width, height: 844 });
    const proof = await entryFailure(page, (attempt) => attempt === 1);
    const originalURL = new URL("/?sort=top&t=week#first-entry", baseURL).href;
    await page.goto(originalURL);
    await entryLoaded(page);
    expect(proof.entries).toHaveLength(2);
    expect(new URL(proof.entries[1]).pathname).toBe(new URL(proof.entries[0]).pathname);
    expect(proof.entries[1]).not.toBe(proof.entries[0]); // Bypass the failed module-map entry.
    expect(new URL(proof.entries[1]).searchParams.get("__allthing_entry_retry")).toBe("1");
    expect(proof.navigations).toEqual([originalURL.split("#")[0]]);
    expect(proof.home).toHaveLength(1);
    await expect(page).toHaveURL(originalURL);
    expect(proof.errors).toEqual([]);
  });

  test(`a persistently missing boot script stops retrying and offers manual Reload at width ${width}`, async ({ page, baseURL }) => {
    await page.setViewportSize({ width, height: 844 });
    let blocked = true;
    const proof = await entryFailure(page, () => blocked, true);
    const originalURL = new URL("/?sort=top&t=week#first-entry", baseURL).href;
    await page.goto(originalURL);
    await expect(page.getByRole("alert")).toContainText("Allthing couldn’t load");
    const reload = page.getByRole("button", { name: "Reload", exact: true });
    await expect(reload).toBeVisible();
    const attempts = proof.entries.length;
    expect(attempts).toBe(2);
    const lateHome = page.waitForResponse((response) => new URL(response.url()).pathname === "/api/v1/home");
    proof.releaseHome();
    await (await lateHome).finished();
    await page.waitForTimeout(2200);
    await expect(page.getByRole("alert")).toContainText("Allthing couldn’t load");
    await expect(reload).toBeVisible();
    await expect(page.locator("#boot .cast-card")).toHaveCount(0);
    expect(proof.entries).toHaveLength(attempts);
    expect(proof.navigations).toEqual([originalURL.split("#")[0]]);
    expect(proof.home).toHaveLength(1);
    blocked = false;
    await reload.click();
    await entryLoaded(page);
    expect(proof.entries).toHaveLength(attempts + 1);
    expect(proof.navigations).toEqual([originalURL.split("#")[0], originalURL.split("#")[0]]);
    expect(proof.home).toHaveLength(2);
    await expect(page).toHaveURL(originalURL);
    expect(proof.errors).toEqual([]);
  });

  test(`a healthy boot script adds no entry recovery requests at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    const proof = await entryFailure(page, () => false);
    await page.goto("/?sort=top&t=week");
    await entryLoaded(page);
    expect(proof.entries).toHaveLength(1);
    expect(proof.navigations).toEqual([page.url()]);
    expect(proof.home).toHaveLength(1);
    expect(proof.errors).toEqual([]);
  });
}

for (const width of [1280, 390]) {
  for (const asset of ["wasm", "js"] as const) {
    test(`a failed ${asset} load offers a working reload at width ${width}`, async ({ page }, info) => {
      await page.setViewportSize({ width, height: 844 });
      let blocked = true;
      const errors: string[] = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await page.route(paths[asset], (route) => blocked ? route.abort("failed") : route.continue());
      await page.route("**/api/v1/**", (route) => route.fulfill({ json: {
        pool: [], meta: { backend: "hypersnap", fetched_at: Date.now() },
      } }));
      await page.goto("/?sort=top&t=week");
      const originalURL = page.url();
      // Capture both a failed baseline and the rendered recovery state.
      try {
        await expect(page.getByRole("alert")).toContainText("Allthing couldn’t load", { timeout: 2500 });
      } finally {
        await page.screenshot({ path: info.outputPath(`boot-${asset}-${width}.png`) });
      }
      const reload = page.getByRole("button", { name: "Reload", exact: true });
      await expect(reload).toBeVisible();
      blocked = false;
      await reload.click();
      await expect(page.getByRole("combobox", { name: "Search casts, people and channels", exact: true })).toBeVisible();
      await expect(page).toHaveURL(originalURL);
      await expect(page.getByRole("alert")).toHaveCount(0);
      await expect(page.locator("#boot")).toHaveCount(0);
      expect(errors).toEqual([]);
    });
  }
}

for (const width of [1280, 390]) {
  test(`the app loads the static copy of its module when the Worker can't serve it at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const modules: string[] = [];
    page.on("request", (request) => {
      const path = new URL(request.url()).pathname;
      // The app's own module only (allthing-<hash>.wasm), not the parts it
      // fetches when idle (split_*.wasm, chunk_*.wasm), such as the poll's.
      if (/\/allthing-[0-9a-f]+\.wasm$/.test(path)) modules.push(path.split("/")[1]);
    });
    await page.route("**/wasm/**", (route) => route.fulfill({ status: 429, contentType: "text/plain", body: "Too many requests" }));
    await page.route("**/api/v1/**", (route) => route.fulfill({ json: {
      pool: [], meta: { backend: "hypersnap", fetched_at: Date.now() },
    } }));
    await page.goto("/?sort=top&t=week");
    await expect(page.getByRole("combobox", { name: "Search casts, people and channels", exact: true })).toBeVisible();
    await expect(page.getByRole("alert")).toHaveCount(0);
    expect(modules).toEqual(["wasm", "assets"]);
    expect(errors).toEqual([]);
  });

  test(`the module comes from the Worker at width ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    const modules: { path: string, status: number, type: string | undefined }[] = [];
    page.on("response", (response) => {
      const path = new URL(response.url()).pathname;
      // As in the fallback case, count exactly one main module, including
      // any unexpected /assets fallback. Idle split/shared modules are separate.
      if (/\/(?:assets|wasm)\/allthing-[0-9a-f]+\.wasm$/.test(path)) modules.push({ path, status: response.status(), type: response.headers()["content-type"] });
    });
    await page.route("**/api/v1/**", (route) => route.fulfill({ json: {
      pool: [], meta: { backend: "hypersnap", fetched_at: Date.now() },
    } }));
    // Exercise the ordering where valid idle modules finish before the main
    // module assertions, rather than relying on the header appearing first.
    const idlePoll = page.waitForResponse((response) => /\/split_poll\.wasm$/.test(new URL(response.url()).pathname));
    await page.goto("/");
    await expect(page.getByRole("combobox", { name: "Search casts, people and channels", exact: true })).toBeVisible();
    expect((await idlePoll).status()).toBe(200);
    expect(modules).toHaveLength(1);
    expect(modules[0].path).toMatch(/^\/wasm\/allthing-[0-9a-f]+\.wasm$/);
    expect(modules[0].status).toBe(200);
    expect(modules[0].type).toBe("application/wasm");
  });
}
