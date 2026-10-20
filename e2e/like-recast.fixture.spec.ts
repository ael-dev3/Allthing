// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type Route, type TestInfo } from "@playwright/test";
import { approvedKey as seedKey, envelope, kept as keptFor, reaction, signIn as signInAs } from "./helpers/farcaster";

/*
 * Liking and recasting from Allthing with a key the reader approves by QR
 * code. The browser's WebCrypto and IndexedDB are real; the Worker's
 * endorsement, the Farcaster app's API and the node are stand-ins (no
 * Farcaster app can approve a key here, and nothing is posted). Every
 * message handed to the node is decoded and its signature checked against
 * the key the browser had endorsed.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const bob = { fid: 7, username: "bob", display_name: "Bob" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const first = { hash: hex(1), author: alice, text: "Like this one", ts, likes: 3, recasts: 0, replies: 0 };
const second = { hash: hex(2), author: bob, text: "Already liked", ts: ts - 60, likes: 1, recasts: 1, replies: 0 };
const FARCASTER = "https://api.farcaster.xyz/v2";
const NODE = "https://haatz.quilibrium.com/v1";
const TOKEN = "0xfc0185a4609110983be9ace986f913c28e50092821297f9d";
const LINK = `farcaster://signed-key-request?token=${TOKEN}`;
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "authorization, content-type" };
const RELAY = "https://relay.farcaster.xyz/v1";
const CHANNEL = "210f1718-427e-46a4-99e3-2207f21f83ec";

type Sent = { type: number; fid: number; at: number; network: number; reaction: number; target: { fid: number; hash: string }; signer: string };
type Calls = { writes: number; endorse: string[]; requests: object[]; polls: number; signers: number; sent: Sent[]; reactions: string[]; signIns: number };
type Options = {
  writes?: boolean;
  /** The Farcaster app's answer to each poll of the request. */
  poll?: (poll: number) => object;
  /** The node's answer to each message: [status, body]. */
  submit?: (call: number) => [number, object];
  /** What the node says the reader liked. */
  liked?: string[];
};

/** A like or recast the node was given: its signature checked, its data read. */
function decode(body: Buffer): Sent {
  const e = envelope(body);
  return { type: e.type, fid: e.fid, at: e.at, network: e.network, ...reaction(e), signer: e.signer };
}

/** A node message saying the reader reacted to `hash`. */
const reacted = (hash: string, kind: "LIKE" | "RECAST") => ({
  data: { type: "MESSAGE_TYPE_REACTION_ADD", fid: me.fid, reactionBody: { type: `REACTION_TYPE_${kind}`, targetCastId: { fid: 42, hash } } },
});

