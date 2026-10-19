// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";

/*
 * Sign In With Farcaster by QR code. The relay and the Worker's nonce and
 * verify endpoints are stand-ins (no Farcaster app can approve a sign-in
 * here); the QR code is the Worker's own, drawn for the relay's link.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no Worker to sign in with");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const author = { fid: 42, username: "carol", display_name: "Carol" };
const cast = { hash: hex(1), author, text: "Hello from the fixture", ts: 1792235681, likes: 0, replies: 0 };
const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64");
const RELAY = "https://relay.farcaster.xyz/v1";
const TOKEN = "210f1718-427e-46a4-99e3-2207f21f83ec";
const LINK = `https://farcaster.xyz/~/siwf?channelToken=${TOKEN}`;
const alice = { fid: 1234, username: "alice", display_name: "Alice", pfp_url: "https://imagedelivery.net/fixture/alice/original" };
const SIGNED = { message: "allthing.social wants you to sign in…", signature: `0x${"ab".repeat(65)}` };
const USER_COOKIE = "__Host-allthing-user";

type Calls = { start: number; channel: any[]; polls: number; verify: any[]; signout: number; origins: string[] };

/**
 * The page's stand-ins. `relay` decides each status poll's answer;
 * `verify` the Worker's answer to the signed message.
 */
async function serve(page: Page, opts: { relay?: (poll: number) => [number, object]; verify?: [number, object] } = {}) {
  const calls: Calls = { start: 0, channel: [], polls: 0, verify: [], signout: 0, origins: [] };
  const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "authorization, content-type" };
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [cast], notes: {}, meta } });
  });
  await page.route("**/api/v1/auth/start", (route) => {
    calls.start += 1;
    calls.origins.push(route.request().headers()["origin"] ?? "");
    const origin = new URL(page.url()).origin;
    return route.fulfill({ json: { nonce: `nonce${calls.start}abcdef012345`, domain: new URL(origin).host, uri: origin } });
  });
  // The Worker draws the code itself.
  await page.route("**/api/v1/auth/qr", (route) => route.continue());
  await page.route("**/api/v1/auth/verify", (route) => {
    calls.verify.push(route.request().postDataJSON());
    const [status, json] = opts.verify ?? [200, { user: alice }];
    const cookie = encodeURIComponent(JSON.stringify(alice));
    const headers: Record<string, string> =
      status === 200 ? { "set-cookie": `${USER_COOKIE}=${cookie}; Path=/; Max-Age=600; Secure; SameSite=Lax` } : {};
    return route.fulfill({ status, json, headers });
  });
  await page.route("**/api/v1/auth/signout", (route) => {
    calls.signout += 1;
    return route.fulfill({ json: {}, headers: { "set-cookie": `${USER_COOKIE}=; Path=/; Max-Age=0; Secure; SameSite=Lax` } });
  });
  await page.route(`${RELAY}/channel`, (route) => {
    if (route.request().method() === "OPTIONS") return route.fulfill({ status: 204, headers: cors });
    calls.channel.push(route.request().postDataJSON());
    return route.fulfill({ status: 201, headers: cors, json: { channelToken: TOKEN, url: LINK, nonce: calls.channel.at(-1).nonce } });
  });
  await page.route(`${RELAY}/channel/status`, (route) => {
    if (route.request().method() === "OPTIONS") return route.fulfill({ status: 204, headers: cors });
    expect(route.request().headers()["authorization"]).toBe(`Bearer ${TOKEN}`);
    calls.polls += 1;
    const [status, json] = opts.relay?.(calls.polls) ?? [202, { state: "pending" }];
    return route.fulfill({ status, headers: cors, json });
  });
  await page.route("https://imagedelivery.net/**", (route) => route.fulfill({ contentType: "image/png", body: png }));
  return calls;
}

/** The relay: pending for two polls, then the app's approval. */
const approvedOnThirdPoll = (poll: number): [number, object] =>
  poll < 3 ? [202, { state: "pending" }] : [200, { state: "completed", ...SIGNED, fid: alice.fid }];

async function phone(browser: Browser, testInfo: TestInfo, width = 390, height = 844) {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL, viewport: { width, height }, isMobile: true, hasTouch: true,
  });
  return { context, page: await context.newPage() };
}

