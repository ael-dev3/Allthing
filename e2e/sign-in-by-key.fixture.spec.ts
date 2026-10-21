// SPDX-License-Identifier: AGPL-3.0-or-later
import { createPublicKey, verify } from "node:crypto";
import { expect, test, type Browser, type Page, type Route, type TestInfo } from "@playwright/test";
import { envelope, kept, reaction } from "./helpers/farcaster";

/*
 * Signing in in one step. Where the Worker endorses keys, the sign-in
 * dialog asks the reader to approve Allthing's key in the Farcaster app,
 * and that one approval signs them in and lets them like, recast and post
 * from Allthing: no Sign In With Farcaster first, no second approval. The
 * browser's WebCrypto and IndexedDB are real; the Worker's endpoints, the
 * Farcaster app's API and the node are stand-ins, and the Worker's check of
 * the key's signature is made here as the Worker makes it
 * (e2e/key-sign-in.fixture.spec.ts runs the real one).
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const first = { hash: hex(1), author: alice, text: "Like this one", ts, likes: 3, recasts: 0, replies: 0 };
const FARCASTER = "https://api.farcaster.xyz/v2";
const NODE = "https://haatz.quilibrium.com/v1";
const RELAY = "https://relay.farcaster.xyz/v1";
const TOKEN = "0xfc0185a4609110983be9ace986f913c28e50092821297f9d";
const LINK = `farcaster://signed-key-request?token=${TOKEN}`;
const KEY_UNKNOWN = "That key isn't one this account approved for Allthing";
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "authorization, content-type" };
const USER_COOKIE = "__Host-allthing-user";

type Calls = {
  starts: string[];
  endorse: string[];
  polls: number;
  signers: number;
  signIns: { fid: number; key: string; signature: string; good: boolean }[];
  sent: ReturnType<typeof reaction>[];
  signer: string[];
  relay: number;
  writes: number;
};

/** The page's stand-ins. `refuse` answers the Worker's first `n` checks of the key as a node a moment behind would. */
async function serve(page: Page, opts: { behind?: number } = {}) {
  const calls: Calls = { starts: [], endorse: [], polls: 0, signers: 0, signIns: [], sent: [], signer: [], relay: 0, writes: 0 };
  let key = "";
  const preflight = (route: Route) => route.request().method() === "OPTIONS" && route.fulfill({ status: 204, headers: cors });
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [first], notes: {}, meta } });
  });
  // The Worker endorses keys, so `start` says so.
  await page.route("**/api/v1/auth/start", (route) => {
    const nonce = `${calls.starts.length + 1}`.padStart(32, "a");
    calls.starts.push(nonce);
    const origin = new URL(page.url()).origin;
    return route.fulfill({ json: { nonce, domain: new URL(origin).host, uri: origin, expiration_time: new Date(Date.now() + 600_000).toISOString(), keys: true } });
  });
  await page.route("**/api/v1/auth/signer", (route) => {
    key = route.request().postDataJSON().key;
    calls.endorse.push(key);
    return route.fulfill({ json: { key, requestFid: 3356318, signature: `0x${"5a".repeat(65)}`, deadline: Math.floor(Date.now() / 1000) + 3600 } });
  });
  // The Worker draws the code itself.
  await page.route("**/api/v1/auth/qr", (route) => route.continue());
  // The Worker's check: the key's Ed25519 signature of this site and the
  // latest nonce, then whether the node lists the key.
  await page.route("**/api/v1/auth/key", (route) => {
    const body = route.request().postDataJSON();
    const domain = new URL(page.url()).host;
    const message = `${domain} asks you to sign in with your Allthing key.\nNonce: ${calls.starts.at(-1)}`;
    const raw = Buffer.from(body.key.slice(2), "hex");
    const der = createPublicKey({ key: Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), raw]), format: "der", type: "spki" });
    const good = verify(null, Buffer.from(message), der, Buffer.from(body.signature.slice(2), "hex"));
    calls.signIns.push({ ...body, good });
    if (!good) return route.fulfill({ status: 401, json: { error: "The signature doesn't match this sign-in" } });
    if (calls.signIns.length <= (opts.behind ?? 0)) return route.fulfill({ status: 401, json: { error: KEY_UNKNOWN } });
    const cookie = `${USER_COOKIE}=${encodeURIComponent(JSON.stringify(me))}; Path=/; Max-Age=600; Secure; SameSite=Lax`;
    return route.fulfill({ json: { user: me }, headers: { "set-cookie": cookie } });
  });
  await page.route("**/api/v1/signer", (route) => {
    calls.writes += 1;
    return route.fulfill({ json: { app_fid: 3356318 } });
  });
  await page.route(`${FARCASTER}/signed-key-requests`, async (route) => {
    if (await preflight(route)) return;
    return route.fulfill({ headers: cors, json: { result: { signedKeyRequest: { token: TOKEN, deeplinkUrl: LINK, key, requestFid: 3356318, state: "pending" } } } });
  });
  // Pending, then approved, then added to the account.
  await page.route(`${FARCASTER}/signed-key-request?*`, (route) => {
    calls.polls += 1;
    const state = calls.polls < 2 ? { state: "pending" } : calls.polls < 3 ? { state: "approved", userFid: me.fid } : { state: "completed", userFid: me.fid };
    return route.fulfill({ headers: cors, json: { result: { signedKeyRequest: { token: TOKEN, deeplinkUrl: LINK, key, requestFid: 3356318, ...state } } } });
  });
  await page.route(`${NODE}/signersByFid?*`, (route) => {
    calls.signers += 1;
    return route.fulfill({ headers: cors, json: { signers: calls.signers < 2 ? [] : [{ key, keyType: 1, fid: me.fid }] } });
  });
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  await page.route(`${NODE}/submitMessage`, async (route) => {
    if (await preflight(route)) return;
    const e = envelope(route.request().postDataBuffer()!);
    calls.sent.push(reaction(e));
    calls.signer.push(e.signer);
    return route.fulfill({ headers: cors, json: {} });
  });
  // One step: Sign In With Farcaster's relay is never asked.
  await page.route(`${RELAY}/**`, (route) => {
    calls.relay += 1;
    return route.fulfill({ status: 500, headers: cors, json: {} });
  });
  return calls;
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  return { context, page: await context.newPage() };
}