async function serve(page: Page, options: Options = {}) {
  const { writes = true, poll = (n: number) => (n < 3 ? { state: "pending" } : n < 4 ? { state: "approved", userFid: me.fid } : { state: "completed", userFid: me.fid }) } = options;
  const calls: Calls = { writes: 0, endorse: [], requests: [], polls: 0, signers: 0, sent: [], reactions: [], signIns: 0 };
  let key = "";
  const preflight = (route: Route) => route.request().method() === "OPTIONS" && route.fulfill({ status: 204, headers: cors });
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  // Who liked or recasted: two people, and more for "See everyone".
  await page.route("**/api/v1/reactions?*", (route) => {
    const url = new URL(route.request().url());
    const all = url.searchParams.get("limit") !== "10";
    return route.fulfill({ json: { hash: url.searchParams.get("hash"), kind: url.searchParams.get("type"), users: [alice, bob], more: !all, meta } });
  });
  // Signing in, by the relay and the Worker: the Farcaster app approves on
  // the relay's second poll, and the Worker's check signs the reader in.
  await page.route("**/api/v1/auth/start", (route) => {
    const origin = new URL(page.url()).origin;
    return route.fulfill({ json: { nonce: "nonce1abcdef012345", domain: new URL(origin).host, uri: origin, expiration_time: new Date(Date.now() + 600_000).toISOString() } });
  });
  await page.route("**/api/v1/auth/verify", (route) => {
    calls.signIns += 1;
    const cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify(me))}; Path=/; Max-Age=600; Secure; SameSite=Lax`;
    return route.fulfill({ json: { user: me }, headers: { "set-cookie": cookie } });
  });
  let relayPolls = 0;
  await page.route(`${RELAY}/channel`, (route) => {
    if (route.request().method() === "OPTIONS") return route.fulfill({ status: 204, headers: cors });
    relayPolls = 0;
    return route.fulfill({ status: 201, headers: cors, json: { channelToken: CHANNEL, url: `https://farcaster.xyz/~/siwf?channelToken=${CHANNEL}`, nonce: route.request().postDataJSON().nonce } });
  });
  await page.route(`${RELAY}/channel/status`, (route) => {
    if (route.request().method() === "OPTIONS") return route.fulfill({ status: 204, headers: cors });
    relayPolls += 1;
    return relayPolls < 2
      ? route.fulfill({ status: 202, headers: cors, json: { state: "pending" } })
      : route.fulfill({ headers: cors, json: { state: "completed", message: "allthing.social wants you to sign in…", signature: `0x${"ab".repeat(65)}`, fid: me.fid, username: me.username } });
  });
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [first, second], notes: {}, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: { thread: { cast: first, children: [] }, notes: {}, meta } }));
  await page.route("**/api/v1/signer", (route) => {
    calls.writes += 1;
    return writes ? route.fulfill({ json: { app_fid: 539854 } }) : route.fulfill({ status: 404, json: { error: "Liking and recasting from Allthing isn't set up on this server" } });
  });
  await page.route("**/api/v1/auth/signer", (route) => {
    key = route.request().postDataJSON().key;
    calls.endorse.push(key);
    return route.fulfill({ json: { key, requestFid: 539854, signature: `0x${"5a".repeat(65)}`, deadline: Math.floor(Date.now() / 1000) + 3600 } });
  });
  // The Worker draws the code itself.
  await page.route("**/api/v1/auth/qr", (route) => route.continue());
  await page.route(`${FARCASTER}/signed-key-requests`, async (route) => {
    if (await preflight(route)) return;
    calls.requests.push(route.request().postDataJSON());
    return route.fulfill({ headers: cors, json: { result: { signedKeyRequest: { token: TOKEN, deeplinkUrl: LINK, key, requestFid: 539854, state: "pending" } } } });
  });
  await page.route(`${FARCASTER}/signed-key-request?*`, (route) => {
    expect(new URL(route.request().url()).searchParams.get("token")).toBe(TOKEN);
    calls.polls += 1;
    return route.fulfill({ headers: cors, json: { result: { signedKeyRequest: { token: TOKEN, deeplinkUrl: LINK, key, requestFid: 539854, ...poll(calls.polls) } } } });
  });
  await page.route(`${NODE}/signersByFid?*`, (route) => {
    calls.signers += 1;
    return route.fulfill({ headers: cors, json: { signers: calls.signers < 2 ? [] : [{ key, keyType: 1, fid: me.fid }] } });
  });
  await page.route(`${NODE}/reactionsByFid?*`, (route) => {
    const kind = new URL(route.request().url()).searchParams.get("reaction_type")!;
    calls.reactions.push(kind);
    const messages = kind === "Like" ? (options.liked ?? [second.hash]).map((hash) => reacted(hash, "LIKE")) : [];
    return route.fulfill({ headers: cors, json: { messages } });
  });
  await page.route(`${NODE}/reactionById?*`, (route) =>
    route.fulfill({ status: 400, headers: cors, json: { error: "Failed to get reaction", error_detail: 'status: NotFound, message: "not found"' } }),
  );
  await page.route(`${NODE}/submitMessage`, async (route) => {
    if (await preflight(route)) return;
    expect(route.request().headers()["content-type"]).toBe("application/octet-stream");
    calls.sent.push(decode(route.request().postDataBuffer()!));
    const [status, json] = options.submit?.(calls.sent.length) ?? [200, {}];
    return route.fulfill({ status, headers: cors, json });
  });
  return calls;
}

const signIn = (page: Page) => signInAs(page, me);
const kept = (page: Page) => keptFor(page, me.fid);
const approvedKey = (page: Page, ago: number) => seedKey(page, me.fid, ago);

const card = (page: Page, cast: { hash: string }) => page.locator(`.cast-card[data-hash="${cast.hash}"]`);

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  return { context, page: await context.newPage() };
}

