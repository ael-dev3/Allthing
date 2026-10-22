// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";
import { appCame } from "./helpers/app";
import { expectDraft } from "./helpers/composer";
import { approvedKey, cast as castOf, kept, reaction, removed, signIn } from "./helpers/farcaster";
import { NODE, standIn, type Answer, type StandIn } from "./helpers/node";
import type { Envelope } from "./helpers/farcaster";

/*
 * Writes from Allthing land on Farcaster, or are taken back. The browser
 * signs each like, recast, quote, and their undoing, with the reader's key
 * (WebCrypto's, real) and hands it to the node. A stand-in node checks each
 * message as Snapchain would (its hash is BLAKE3 of its data, its Ed25519
 * signature is by the key the reader approved, on mainnet, by the reader,
 * dated now) and answers the browser's read-backs from what it merged. What
 * stands on the page is what the node shows: a message it never shows,
 * refuses, or can't be reached for is taken back exactly, with why, and a
 * way to try again. Nothing is posted anywhere.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = Math.floor(Date.now() / 1000) - 600;
const EPOCH = 1_609_459_200;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const post = { hash: hex(1), author: alice, text: "Worth quoting", ts, likes: 3, recasts: 1, replies: 0 };
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" };

/** The page at a width: 1280 with a mouse, or a 390 px phone, by touch. */
type At = { page: Page; press: (control: Locator) => Promise<void>; close: () => Promise<void> };

async function at(width: 1280 | 390, desktop: Page, browser: Browser, testInfo: TestInfo): Promise<At> {
  if (width === 1280) return { page: desktop, press: (control) => control.click(), close: async () => {} };
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  return { page: await context.newPage(), press: (control) => control.tap(), close: () => context.close() };
}

/**
 * The Worker and the node, as stand-ins, and the reader signed in with a
 * key approved an hour ago, on `path`. The node does with each new message
 * what `answer` says.
 */
async function ready(page: Page, path: string, answer?: (n: number, message: Envelope) => Answer): Promise<StandIn> {
  await signIn(page, me);
  let key = "";
  const node = await standIn(page, { fid: me.fid, key: () => key, answer });
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [post], notes: {}, meta } });
  });
  // A cast's page: the post, or a cast of the reader's that the node has.
  await page.route("**/api/v1/cast?*", (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash")!;
    const mine = node.merged.get(hash);
    const cast = mine ? { hash, author: me, text: castOf(mine).text || "My quote", ts: mine.at + EPOCH, likes: 0, recasts: 0, replies: 0 } : post;
    return route.fulfill({ json: { thread: { cast, children: [] }, notes: {}, meta } });
  });
  await page.route("**/api/v1/user?*", (route) =>
    route.fulfill({ json: { user: { ...me, bio: "", follower_count: 5, following_count: 3 }, pool: [], notes: {}, meta } }),
  );
  await page.route("**/api/v1/reactions?*", (route) => {
    const url = new URL(route.request().url());
    return route.fulfill({ json: { hash: url.searchParams.get("hash"), kind: url.searchParams.get("type"), users: [], more: false, meta } });
  });
  await page.route("**/api/v1/replies?*", (route) => route.fulfill({ json: { hash: post.hash, replies: [], more: false, meta } }));
  await page.route("**/api/v1/refresh", (route) => route.fulfill({ json: { expired: 1 } }));
  await page.route("**/api/v1/signer", (route) => route.fulfill({ json: { app_fid: 3356318 } }));
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  await page.route(`${NODE}/signersByFid?*`, (route) => route.fulfill({ headers: cors, json: { signers: [{ key }] } }));
  await page.route(`${NODE}/userNameProofByName?*`, (route) =>
    route.fulfill({ status: 400, headers: cors, json: { error: "Failed to get username proof", error_detail: 'status: NotFound, message: "Username proof not found"' } }),
  );
  await page.goto(path);
  await expect(page.locator(".cast-card:not(.skeleton), .post").first()).toBeVisible();
  key = await approvedKey(page, me.fid, 3_600_000);
  // Ready to write once the page has read the key: it then reads the
  // reader's likes and recasts from the node.
  const keyRead = page.waitForRequest((request) => request.url().includes("/reactionsByFid?"));
  await page.reload();
  await appCame(page);
  await keyRead;
  await expect(page.locator(".cast-card:not(.skeleton), .post").first()).toBeVisible();
  return node;
}