test("signs in by QR code with the Farcaster app, stays signed in, and signs out", async ({ page }) => {
  const calls = await serve(page, { relay: approvedOnThirdPoll });
  await page.goto("/");
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  const signIn = page.getByRole("button", { name: "Sign in with Farcaster" });
  await expect(signIn).toHaveText("Sign in");
  await signIn.click();

  const dialog = page.getByRole("dialog", { name: "Sign in with Farcaster" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Close" })).toBeFocused();
  // The code: the Worker's drawing of the relay's link, large and square.
  const code = dialog.getByRole("img", { name: "Sign-in QR code" });
  await expect(code).toBeVisible();
  const box = (await code.boundingBox())!;
  expect(box.width).toBeGreaterThanOrEqual(240);
  expect(Math.abs(box.width - box.height)).toBeLessThan(1);
  expect(await code.locator("path").getAttribute("d")).toMatch(/^M4 4h7v1h-7z/);
  // The code, and nothing else to choose: no link to the Farcaster app.
  await expect(dialog.getByRole("link")).toHaveCount(0);
  await expect(dialog.getByRole("status")).toHaveText("Waiting for you to approve in Farcaster…");
  // The channel carries the Worker's nonce, this site and its address.
  const origin = new URL(page.url()).origin;
  expect(calls.channel).toEqual([
    { siweUri: origin, domain: new URL(origin).host, nonce: "nonce1abcdef012345", acceptAuthAddress: true },
  ]);
  expect(calls.origins).toEqual([origin]);

  // The app approves: the Worker checks the signed message, and the header
  // shows who signed in.
  await expect(dialog).toBeHidden();
  expect(calls.polls).toBe(3);
  expect(calls.verify).toEqual([SIGNED]);
  const account = page.getByRole("link", { name: "Signed in as @alice: your profile" });
  await expect(account).toBeVisible();
  await expect(account).toBeFocused();
  await expect(account).toHaveAttribute("href", "/u/alice");
  await expect(account.locator("img.avatar")).toHaveAttribute("src", /alice/);
  await expect(signIn).toHaveCount(0);

  // The session outlives the page.
  await page.reload();
  await expect(account).toBeVisible();

  // A click on the picture opens their own profile.
  await page.mouse.move(5, 300);
  await account.click();
  await expect(page).toHaveURL(/\/u\/alice$/);
  await page.goBack();
  await expect(account).toBeVisible();

  // Hovering it shows the account menu: who it is, their profile, and signing out.
  await page.mouse.move(5, 300);
  await account.hover();
  const menu = page.getByRole("dialog", { name: "Your account" });
  await expect(menu).toContainText("Alice");
  await expect(menu).toContainText("@alice");
  await expect(menu.getByRole("link", { name: "Your profile" })).toHaveAttribute("href", "/u/alice");
  await menu.getByRole("button", { name: "Sign out" }).click();
  await expect(signIn).toBeVisible();
  await expect(signIn).toBeFocused();
  await expect.poll(() => calls.signout).toBe(1);
  await page.reload();
  await expect(signIn).toBeVisible();
  await expect(account).toHaveCount(0);
});

test("on a phone, the code fits the screen, with no link to the Farcaster app", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  const calls = await serve(page, { relay: approvedOnThirdPoll });
  await page.goto("/");
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  // A compact button in the header, next to the search box.
  const signIn = page.getByRole("button", { name: "Sign in with Farcaster" });
  await expect(signIn).toBeInViewport({ ratio: 1 });
  await expect(page.locator(".search-box")).toBeInViewport({ ratio: 1 });
  expect((await signIn.boundingBox())!.width).toBeLessThanOrEqual(40);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await signIn.tap();
  const dialog = page.getByRole("dialog", { name: "Sign in with Farcaster" });
  const code = dialog.getByRole("img", { name: "Sign-in QR code" });
  await expect(code).toBeVisible();
  await expect(dialog).toBeInViewport({ ratio: 1 });
  expect((await code.boundingBox())!.width).toBeGreaterThanOrEqual(220);
  await expect(dialog.getByRole("link")).toHaveCount(0);
  await expect(dialog).toBeHidden();
  expect(calls.verify).toHaveLength(1);
  const account = page.getByRole("link", { name: "Signed in as @alice: your profile" });
  await expect(account).toBeInViewport({ ratio: 1 });
  // On a touch screen, a tap on the picture opens the menu, which has the
  // profile and Sign out, rather than following the link.
  await account.tap();
  const menu = page.getByRole("dialog", { name: "Your account" });
  await expect(menu).toBeInViewport({ ratio: 1 });
  await expect(page).toHaveURL(/\/$/);
  await expect(menu.getByRole("link", { name: "Your profile" })).toHaveAttribute("href", "/u/alice");
  await menu.getByRole("button", { name: "Sign out" }).tap();
  await expect(signIn).toBeVisible();
  await context.close();
});

test("at 393 px, a tap on the sign-in icon opens the dialog", async ({ browser }, testInfo) => {
  // 393 × 852: an iPhone 15 or 16.
  const { context, page } = await phone(browser, testInfo, 393, 852);
  await serve(page);
  await page.goto("/");
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  const signIn = page.getByRole("button", { name: "Sign in with Farcaster" });
  await expect(signIn).toBeInViewport({ ratio: 1 });
  // Nothing covers it where a finger lands.
  const box = (await signIn.boundingBox())!;
  const hit = await page.evaluate(([x, y]) => document.elementFromPoint(x, y)?.closest(".sign-in-button") !== null, [
    box.x + box.width / 2,
    box.y + box.height / 2,
  ]);
  expect(hit).toBe(true);
  await signIn.tap();
  const dialog = page.getByRole("dialog", { name: "Sign in with Farcaster" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("img", { name: "Sign-in QR code" })).toBeVisible();
  await expect(dialog).toBeInViewport({ ratio: 1 });
  await context.close();
});

test("an expired code offers a new one, and a refused signature says why", async ({ page }) => {
  const calls = await serve(page, {
    relay: (poll) => (poll === 1 ? [401, { error: "Unauthorized" }] : approvedOnThirdPoll(poll)),
    verify: [401, { error: "That address can't sign in as this Farcaster account" }],
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Sign in with Farcaster" }).click();
  const dialog = page.getByRole("dialog", { name: "Sign in with Farcaster" });
  await expect(dialog.getByRole("status")).toHaveText("This code has expired. Get a new one to sign in.");
  await expect(dialog.getByRole("img", { name: "Sign-in QR code" })).toHaveCount(0);
  await dialog.getByRole("button", { name: "Get a new code" }).click();
  // A new nonce and channel; this time the app approves, but the Worker
  // refuses the signature.
  await expect(dialog.getByRole("status")).toHaveText("That address can't sign in as this Farcaster account");
  expect(calls.start).toBe(2);
  expect(calls.channel.map((c) => c.nonce)).toEqual(["nonce1abcdef012345", "nonce2abcdef012345"]);
  await expect(page.getByRole("link", { name: /Signed in as/ })).toHaveCount(0);
  // Escape closes the dialog and stops polling.
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  const polls = calls.polls;
  await page.waitForTimeout(2000);
  expect(calls.polls).toBe(polls);
});

test("a server without SESSION_SECRET says sign-in isn't set up", async ({ page }) => {
  await serve(page);
  await page.route("**/api/v1/auth/start", (route) =>
    route.fulfill({ status: 503, json: { error: "Sign-in isn't set up on this server yet" } }),
  );
  await page.goto("/");
  await page.getByRole("button", { name: "Sign in with Farcaster" }).click();
  const dialog = page.getByRole("dialog", { name: "Sign in with Farcaster" });
  await expect(dialog.getByRole("status")).toHaveText("Sign-in isn't set up on this server yet");
  await dialog.getByRole("button", { name: "Close" }).click();
  await expect(dialog).toBeHidden();
});

test("the sign-in endpoints take only same-origin POSTs and draw only Farcaster links", async ({ request, baseURL }) => {
  const origin = new URL(baseURL!).origin;
  const post = (path: string, data: object, headers: Record<string, string> = { origin }) =>
    request.post(path, { data, headers });
  expect((await request.get("/api/v1/auth/start")).status()).toBe(405);
  expect((await post("/api/v1/auth/qr", { url: LINK }, { origin: "https://evil.example" })).status()).toBe(403);
  expect((await post("/api/v1/auth/qr", { url: "https://evil.example/~/siwf?channelToken=abcdefgh" })).status()).toBe(400);
  const drawn = await post("/api/v1/auth/qr", { url: LINK });
  expect(drawn.status()).toBe(200);
  expect(drawn.headers()["cache-control"]).toBe("no-store");
  expect((await drawn.json()).size).toBe(45);
  // Unsigned: no sign-in to finish.
  expect((await post("/api/v1/auth/verify", SIGNED)).status()).toBeGreaterThanOrEqual(400);
  // Started: a nonce, kept in an HttpOnly cookie (when SESSION_SECRET is set).
  const started = await post("/api/v1/auth/start", {});
  if (started.status() === 200) {
    expect((await started.json()).nonce).toMatch(/^[0-9a-f]{32}$/);
    expect(started.headers()["set-cookie"]).toMatch(/^__Host-allthing-siwf=[^;]+; Path=\/; Max-Age=600; Secure; SameSite=Strict; HttpOnly/);
  } else {
    expect(started.status()).toBe(503);
  }
  const out = await post("/api/v1/auth/signout", {});
  expect(out.status()).toBe(200);
  expect(out.headersArray().filter((h) => h.name.toLowerCase() === "set-cookie").map((h) => h.value.split("=")[0])).toEqual([
    "__Host-allthing-session",
    USER_COOKIE,
  ]);
});