/** Wait for the dialog's look to settle (its fade in), for a screenshot. */
const settled = (page: Page) =>
  page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== "running" || a.effect?.getComputedTiming().endTime === Infinity));

test("at 1280, one approval of Allthing's key signs in, and the reader likes at once with it", async ({ page }, testInfo) => {
  const calls = await serve(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Sign in with Farcaster" }).click();
  const dialog = page.getByRole("dialog", { name: "Sign in with Farcaster" });
  // A code for the reader's phone, which asks for Allthing's key.
  await expect(dialog.getByRole("img", { name: "Sign-in QR code" })).toBeVisible();
  await expect(dialog).toContainText("Scan the code with your phone's camera, then approve Allthing in the Farcaster app.");
  await expect(dialog).toContainText("One approval signs you in and lets you post, like and recast from here");
  await expect(dialog.getByRole("link", { name: "Approve in Farcaster" })).toHaveCount(0);
  expect(calls.endorse).toHaveLength(1);
  await settled(page);
  await page.screenshot({ path: testInfo.outputPath("one-step-1280.png") });
  // Approved in the app: signed in, the dialog gone, the account in the header.
  await expect(dialog).toBeHidden({ timeout: 20_000 });
  await expect(page.getByRole("link", { name: "Signed in as @reader: your profile" })).toBeVisible();
  // The Worker was shown the key it endorsed, signing this site's latest nonce.
  expect(calls.signIns).toEqual([{ fid: me.fid, key: calls.endorse[0], signature: expect.stringMatching(/^0x[0-9a-f]{128}$/), good: true }]);
  expect(calls.relay).toBe(0);
  // The key is the account's now, approved, and unexportable; none waits as a sign-in's.
  expect(await kept(page, me.fid)).toMatchObject({ key: calls.endorse[0], approved: true, extractable: false, algorithm: "Ed25519" });
  expect(await kept(page, 0)).toBeNull();
  // A like goes out at once, signed with that key: no second approval.
  await page.getByRole("button", { name: "Like", exact: true }).click();
  await expect.poll(() => calls.sent.length).toBe(1);
  expect(calls.sent[0]).toEqual({ reaction: 1, target: { fid: alice.fid, hash: first.hash } });
  expect(calls.signer).toEqual([calls.endorse[0]]);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(calls.endorse).toHaveLength(1);
});

test("at 390, the dialog links to the Farcaster app instead of a code, and the approval signs in", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  const calls = await serve(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Sign in with Farcaster" }).tap();
  const dialog = page.getByRole("dialog", { name: "Sign in with Farcaster" });
  const approve = dialog.getByRole("link", { name: "Approve in Farcaster" });
  await expect(approve).toHaveAttribute("href", LINK);
  await expect(dialog.getByRole("img", { name: "Sign-in QR code" })).toHaveCount(0);
  await expect(dialog).toContainText("Approve Allthing in the Farcaster app, then come back here.");
  // The button and the dialog fit the screen.
  const box = (await dialog.boundingBox())!;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(390);
  const link = (await approve.boundingBox())!;
  expect(link.height).toBeGreaterThanOrEqual(40);
  await settled(page);
  await page.screenshot({ path: testInfo.outputPath("one-step-390.png") });
  await expect(dialog).toBeHidden({ timeout: 20_000 });
  await expect(page.getByRole("link", { name: "Signed in as @reader: your profile" })).toBeVisible();
  expect(calls.signIns.map((s) => s.good)).toEqual([true]);
  expect(await kept(page, me.fid)).toMatchObject({ approved: true });
  await context.close();
});

test("a node a moment behind the Farcaster app is asked again, with no new approval", async ({ page }) => {
  const calls = await serve(page, { behind: 1 });
  await page.goto("/");
  await page.getByRole("button", { name: "Sign in with Farcaster" }).click();
  await expect(page.getByRole("dialog", { name: "Sign in with Farcaster" })).toBeHidden({ timeout: 25_000 });
  await expect(page.getByRole("link", { name: "Signed in as @reader: your profile" })).toBeVisible();
  // The same key, asked twice; one endorsement, one request.
  expect(calls.signIns.map((s) => [s.key, s.good])).toEqual([
    [calls.endorse[0], true],
    [calls.endorse[0], true],
  ]);
  expect(calls.endorse).toHaveLength(1);
});

test("a reader signed in before Allthing could write for them is asked once to approve its key", async ({ page }) => {
  const calls = await serve(page);
  await page.addInitScript((user) => {
    document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax`;
  }, me);
  await page.goto("/");
  // The approval opens by itself, once.
  const approve = page.getByRole("dialog", { name: "Approve Allthing" });
  await expect(approve).toBeVisible();
  await expect(approve.getByRole("img", { name: "QR code to approve Allthing" })).toBeVisible();
  expect(await page.evaluate(() => localStorage.getItem("allthing-asked-to-approve"))).toBe(String(me.fid));
  await page.keyboard.press("Escape");
  await expect(approve).toBeHidden();
  // Not again: the page asks what it can do, and opens nothing.
  const before = calls.writes;
  await page.reload();
  await expect.poll(() => calls.writes).toBeGreaterThan(before);
  await page.waitForTimeout(1000);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  // A like still asks, as before.
  await page.getByRole("button", { name: "Like", exact: true }).click();
  await expect(approve).toBeVisible();
});