const card = (page: Page) => page.locator(`.cast-card[data-hash="${post.hash}"]`);
const likeOf = (page: Page) => card(page).getByRole("button", { name: "Like", exact: true });
const recastOf = (page: Page) => card(page).getByRole("button", { name: "Recast", exact: true });
const likes = (page: Page) => card(page).locator(".stat-likes .n");
const recasts = (page: Page) => card(page).locator(".stat-recasts .n");
const notice = (page: Page, text: string | RegExp) => page.locator(".toast").filter({ hasText: text });
/** Sent from here and not shown on the node yet: busy, and pulsing. */
const settled = (control: Locator) => expect(control).not.toHaveAttribute("aria-busy");
const reactionRead = (kind: "Like" | "Recast") => `reactionById?fid=${me.fid}&reaction_type=${kind}&target_fid=${alice.fid}&target_hash=${post.hash}`;

for (const width of [1280, 390] as const) {
  test(`at ${width}, a like and a recast stand once the node shows them, and so does undoing each`, async ({ page: desktop, browser }, testInfo) => {
    const { page, press, close } = await at(width, desktop, browser, testInfo);
    const node = await ready(page, "/");
    const [like, recast] = [likeOf(page), recastOf(page)];
    await expect(likes(page)).toHaveText("3");
    // At once: pressed, counted, and busy until the node shows it.
    await press(like);
    await expect(like).toHaveAttribute("aria-pressed", "true");
    await expect(likes(page)).toHaveText("4");
    await settled(like);
    // The node has this very message: read back by the reader, kind and cast.
    expect(node.sent).toHaveLength(1);
    expect([node.sent[0].type, reaction(node.sent[0])]).toEqual([3, { reaction: 1, target: { fid: alice.fid, hash: post.hash } }]);
    expect(node.merged.has(node.sent[0].hash)).toBe(true);
    expect(node.reads).toContain(reactionRead("Like"));
    await expect(like).toHaveAttribute("aria-pressed", "true");
    await expect(likes(page)).toHaveText("4");

    await press(recast);
    await expect(recast).toHaveAttribute("aria-pressed", "true");
    await expect(recasts(page)).toHaveText("2");
    await settled(recast);
    expect(reaction(node.sent[1])).toEqual({ reaction: 2, target: { fid: alice.fid, hash: post.hash } });
    expect(node.reads).toContain(reactionRead("Recast"));

    // Undone: a removal, a second later at least, done once the node no
    // longer has the like.
    await press(like);
    await expect(like).toHaveAttribute("aria-pressed", "false");
    await expect(likes(page)).toHaveText("3");
    await settled(like);
    expect([node.sent[2].type, reaction(node.sent[2]).reaction]).toEqual([4, 1]);
    expect(node.sent[2].at).toBeGreaterThan(node.sent[0].at);
    await press(recast);
    await expect(recast).toHaveAttribute("aria-pressed", "false");
    await settled(recast);
    expect([node.sent[3].type, reaction(node.sent[3]).reaction]).toEqual([4, 2]);
    await expect(recasts(page)).toHaveText("1");
    await expect(likes(page)).toHaveText("3");
    // Each was read back after it went, and nothing was taken back.
    expect(node.reads.filter((read) => read === reactionRead("Like")).length).toBeGreaterThanOrEqual(2);
    await expect(page.locator(".toast.shown")).toHaveCount(0);
    await close();
  });

  test(`at ${width}, a like the node takes but never shows is undone, with why, and Retry likes again`, async ({ page: desktop, browser }, testInfo) => {
    const { page, press, close } = await at(width, desktop, browser, testInfo);
    const node = await ready(page, "/", (n) => (n === 1 ? "never" : "merge"));
    const like = likeOf(page);
    await press(like);
    await expect(like).toHaveAttribute("aria-pressed", "true");
    await expect(likes(page)).toHaveText("4");
    await expect(card(page).locator(".stat-likes")).toHaveClass(/pending/);
    // 15 s of looking for it on the node, then back exactly as it was.
    const undone = notice(page, "Your like didn't show up on Farcaster in time, so it's undone.");
    await expect(undone).toBeVisible({ timeout: 30_000 });
    await expect(like).toHaveAttribute("aria-pressed", "false");
    await expect(likes(page)).toHaveText("3");
    await expect(card(page).locator(".stat-likes")).not.toHaveClass(/pending/);
    expect(node.sent).toHaveLength(1);
    expect(node.reads.filter((read) => read === reactionRead("Like")).length).toBe(6);
    // Retry: a like again, which this time shows.
    await press(undone.getByRole("button", { name: "Retry" }));
    await expect(like).toHaveAttribute("aria-pressed", "true");
    await settled(like);
    expect(node.sent).toHaveLength(2);
    expect(node.sent[1].type).toBe(3);
    expect(node.merged.has(node.sent[1].hash)).toBe(true);
    await expect(likes(page)).toHaveText("4");
    await close();
  });

  test(`at ${width}, a like the node refuses is undone with its reason; a busy node gets the same like again`, async ({ page: desktop, browser }, testInfo) => {
    const { page, press, close } = await at(width, desktop, browser, testInfo);
    // Refused; then busy, down, and taken.
    const answers: Answer[] = [{ refuse: "message has a timestamp too far in the future" }, "unavailable", "down", "merge"];
    const node = await ready(page, "/", (n) => answers[n - 1] ?? "merge");
    const like = likeOf(page);
    await press(like);
    await expect(notice(page, "Farcaster didn't take it: message has a timestamp too far in the future")).toBeVisible();
    await expect(like).toHaveAttribute("aria-pressed", "false");
    await expect(likes(page)).toHaveText("3");
    // Nothing to try again: the same would be refused again.
    await expect(page.locator(".toast").getByRole("button")).toHaveCount(0);
    expect(node.reads).toHaveLength(0);

    // Busy (a 400 whose status says Unavailable, as Hypersnap answers),
    // then down (503): the same message again after 1 and 2 s, then taken.
    await press(like);
    await expect(like).toHaveAttribute("aria-pressed", "true");
    await settled(like);
    expect(node.sent).toHaveLength(4);
    expect(node.answers).toEqual([400, 400, 503, 200]);
    const [, ...again] = node.sent;
    expect(new Set(again.map((sent) => sent.hash)).size).toBe(1);
    expect(node.merged.has(again[0].hash)).toBe(true);
    await expect(likes(page)).toHaveText("4");
    await close();
  });

  test(`at ${width}, a node out of reach through every try: the like is undone, and Retry likes once it's back`, async ({ page: desktop, browser }, testInfo) => {
    const { page, press, close } = await at(width, desktop, browser, testInfo);
    // Down for the like and the three times it goes again (after 1, 2 and 4 s).
    const node = await ready(page, "/", (n) => (n <= 4 ? "down" : "merge"));
    const like = likeOf(page);
    await press(like);
    await expect(like).toHaveAttribute("aria-pressed", "true");
    const down = notice(page, "Couldn't reach Farcaster's network");
    await expect(down).toBeVisible({ timeout: 20_000 });
    await expect(like).toHaveAttribute("aria-pressed", "false");
    await expect(likes(page)).toHaveText("3");
    expect(node.answers).toEqual([503, 503, 503, 503]);
    expect(new Set(node.sent.map((sent) => sent.hash)).size).toBe(1);
    expect(node.reads).toHaveLength(0);
    await press(down.getByRole("button", { name: "Retry" }));
    await expect(like).toHaveAttribute("aria-pressed", "true");
    await settled(like);
    expect(node.answers.at(-1)).toBe(200);
    expect(node.merged.has(node.sent.at(-1)!.hash)).toBe(true);
    await expect(likes(page)).toHaveText("4");
    await close();
  });

  test(`at ${width}, a like whose answer is lost goes again, the node has it already, and it counts once`, async ({ page: desktop, browser }, testInfo) => {
    const { page, press, close } = await at(width, desktop, browser, testInfo);
    const node = await ready(page, "/", (n) => (n === 1 ? "lost" : "merge"));
    const like = likeOf(page);
    await press(like);
    await expect(like).toHaveAttribute("aria-pressed", "true");
    await settled(like);
    // The same message twice: merged the first time, a duplicate the second.
    expect(node.sent).toHaveLength(2);
    expect(node.sent[1].hash).toBe(node.sent[0].hash);
    expect(node.answers).toEqual([0, 400]);
    expect(node.merged.size).toBe(1);
    await expect(like).toHaveAttribute("aria-pressed", "true");
    await expect(likes(page)).toHaveText("4");
    await expect(page.locator(".toast.shown")).toHaveCount(0);
    await close();
  });

  test(`at ${width}, a key the node doesn't know is forgotten, the like undone, and the notice offers to approve again`, async ({ page: desktop, browser }, testInfo) => {
    const { page, press, close } = await at(width, desktop, browser, testInfo);
    const node = await ready(page, "/", () => "unknown-key");
    const like = likeOf(page);
    await press(like);
    const gone = notice(page, "Allthing's key isn't on your Farcaster account anymore.");
    await expect(gone).toBeVisible();
    await expect(like).toHaveAttribute("aria-pressed", "false");
    await expect(likes(page)).toHaveText("3");
    await expect.poll(() => kept(page, me.fid)).toBeNull();
    expect(node.sent).toHaveLength(1);
    await press(gone.getByRole("button", { name: "Approve again" }));
    await expect(page.getByRole("dialog", { name: "Approve Allthing" })).toBeVisible();
    await close();
  });

  test(`at ${width}, an unlike the node never shows leaves the like as it was`, async ({ page: desktop, browser }, testInfo) => {
    const { page, press, close } = await at(width, desktop, browser, testInfo);
    const node = await ready(page, "/", (n) => (n === 2 ? "never" : "merge"));
    const like = likeOf(page);
    await press(like);
    await expect(like).toHaveAttribute("aria-pressed", "true");
    await settled(like);
    await expect(likes(page)).toHaveText("4");
    await press(like);
    await expect(like).toHaveAttribute("aria-pressed", "false");
    await expect(likes(page)).toHaveText("3");
    await expect(notice(page, "Your unlike didn't show up on Farcaster in time, so the like is back.")).toBeVisible({ timeout: 30_000 });
    await expect(like).toHaveAttribute("aria-pressed", "true");
    await expect(likes(page)).toHaveText("4");
    expect(node.sent.map((sent) => sent.type)).toEqual([3, 4]);
    await close();
  });

  test(`at ${width}, a quote shows at once as posting, then as posted once the node shows it; deleting it is confirmed too`, async ({ page: desktop, browser }, testInfo) => {
    const { page, press, close } = await at(width, desktop, browser, testInfo);
    // The node takes the quote at once, and has it 3 s later.
    const node = await ready(page, `/p/${post.hash}`, (n, sent) => (sent.type === 1 ? { mergeAfter: 3000 } : "merge"));
    await press(page.getByRole("button", { name: "Quote @alice's cast" }));
    const dialog = page.getByRole("dialog", { name: "Quote" });
    await dialog.getByRole("textbox").fill("Well said");
    await press(dialog.getByRole("button", { name: "Quote", exact: true }));
    await expect(dialog).toBeHidden();
    expect(node.sent).toHaveLength(1);
    const quote = node.sent[0];
    expect(castOf(quote)).toEqual({ text: "Well said", mentions: [], positions: [], embeds: [{ cast: { fid: alice.fid, hash: post.hash } }], parent: null, type: 0 });
    // Above the reader's feed at once, as posting; posted once the node has it.
    await press(page.getByRole("link", { name: "Allthing, home", exact: true }));
    const mine = page.locator(".cast-card.outgoing");
    await expect(mine).toContainText("Well said");
    await expect(mine.locator(".outgoing-state")).toHaveText("Posting…");
    await expect(mine.getByRole("link", { name: "Posted" })).toHaveAttribute("href", `/p/${quote.hash}`, { timeout: 15_000 });
    expect(node.reads).toContain(`castById?fid=${me.fid}&hash=${quote.hash}`);
    await expect(notice(page, "Posted.")).toBeVisible();

    // Deleting it: a CastRemove of its hash, shown once the node no longer has it.
    await press(mine.getByRole("link", { name: "Posted" }));
    const shown = page.locator(".post");
    await expect(shown).toContainText("Well said");
    await press(shown.getByRole("button", { name: "Delete" }));
    await press(shown.getByRole("group", { name: "Delete this cast?" }).getByRole("button", { name: "Delete" }));
    await expect(shown.locator(".deleted-note")).toBeVisible();
    expect([node.sent[1].type, removed(node.sent[1])]).toEqual([2, quote.hash]);
    expect(node.reads.filter((read) => read === `castById?fid=${me.fid}&hash=${quote.hash}`).length).toBeGreaterThanOrEqual(2);
    await expect(notice(page, "Deleted.")).toBeVisible();
    await close();
  });

  test(`at ${width}, a quote the node never shows leaves the reader's lists, its draft comes back, and Retry sends the same message`, async ({ page: desktop, browser }, testInfo) => {
    const { page, press, close } = await at(width, desktop, browser, testInfo);
    const node = await ready(page, `/p/${post.hash}`, (n) => (n === 1 ? "never" : "merge"));
    await press(page.getByRole("button", { name: "Quote @alice's cast" }));
    const dialog = page.getByRole("dialog", { name: "Quote" });
    await dialog.getByRole("textbox").fill("Agreed");
    await press(dialog.getByRole("button", { name: "Quote", exact: true }));
    await expect(dialog).toBeHidden();
    const quote = node.sent[0];
    // On the reader's own profile at once, as posting (on a phone, a tap on
    // the picture opens the account menu, which has the profile).
    await press(page.getByRole("link", { name: `Signed in as @${me.username}: your profile` }));
    if (width === 390) await press(page.getByRole("dialog", { name: "Your account" }).getByRole("link", { name: "Your profile" }));
    await expect(page.locator(".profile")).toBeVisible();
    const mine = page.locator(".cast-card.outgoing");
    await expect(mine).toContainText("Agreed");
    await expect(mine.locator(".outgoing-state")).toHaveText("Posting…");
    // Never shown: taken back, with why, and a way to send it again.
    const back = notice(page, "Your quote didn't show up on Farcaster in time, so it's taken back.");
    await expect(back).toBeVisible({ timeout: 30_000 });
    await expect(mine).toHaveCount(0);
    expect(node.merged.size).toBe(0);
    // The same message again, which shows this time.
    await press(back.getByRole("button", { name: "Retry" }));
    await expect(mine).toContainText("Agreed");
    await expect(mine.getByRole("link", { name: "Posted" })).toHaveAttribute("href", `/p/${quote.hash}`, { timeout: 15_000 });
    expect(node.sent).toHaveLength(2);
    expect(node.sent[1].hash).toBe(quote.hash);
    expect(node.merged.has(quote.hash)).toBe(true);
    await close();
  });
}

