// SPDX-License-Identifier: AGPL-3.0-or-later
import axe from "axe-core";
import { createPublicKey, verify } from "node:crypto";
import { expect, test, type Browser, type Locator, type Page, type Route, type TestInfo } from "@playwright/test";
import { appCame } from "./helpers/app";
import { expectDraft } from "./helpers/composer";
import { approvedKey, cast as castOf, envelope, kept, link, reaction, signIn, type Envelope } from "./helpers/farcaster";

/*
 * One QR flow. Where the Worker endorses keys, signing in is approving
 * Allthing's key in the Farcaster app, once: a code on a computer, a link
 * on a phone. That one approval signs the reader in and lets them cast,
 * reply, quote, like, recast, follow and delete from here, with no second
 * dialog. A reader signed in without a key in this browser (signed in
 * before keys, after Revoke permissions, or with its storage cleared) meets
 * the same dialog, in its approve form, on their first write, which then
 * goes. Sign In With Farcaster's relay is only the fallback where the Worker
 * endorses no keys. WebCrypto and IndexedDB are real; the Worker, the
 * Farcaster app's API, the relay and the node are stand-ins.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = Math.floor(Date.now() / 1000) - 600;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const first = { hash: hex(1), author: alice, text: "Like this one", ts, likes: 3, recasts: 0, replies: 0 };
const FARCASTER = "https://api.farcaster.xyz/v2";
const NODE = "https://haatz.quilibrium.com/v1";
const RELAY = "https://relay.farcaster.xyz/v1";
const TOKEN = "0xfc0185a4609110983be9ace986f913c28e50092821297f9d";
const LINK = `farcaster://signed-key-request?token=${TOKEN}`;
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "authorization, content-type" };
const USER_COOKIE = "__Host-allthing-user";

type Calls = { endorse: string[]; signIns: number; relay: number; sent: Envelope[]; approvedAt: number };

/**
 * The page's stand-ins. The Worker endorses keys (`keys`) or doesn't; the
 * Farcaster app approves a request on its second poll and has added the key
 * by its third; the node lists it the second time it's asked.
 */