test("signed out, the icons open the cast in the Farcaster app and never the list; the preview is as it was", async ({ page }) => {
  const calls = await serve(page);
  await page.goto("/");
  const like = card(page, first).getByRole("button", { name: "Like", exact: true });
  // Not a toggle here: a way to the Farcaster app.
  await expect(like).not.toHaveAttribute("aria-pressed");
  await expect(like).toHaveAttribute("aria-haspopup", "dialog");
  await expect(card(page, first).getByRole("button", { name: "Recast", exact: true })).toBeVisible();
  // Pointing at the heart previews who liked, as pointing at the count does.
  await like.hover();
  const preview = card(page, first).getByRole("dialog", { name: "Liked by" });
  await expect(preview).toContainText("@alice");
  // A click on it hands the like to the Farcaster app; the list doesn't open,
  // and no one is asked to sign in.
  await like.click();
  const handOff = page.getByRole("dialog", { name: "Like in Farcaster" });
  await expect(handOff.getByRole("link", { name: /Open on farcaster\.xyz/ })).toHaveAttribute("href", `https://farcaster.xyz/alice/${first.hash.slice(0, 10)}`);
  await expect(page.getByRole("dialog", { name: "Sign in with Farcaster" })).toHaveCount(0);
  await expect(page.locator(".who-modal")).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(handOff).toHaveCount(0);
  await expect(like).toBeFocused();
  // Signing in from the header, where the Worker endorses keys, makes the
  // heart a toggle, and asks for no key until it's pressed.
  await page.getByRole("button", { name: "Sign in with Farcaster" }).click();
  await expect(page.getByRole("link", { name: "Signed in as @reader: your profile" })).toBeVisible();
  await expect.poll(() => calls.writes).toBe(1);
  await expect(like).toHaveAttribute("aria-pressed", "false");
  await expect(page.getByRole("dialog", { name: "Approve Allthing" })).toHaveCount(0);
  expect(calls.endorse).toHaveLength(0);
  // The full list: the preview's "See everyone", or a click on the count.
  await like.hover();
  await preview.getByRole("button", { name: "See everyone →" }).click();
  const list = page.locator(".who-modal");
  await expect(list).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(list).toHaveCount(0);
  await card(page, first).getByRole("button", { name: "3 likes: show who" }).click();
  await expect(list).toBeVisible();
  expect(calls.sent).toHaveLength(0);
});