test("a quote taken back puts its draft back in the composer, for the same cast", async ({ page }) => {
  const node = await ready(page, `/p/${post.hash}`, () => "never");
  await page.getByRole("button", { name: "Quote @alice's cast" }).click();
  const dialog = page.getByRole("dialog", { name: "Quote" });
  await dialog.getByRole("textbox").fill("Agreed");
  await dialog.getByRole("button", { name: "Quote", exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(notice(page, "Your quote didn't show up on Farcaster in time, so it's taken back.")).toBeVisible({ timeout: 30_000 });
  expect(node.sent).toHaveLength(1);
  await page.getByRole("button", { name: "Quote @alice's cast" }).click();
  await expectDraft(dialog.getByRole("textbox"), "Agreed");
});

test("deleting a cast the node still shows leaves it, says why, and Retry deletes it", async ({ page }) => {
  // The quote merges; its delete is taken but never shows, then goes.
  const node = await ready(page, `/p/${post.hash}`, (n) => (n === 2 ? "never" : "merge"));
  await page.getByRole("button", { name: "Quote @alice's cast" }).click();
  const dialog = page.getByRole("dialog", { name: "Quote" });
  await dialog.getByRole("textbox").fill("To delete");
  await dialog.getByRole("button", { name: "Quote", exact: true }).click();
  await expect(notice(page, "Posted.")).toBeVisible();
  const quote = node.sent[0];
  await page.goto(`/p/${quote.hash}`);
  const shown = page.locator(".post");
  await expect(shown).toContainText("To delete");
  await shown.getByRole("button", { name: "Delete" }).click();
  await shown.getByRole("group", { name: "Delete this cast?" }).getByRole("button", { name: "Delete" }).click();
  await expect(shown.getByRole("button", { name: "Deleting…" })).toBeDisabled();
  const still = notice(page, "Deleting it didn't show up on Farcaster in time: the cast is still there.");
  await expect(still).toBeVisible({ timeout: 30_000 });
  await expect(shown.locator(".deleted-note")).toBeHidden();
  await expect(shown).toContainText("To delete");
  // Retry deletes it again, with no second ask.
  await still.getByRole("button", { name: "Retry" }).click();
  await expect(shown.locator(".deleted-note")).toBeVisible();
  expect(node.sent.map((sent) => sent.type)).toEqual([1, 2, 2]);
  expect(removed(node.sent[2])).toBe(quote.hash);
});