async function serve(page: Page, { keys = true }: { keys?: boolean } = {}) {
  const calls: Calls = { endorse: [], signIns: 0, relay: 0, sent: [], approvedAt: 0 };
  let key = "";
  let polls = 0;
  let signers = 0;
  let nonce = "";
  const preflight = (route: Route) => route.request().method() === "OPTIONS" && route.fulfill({ status: 204, headers: cors });
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [first], notes: {}, meta } });
  });
  await page.route("**/api/v1/user?*", (route) =>
    route.fulfill({ json: { user: { ...alice, bio: "", follower_count: 120, following_count: 3 }, pool: [first], notes: {}, meta } }),
  );
  await page.route("**/api/v1/auth/start", (route) => {
    nonce = `${Date.now()}`.padStart(32, "a");
    const origin = new URL(page.url()).origin;
    return route.fulfill({ json: { nonce, domain: new URL(origin).host, uri: origin, expiration_time: new Date(Date.now() + 600_000).toISOString(), keys } });
  });
  await page.route("**/api/v1/auth/signer", (route) => {
    key = route.request().postDataJSON().key;
    calls.endorse.push(key);
    return route.fulfill({ json: { key, requestFid: 3356318, signature: `0x${"5a".repeat(65)}`, deadline: Math.floor(Date.now() / 1000) + 3600 } });
  });
  await page.route("**/api/v1/auth/qr", (route) => route.continue());
  // The Worker's check of a sign-in by key: the key's signature of the
  // site and the latest nonce.
  await page.route("**/api/v1/auth/key", (route) => {
    const body = route.request().postDataJSON();
    const message = `${new URL(page.url()).host} asks you to sign in with your Allthing key.\nNonce: ${nonce}`;
    const der = createPublicKey({ key: Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), Buffer.from(body.key.slice(2), "hex")]), format: "der", type: "spki" });
    expect(verify(null, Buffer.from(message), der, Buffer.from(body.signature.slice(2), "hex"))).toBe(true);
    calls.signIns += 1;
    const cookie = `${USER_COOKIE}=${encodeURIComponent(JSON.stringify(me))}; Path=/; Max-Age=600; Secure; SameSite=Lax`;
    return route.fulfill({ json: { user: me }, headers: { "set-cookie": cookie } });
  });
  await page.route("**/api/v1/signer", (route) => (keys ? route.fulfill({ json: { app_fid: 3356318 } }) : route.fulfill({ status: 404, json: { error: "No keys here" } })));
  await page.route("**/api/v1/refresh", (route) => route.fulfill({ json: { expired: 1 } }));
  await page.route(`${FARCASTER}/signed-key-requests`, async (route) => {
    if (await preflight(route)) return;
    polls = 0;
    signers = 0;
    return route.fulfill({ headers: cors, json: { result: { signedKeyRequest: { token: TOKEN, deeplinkUrl: LINK, key, requestFid: 3356318, state: "pending" } } } });
  });
  await page.route(`${FARCASTER}/signed-key-request?*`, (route) => {
    polls += 1;
    if (polls === 2) calls.approvedAt = Date.now();
    const state = polls < 2 ? { state: "pending" } : polls < 3 ? { state: "approved", userFid: me.fid } : { state: "completed", userFid: me.fid };
    return route.fulfill({ headers: cors, json: { result: { signedKeyRequest: { token: TOKEN, deeplinkUrl: LINK, key, requestFid: 3356318, ...state } } } });
  });
  await page.route(`${NODE}/signersByFid?*`, (route) => {
    signers += 1;
    return route.fulfill({ headers: cors, json: { signers: signers < 2 ? [] : [{ key, keyType: 1, fid: me.fid }] } });
  });
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  await page.route(`${NODE}/linkById?*`, (route) =>
    route.fulfill({ status: 400, headers: cors, json: { error: "Failed to get link", error_detail: 'status: NotFound, message: "not found"' } }),
  );
  await page.route(`${NODE}/userNameProofByName?*`, (route) => route.fulfill({ status: 400, headers: cors, json: { error: "Not found" } }));
  await page.route(`${NODE}/submitMessage`, async (route) => {
    if (await preflight(route)) return;
    const e = envelope(route.request().postDataBuffer()!);
    expect(e.signer).toBe(key);
    calls.sent.push(e);
    return route.fulfill({ headers: cors, json: {} });
  });
  // Sign In With Farcaster's relay: counted, and only answered where the
  // Worker endorses no keys.
  await page.route(`${RELAY}/channel`, (route) => {
    if (route.request().method() === "OPTIONS") return route.fulfill({ status: 204, headers: cors });
    calls.relay += 1;
    const token = "210f1718-427e-46a4-99e3-2207f21f83ec";
    return route.fulfill({ status: 201, headers: cors, json: { channelToken: token, url: `https://farcaster.xyz/~/siwf?channelToken=${token}`, nonce: route.request().postDataJSON().nonce } });
  });
  await page.route(`${RELAY}/channel/status`, (route) => {
    if (route.request().method() === "OPTIONS") return route.fulfill({ status: 204, headers: cors });
    calls.relay += 1;
    return route.fulfill({ status: 202, headers: cors, json: { state: "pending" } });
  });
  return calls;
}

/** The page at a width: 1280 with a mouse, or a 390 px phone, by touch. */
type At = { page: Page; width: 1280 | 390; press: (control: Locator) => Promise<void>; close: () => Promise<void> };

async function at(width: 1280 | 390, desktop: Page, browser: Browser, testInfo: TestInfo): Promise<At> {
  if (width === 1280) return { page: desktop, width, press: (control) => control.click(), close: async () => {} };
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  return { page: await context.newPage(), width, press: (control) => control.tap(), close: () => context.close() };
}