test("signed in from the header, a like approves a key, then likes", async ({ page }) => {
  const calls = await serve(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Sign in with Farcaster" }).click();
  await expect(page.getByRole("link", { name: "Signed in as @reader: your profile" })).toBeVisible();
  const like = card(page, first).getByRole("button", { name: "Like", exact: true });
  await expect(like).toHaveAttribute("aria-pressed", "false");
  await like.click();
  // On to the key, approved in the Farcaster app.
  const keyDialog = page.getByRole("dialog", { name: "Approve Allthing" });
  await expect(keyDialog.getByRole("img", { name: "QR code to approve Allthing" })).toBeVisible();
  expect(calls.signIns).toBe(1);
  expect(calls.writes).toBe(1);
  // Approved too: the like that started it all goes out.
  await expect(keyDialog).toBeHidden();
  await expect.poll(() => calls.sent.length).toBe(1);
  expect(calls.sent[0]).toMatchObject({ type: 3, reaction: 1, fid: me.fid, target: { fid: 42, hash: first.hash }, signer: calls.endorse[0] });
  await expect(like).toHaveAttribute("aria-pressed", "true");
  await expect(card(page, first).locator(".stat-likes .n")).toHaveText("4");
  await expect(page.locator(".who-modal")).toHaveCount(0);
});

test("on a server that endorses no keys, a signed-in reader likes and recasts in the Farcaster app", async ({ page }) => {
  await signIn(page);
  const calls = await serve(page, { writes: false });
  await page.goto("/");
  await expect(card(page, first)).toBeVisible();
  await expect.poll(() => calls.writes).toBe(1);
  const recast = card(page, first).getByRole("button", { name: "Recast", exact: true });
  // Pressed or not as the reader recasted, in any app: here, not.
  await expect(recast).toHaveAttribute("aria-pressed", "false");
  await recast.click();
  const handOff = page.getByRole("dialog", { name: "Recast in Farcaster" });
  await expect(handOff.locator(".handoff-line")).toHaveCount(0);
  await expect(handOff.getByRole("link", { name: /Open on farcaster\.xyz/ })).toHaveAttribute("href", `https://farcaster.xyz/alice/${first.hash.slice(0, 10)}`);
  // The count is as it was, and no key is asked for.
  await page.keyboard.press("Escape");
  await expect(card(page, first).getByRole("button", { name: "3 likes: show who" })).toBeVisible();
  await expect(page.getByRole("dialog", { name: "Approve Allthing" })).toHaveCount(0);
  expect(calls.endorse).toHaveLength(0);
  expect(calls.sent).toHaveLength(0);
});

test("a like asks for a key by QR code, then likes, unlikes and remembers", async ({ page }) => {
  await signIn(page);
  const calls = await serve(page);
  await page.goto("/");
  const like = card(page, first).getByRole("button", { name: "Like", exact: true });
  await expect(like).toHaveAttribute("aria-pressed", "false");
  await expect(card(page, first).locator(".stat-likes .n")).toHaveText("3");
  // No key yet: what the reader liked isn't read.
  expect(calls.reactions).toEqual([]);

  await like.click();
  const dialog = page.getByRole("dialog", { name: "Approve Allthing" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Close" })).toBeFocused();
  // The Worker's drawing of the request's link; no link to follow instead.
  const code = dialog.getByRole("img", { name: "QR code to approve Allthing" });
  await expect(code).toBeVisible();
  expect(await code.locator("path").getAttribute("d")).toMatch(/^M4 4h7v1h-7z/);
  await expect(dialog.getByRole("link")).toHaveCount(0);
  await expect(dialog).toContainText("Allthing's servers never see it");
  // The Farcaster app will name the account that asks, a person's.
  await expect(dialog).toContainText("The Farcaster app shows the request as from @0xael.eth, who makes Allthing.");
  await page.screenshot({ path: test.info().outputPath("approve-1280.png") });
  // A fresh key, endorsed by the Worker and passed on as it is.
  expect(calls.endorse).toHaveLength(1);
  const key = calls.endorse[0];
  expect(key).toMatch(/^0x[0-9a-f]{64}$/);
  expect(calls.requests).toEqual([expect.objectContaining({ key, requestFid: 539854, signature: `0x${"5a".repeat(65)}` })]);
  const pending = await kept(page);
  expect(pending).toEqual({ key, extractable: false, algorithm: "Ed25519", approved: false, token: TOKEN });

  // Approved, added, and known to the node: the dialog closes, and the like
  // that opened it goes out.
  await expect(dialog.getByRole("status")).toHaveText("Approved. Adding Allthing's key to your account…");
  await expect(dialog).toBeHidden();
  await expect(like).toBeFocused();
  expect(calls.polls).toBe(4);
  await expect.poll(() => calls.sent.length).toBe(1);
  const [liked] = calls.sent;
  expect(liked).toEqual({ type: 3, fid: me.fid, at: expect.any(Number), network: 1, reaction: 1, target: { fid: 42, hash: first.hash }, signer: key });
  expect(liked.at).toBeGreaterThan(Math.floor(Date.now() / 1000) - 1_609_459_200 - 60);
  await expect(like).toHaveAttribute("aria-pressed", "true");
  await expect(card(page, first).locator(".stat-likes .n")).toHaveText("4");
  expect(await kept(page)).toEqual({ key, extractable: false, algorithm: "Ed25519", approved: true, token: null });
  // Now what the reader liked is read: the second cast's heart fills.
  await expect(card(page, second).getByRole("button", { name: "Like", exact: true })).toHaveAttribute("aria-pressed", "true");

  // Unlike: a removal, a second later at least, so it wins.
  await like.click();
  await expect.poll(() => calls.sent.length).toBe(2);
  expect(calls.sent[1]).toMatchObject({ type: 4, reaction: 1, target: { fid: 42, hash: first.hash }, signer: key });
  expect(calls.sent[1].at).toBeGreaterThan(liked.at);
  await expect(like).toHaveAttribute("aria-pressed", "false");
  await expect(card(page, first).locator(".stat-likes .n")).toHaveText("3");

  // Recast with the same key: no dialog.
  await card(page, first).getByRole("button", { name: "Recast", exact: true }).click();
  await expect.poll(() => calls.sent.length).toBe(3);
  expect(calls.sent[2]).toMatchObject({ type: 3, reaction: 2, target: { hash: first.hash } });
  await expect(card(page, first).locator(".stat-recasts .n")).toHaveText("1");
  await expect(dialog).toBeHidden();

  // The key outlives the page; the account menu can turn it off.
  await page.reload();
  await expect(card(page, second).getByRole("button", { name: "Like", exact: true })).toHaveAttribute("aria-pressed", "true");
  expect(calls.endorse).toHaveLength(1);
  await page.mouse.move(5, 300);
  await page.getByRole("link", { name: "Signed in as @reader: your profile" }).hover();
  const menu = page.getByRole("dialog", { name: "Your account" });
  await menu.getByRole("button", { name: "Stop liking from this browser" }).click();
  await expect(page.getByRole("status").filter({ hasText: "This browser's key is deleted" })).toBeVisible();
  expect(await kept(page)).toBeNull();
});

test("on a phone, the buttons are easy to tap and the dialog fits", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  await signIn(page);
  const calls = await serve(page);
  await page.goto("/");
  const recast = card(page, first).getByRole("button", { name: "Recast", exact: true });
  await expect(recast).toBeVisible();
  for (const button of [recast, card(page, first).getByRole("button", { name: "Like", exact: true })]) {
    const box = (await button.boundingBox())!;
    expect(box.width).toBeGreaterThanOrEqual(24);
    expect(box.height).toBeGreaterThanOrEqual(24);
  }
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await recast.tap();
  const dialog = page.getByRole("dialog", { name: "Approve Allthing" });
  // On the phone itself, no code to scan: the link opens the Farcaster app.
  const open = dialog.getByRole("link", { name: "Approve in Farcaster" });
  await expect(open).toHaveAttribute("href", /^farcaster:\/\/signed-key-request\?token=/);
  await expect(dialog.getByRole("img", { name: "QR code to approve Allthing" })).toHaveCount(0);
  await expect(dialog).toBeInViewport({ ratio: 1 });
  await page.screenshot({ path: testInfo.outputPath("approve-390.png") });
  await expect(dialog).toBeHidden();
  await expect.poll(() => calls.sent.length).toBe(1);
  expect(calls.sent[0]).toMatchObject({ type: 3, reaction: 2, fid: me.fid, target: { fid: 42, hash: first.hash } });
  await expect(recast).toHaveAttribute("aria-pressed", "true");
  await expect(card(page, first).locator(".stat-recasts .n")).toHaveText("1");
  await context.close();
});

test("on a phone, signed out, a tap on the heart opens the Farcaster app's way; one on the count previews who", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  const calls = await serve(page);
  await page.goto("/");
  // The count is a target of its own beside the heart, as tall as the heart's.
  const count = card(page, first).getByRole("button", { name: "3 likes: show who" });
  const box = (await count.boundingBox())!;
  expect(box.height).toBeGreaterThanOrEqual(24);
  await count.tap();
  const preview = card(page, first).getByRole("dialog", { name: "Liked by" });
  await expect(preview).toContainText("@alice");
  await expect(page.locator(".who-modal")).toHaveCount(0);
  await preview.getByRole("button", { name: "See everyone →" }).tap();
  const list = page.locator(".who-modal");
  await expect(list).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(list).toHaveCount(0);
  await card(page, first).getByRole("button", { name: "Like", exact: true }).tap();
  // No code on a phone: its link opens the app.
  const handOff = page.getByRole("dialog", { name: "Like in Farcaster" });
  await expect(handOff).toBeInViewport({ ratio: 1 });
  await expect(handOff.getByRole("link", { name: /Open in Farcaster/ })).toHaveAttribute("href", `https://farcaster.xyz/alice/${first.hash.slice(0, 10)}`);
  await expect(handOff.getByRole("img")).toHaveCount(0);
  await expect(list).toHaveCount(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  expect(calls.sent).toHaveLength(0);
  await context.close();
});

test("a key removed in the Farcaster app is forgotten, and a failed like is undone", async ({ page }) => {
  await signIn(page);
  const calls = await serve(page, { submit: (n) => (n === 1 ? [503, { error: "unavailable" }] : [400, { error: "Failed to submit message", error_detail: 'status: InvalidArgument, message: "bad_request.validation_failure/invalid signer"' }]) });
  await page.goto("/");
  await expect(card(page, first)).toBeVisible();
  const key = await approvedKey(page, 3_600_000);
  await page.reload();
  const like = card(page, first).getByRole("button", { name: "Like", exact: true });
  // The node is down: the like is undone, and the reader told.
  await like.click();
  await expect(page.getByRole("status").filter({ hasText: "Couldn't reach Farcaster's network" })).toBeVisible();
  await expect(like).toHaveAttribute("aria-pressed", "false");
  await expect(card(page, first).locator(".stat-likes .n")).toHaveText("3");
  expect(calls.sent[0]).toMatchObject({ type: 3, signer: key });
  // The node doesn't know the key any more: it was removed.
  await like.click();
  await expect(page.getByRole("status").filter({ hasText: "Allthing's key isn't on your Farcaster account anymore" })).toBeVisible();
  await expect(like).toHaveAttribute("aria-pressed", "false");
  await expect.poll(() => kept(page)).toBeNull();
  expect(calls.sent).toHaveLength(2);
  // So the next like asks for a new one.
  await like.click();
  await expect(page.getByRole("dialog", { name: "Approve Allthing" })).toBeVisible();
});

test("an approval from another account is refused, and signing out forgets the key", async ({ page }) => {
  await signIn(page);
  const calls = await serve(page, { poll: (n) => (n < 2 ? { state: "pending" } : { state: "approved", userFid: 999 }) });
  await page.goto("/");
  await card(page, first).getByRole("button", { name: "Like", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Approve Allthing" });
  await expect(dialog.getByRole("status")).toHaveText(
    "That was approved from another Farcaster account (fid 999). Approve from the account you're signed in with.",
  );
  await expect(dialog.getByRole("button", { name: "Get a new code" })).toBeVisible();
  expect(calls.sent).toHaveLength(0);
  expect(await kept(page)).toBeNull();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();

  // An approved key goes with the session.
  await approvedKey(page, 60_000);
  await page.reload();
  await expect(card(page, first).getByRole("button", { name: "Like", exact: true })).toBeVisible();
  await page.mouse.move(5, 300);
  await page.getByRole("link", { name: "Signed in as @reader: your profile" }).hover();
  await page.getByRole("dialog", { name: "Your account" }).getByRole("button", { name: "Sign out" }).click();
  await expect(page.getByRole("button", { name: "Sign in with Farcaster" })).toBeVisible();
  await expect.poll(() => kept(page)).toBeNull();
  // Signed out, the hearts open the casts in the Farcaster app.
  await expect(card(page, second).getByRole("button", { name: "Like", exact: true })).not.toHaveAttribute("aria-pressed");
});

test("the real Worker endorses keys only for a session, from Allthing, and draws only Farcaster's links", async ({ request, baseURL }) => {
  const writes = await request.get("/api/v1/signer");
  expect(writes.status()).toBe(401);
  expect(writes.headers()["cache-control"]).toBe("no-store");
  const origin = new URL(baseURL!).origin;
  const key = `0x${"11".repeat(32)}`;
  const foreign = await request.post("/api/v1/auth/signer", { data: { key }, headers: { origin: "https://evil.example" } });
  expect(foreign.status()).toBe(403);
  const forged = await request.post("/api/v1/auth/signer", {
    data: { key },
    headers: { origin, cookie: "__Host-allthing-session=1234.9999999999.00" },
  });
  expect(forged.status()).toBe(401);
  const code = await request.post("/api/v1/auth/qr", { data: { url: LINK }, headers: { origin } });
  expect(code.status()).toBe(200);
  expect((await code.json()).path).toMatch(/^M4 4h7v1h-7z/);
  // The Farcaster app's own links, which writes are handed to: its composer,
  // a cast and a profile.
  for (const url of ["https://farcaster.xyz/~/compose?text=gm&parentCastHash=0x01", `https://farcaster.xyz/alice/${first.hash.slice(0, 10)}`, "https://farcaster.xyz/alice"]) {
    const drawn = await request.post("/api/v1/auth/qr", { data: { url }, headers: { origin } });
    expect(drawn.status(), url).toBe(200);
  }
  for (const url of [
    `${LINK}&next=https://evil.example`,
    "farcaster://signed-key-request?token=0x12",
    "https://evil.example/?token=0xfc01",
    "https://farcaster.xyz/~/settings",
    "https://farcaster.xyz/alice?next=https://evil.example",
    "https://farcaster.xyz.evil.example/alice",
  ]) {
    const refused = await request.post("/api/v1/auth/qr", { data: { url }, headers: { origin } });
    expect(refused.status(), url).toBe(400);
  }
  const status = await (await request.get("/api/v1/status")).json();
  expect(status.writes).toEqual({ ready: false });
});