/** Accessibility problems axe finds in `scope`, once its animations are done. */
async function axeViolations(page: Page, scope: string) {
  await page.evaluate(() => Promise.all(document.getAnimations().filter((a) => a.effect?.getComputedTiming().endTime !== Infinity).map((a) => a.finished.catch(() => undefined))));
  await page.evaluate(axe.source);
  return page.evaluate(async (scope) => {
    // @ts-expect-error axe is the injected script's global
    const result = await window.axe.run(scope, { resultTypes: ["violations"] });
    return result.violations.map((v: { id: string; nodes: { target: string[] }[] }) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`);
  }, scope);
}

/** What the dialog asks of the reader, at each step: a phone can't scan its own screen. */
const intro = {
  1280: "Scan the code with your phone's camera, then approve Allthing in the Farcaster app.",
  390: "Approve Allthing in the Farcaster app, then come back here.",
} as const;

/** What a phone shows in a code's place: the link to approve in the Farcaster app. */
const approval = (dialog: Locator, width: 1280 | 390, name: string) =>
  width === 1280 ? dialog.getByRole("img", { name }) : dialog.getByRole("link", { name: "Approve in Farcaster" });

for (const width of [1280, 390] as const) {
  test(`at ${width}, one approval signs in, and the reader likes, recasts, casts and follows with no other dialog`, async ({ page: desktop, browser }, testInfo) => {
    const it = await at(width, desktop, browser, testInfo);
    const { page } = it;
    const calls = await serve(page);
    await page.goto("/");
    await appCame(page);
    // Every note the dialog shows, from its first moment.
    await page.evaluate(() => {
      const seen: string[] = [];
      Object.assign(window, { notesSeen: seen });
      new MutationObserver(() => {
        const note = document.querySelector(".signin-note")?.textContent;
        if (note !== undefined && note !== null && seen.at(-1) !== note) seen.push(note);
      }).observe(document.body, { subtree: true, childList: true, characterData: true });
    });
    await it.press(page.getByRole("button", { name: "Sign in with Farcaster" }));
    const dialog = page.getByRole("dialog", { name: "Sign in with Farcaster" });
    await expect(approval(dialog, width, "Sign-in QR code")).toBeVisible();
    if (width === 390) await expect(approval(dialog, width, "")).toHaveAttribute("href", LINK);
    // Said before the scan: one approval, for signing in and every write,
    // asked for by Allthing's own account.
    await expect(dialog).toContainText("One approval signs you in and lets you cast, reply, quote, like, recast, follow and delete from here");
    await expect(dialog).toContainText("The Farcaster app shows the request as from Allthing, @allthingapp.");
    // Never, even while the code was coming, Sign In With Farcaster's note.
    const notes = await page.evaluate(() => (window as unknown as { notesSeen: string[] }).notesSeen);
    expect(notes.filter((note) => note.includes("nothing more"))).toEqual([]);
    expect(await axeViolations(page, ".signin-modal")).toEqual([]);
    await page.screenshot({ path: testInfo.outputPath(`one-qr-${width}-sign-in.png`) });
    await expect(dialog.getByRole("status")).toHaveText("Approved. Signing you in…", { timeout: 15_000 });
    await expect(dialog.locator(".signin-intro")).toHaveText(intro[width]);
    await page.screenshot({ path: testInfo.outputPath(`one-qr-${width}-signing-in.png`) });
    await expect(dialog).toBeHidden({ timeout: 20_000 });
    await expect(page.getByRole("link", { name: "Signed in as @reader: your profile" })).toBeVisible();
    const took = Date.now() - calls.approvedAt;
    console.log(`one-qr ${width}: approval to signed in, ${took} ms`);
    testInfo.annotations.push({ type: "approval to signed in (ms)", description: String(took) });
    expect(calls.relay).toBe(0);
    expect(calls.endorse).toHaveLength(1);

    // Every write goes at once with that key: no other dialog, no other code.
    const card = page.locator(`.cast-card[data-hash="${first.hash}"]`);
    await it.press(card.getByRole("button", { name: "Like", exact: true }));
    await expect.poll(() => calls.sent.length).toBe(1);
    await it.press(card.getByRole("button", { name: "Recast", exact: true }));
    await expect.poll(() => calls.sent.length).toBe(2);
    expect(calls.sent.map((e) => [e.type, reaction(e).reaction])).toEqual([
      [3, 1],
      [3, 2],
    ]);
    await it.press(page.getByRole("button", { name: "Write a cast" }).first());
    const composer = page.getByRole("dialog", { name: "New cast" });
    await composer.getByRole("textbox").fill("gm from one approval");
    await it.press(composer.getByRole("button", { name: "Cast", exact: true }));
    await expect.poll(() => calls.sent.length).toBe(3);
    expect(castOf(calls.sent[2]).text).toBe("gm from one approval");
    await page.goto(`/u/${alice.username}`);
    await it.press(page.getByRole("button", { name: `Follow @${alice.username}` }));
    await expect.poll(() => calls.sent.length).toBe(4);
    expect([calls.sent[3].type, link(calls.sent[3]).target]).toEqual([5, alice.fid]);
    await expect(page.getByRole("dialog", { name: /Approve Allthing/ })).toHaveCount(0);
    expect(calls.endorse).toHaveLength(1);
    expect(calls.relay).toBe(0);
    await it.close();
  });

  test(`at ${width}, a reader signed in with no key here: nothing asks until they write, then the same dialog approves, and the write goes`, async ({ page: desktop, browser }, testInfo) => {
    const it = await at(width, desktop, browser, testInfo);
    const { page } = it;
    const calls = await serve(page);
    await signIn(page, me);
    await page.goto("/");
    await appCame(page);
    await page.waitForTimeout(1000);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await it.press(page.locator(`.cast-card[data-hash="${first.hash}"]`).getByRole("button", { name: "Like", exact: true }));
    const dialog = page.getByRole("dialog", { name: "Approve Allthing to write from here" });
    await expect(approval(dialog, width, "QR code to approve Allthing")).toBeVisible();
    // The sign-in dialog itself, in its approve form.
    await expect(dialog).toHaveClass(/\bsignin-modal\b/);
    await expect(dialog).toContainText("One approval lets you cast, reply, quote, like, recast, follow and delete from here");
    await expect(dialog).toContainText("The Farcaster app shows the request as from Allthing, @allthingapp.");
    expect(await axeViolations(page, ".signin-modal")).toEqual([]);
    await page.screenshot({ path: testInfo.outputPath(`one-qr-${width}-approve.png`) });
    await expect(dialog.getByRole("status")).toHaveText("Approved. Adding Allthing's key to your account…", { timeout: 15_000 });
    await expect(dialog.locator(".signin-intro")).toHaveText(intro[width]);
    await page.screenshot({ path: testInfo.outputPath(`one-qr-${width}-adding.png`) });
    await expect(dialog).toBeHidden({ timeout: 20_000 });
    // The like that asked goes, signed with the key just approved.
    await expect.poll(() => calls.sent.length).toBe(1);
    expect(reaction(calls.sent[0])).toEqual({ reaction: 1, target: { fid: alice.fid, hash: first.hash } });
    expect(await kept(page, me.fid)).toMatchObject({ approved: true });
    expect(calls.signIns).toBe(0);
    expect(calls.relay).toBe(0);
    await it.close();
  });

  test(`at ${width}, a first cast with no key here: the approval opens over the composer, and once approved the cast goes`, async ({ page: desktop, browser }, testInfo) => {
    const it = await at(width, desktop, browser, testInfo);
    const { page } = it;
    const calls = await serve(page);
    await signIn(page, me);
    await page.goto("/");
    await appCame(page);
    await it.press(page.getByRole("button", { name: "Write a cast" }).first());
    const composer = page.getByRole("dialog", { name: "New cast" });
    await composer.getByRole("textbox").fill("gm, my first from here");
    await it.press(composer.getByRole("button", { name: "Cast", exact: true }));
    const dialog = page.getByRole("dialog", { name: "Approve Allthing to write from here" });
    const code = approval(dialog, width, "QR code to approve Allthing");
    await expect(code).toBeVisible();
    // In front of the composer: what is at the code's place is the approval's.
    const box = (await code.boundingBox())!;
    const inFront = await page.evaluate(([x, y]) => Boolean(document.elementFromPoint(x, y)?.closest(".signin-modal")), [box.x + box.width / 2, box.y + box.height / 2]);
    expect(inFront).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`one-qr-${width}-first-cast.png`) });
    await expect(dialog).toBeHidden({ timeout: 20_000 });
    await expect.poll(() => calls.sent.length).toBe(1);
    expect(castOf(calls.sent[0]).text).toBe("gm, my first from here");
    await expect(composer).toBeHidden();
    await it.close();
  });
}

test("after Revoke permissions, a write opens the same dialog, which approves Allthing again", async ({ page }, testInfo) => {
  const calls = await serve(page);
  await signIn(page, me);
  await page.goto("/");
  await approvedKey(page, me.fid, 3_600_000);
  const keyRead = page.waitForRequest((request) => request.url().includes("/reactionsByFid?"));
  await page.reload();
  await appCame(page);
  await keyRead;
  await page.mouse.move(5, 300);
  await page.getByRole("link", { name: "Signed in as @reader: your profile" }).hover();
  const menu = page.getByRole("dialog", { name: "Your account" });
  await menu.getByRole("button", { name: "Revoke permissions" }).click();
  await menu.getByRole("button", { name: "Click again to revoke" }).click();
  await expect.poll(() => kept(page, me.fid)).toBeNull();
  await page.mouse.move(5, 600);
  await page.locator(`.cast-card[data-hash="${first.hash}"]`).getByRole("button", { name: "Like", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Approve Allthing to write from here" });
  await expect(dialog.getByRole("img", { name: "QR code to approve Allthing" })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("one-qr-1280-after-revoke.png") });
  await expect(dialog).toBeHidden({ timeout: 20_000 });
  await expect.poll(() => calls.sent.length).toBe(1);
  expect(await kept(page, me.fid)).toMatchObject({ key: calls.endorse[0], approved: true });
});

test("closed unapproved, the approval drops the write that asked for it", async ({ page }) => {
  const calls = await serve(page);
  await signIn(page, me);
  await page.goto("/");
  await appCame(page);
  await page.locator(`.cast-card[data-hash="${first.hash}"]`).getByRole("button", { name: "Like", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Approve Allthing to write from here" });
  await expect(dialog.getByRole("img", { name: "QR code to approve Allthing" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await page.waitForTimeout(6_000);
  expect(calls.sent).toHaveLength(0);
});

test("closed unapproved over the composer, the cast stays a draft there", async ({ page }) => {
  const calls = await serve(page);
  await signIn(page, me);
  await page.goto("/");
  await appCame(page);
  await page.getByRole("button", { name: "Write a cast" }).first().click();
  const composer = page.getByRole("dialog", { name: "New cast" });
  await composer.getByRole("textbox").fill("not yet");
  await composer.getByRole("button", { name: "Cast", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Approve Allthing to write from here" });
  await expect(dialog.getByRole("img", { name: "QR code to approve Allthing" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  // Only the approval closed: the composer keeps the draft, and its Cast.
  await expect(composer).toBeVisible();
  await expectDraft(composer.getByRole("textbox"), "not yet");
  await expect(composer.getByRole("button", { name: "Cast", exact: true })).toBeFocused();
  await page.waitForTimeout(6_000);
  expect(calls.sent).toHaveLength(0);
});

test("where the Worker endorses no keys, Sign In With Farcaster is the fallback, and writes go to the Farcaster app", async ({ page }, testInfo) => {
  const calls = await serve(page, { keys: false });
  await page.goto("/");
  await appCame(page);
  await page.getByRole("button", { name: "Sign in with Farcaster" }).click();
  const dialog = page.getByRole("dialog", { name: "Sign in with Farcaster" });
  await expect(dialog.getByRole("img", { name: "Sign-in QR code" })).toBeVisible();
  await expect(dialog.locator(".signin-note")).toHaveText("Signing in tells Allthing who you are, nothing more.");
  expect(calls.relay).toBeGreaterThan(0);
  expect(calls.endorse).toHaveLength(0);
  expect(await axeViolations(page, ".signin-modal")).toEqual([]);
  await page.screenshot({ path: testInfo.outputPath("one-qr-1280-fallback.png") });
  // Signed out, a like goes to the Farcaster app, as where keys aren't endorsed.
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await page.locator(`.cast-card[data-hash="${first.hash}"]`).getByRole("button", { name: "Like", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Like in Farcaster" })).toBeVisible();
  expect(calls.endorse).toHaveLength(0);
});
