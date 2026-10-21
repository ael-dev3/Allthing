// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";
import { appCame } from "./helpers/app";
import { expectDraft } from "./helpers/composer";
import { approvedKey, cast as castOf, signIn, type Envelope } from "./helpers/farcaster";
import { NODE, standIn, type Answer, type StandIn } from "./helpers/node";

/*
 * A reply goes as it does in the Farcaster app: the composer closes at
 * once, and the reply shows in the thread, at the top of what it answers,
 * as posting, with one more reply counted. Once the node shows it, it's a
 * reply like the others, with its time as its link, and screen readers hear
 * "Reply posted."; once the page lists it itself, it shows once. Refused,
 * or not shown in time, it goes, the count goes back, and the composer
 * opens again with the draft and why, with Retry, which hands the node the
 * same message, where that may help. The browser signs each reply with the
 * reader's real WebCrypto key, and a stand-in node checks it as Snapchain
 * would (`helpers/node.ts`). Nothing is posted anywhere.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = Math.floor(Date.now() / 1000) - 600;
const EPOCH = 1_609_459_200;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const bob = { fid: 43, username: "bob", display_name: "Bob" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const post = { hash: hex(1), author: alice, text: "What should I read next?", ts, likes: 3, recasts: 0, replies: 1 };
const bobs = { hash: hex(2), author: bob, text: "Dune, again", ts: ts + 60, likes: 1, recasts: 0, replies: 0, parent_hash: post.hash };
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" };

/** The page at a width: 1280 with a mouse, or a 390 px phone, by touch. */
type At = { page: Page; press: (control: Locator) => Promise<void>; close: () => Promise<void> };

async function at(width: 1280 | 390, desktop: Page, browser: Browser, testInfo: TestInfo): Promise<At> {
  if (width === 1280) return { page: desktop, press: (control) => control.click(), close: async () => {} };
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  return { page: await context.newPage(), press: (control) => control.tap(), close: () => context.close() };
}

type Served = { node: StandIn; listing: { fresh: boolean } };

/**
 * The Worker and the node as stand-ins, and the reader signed in with a key
 * approved an hour ago, on the post's page. The node does with each new
 * message what `answer` says, after `slow` ms. Until `listing.fresh`, the
 * Worker's copy of the post doesn't have the reader's replies, as a copy
 * cached before them wouldn't.
 */
async function ready(page: Page, answer?: (n: number, message: Envelope) => Answer, slow = 0): Promise<Served> {
  await signIn(page, me);
  let key = "";
  const node = await standIn(page, { fid: me.fid, key: () => key, answer, slow });
  const listing = { fresh: false };
  const mine = () =>
    [...node.merged.values()]
      .filter((sent) => sent.type === 1)
      .map((sent) => {
        const { text, parent } = castOf(sent);
        return { hash: sent.hash, author: me, text, ts: sent.at + EPOCH, likes: 0, recasts: 0, replies: 0, parent_hash: parent?.cast?.hash };
      });
  const thread = () => {
    const replies = listing.fresh ? mine() : [];
    const under = (hash: string) => replies.filter((r) => r.parent_hash === hash).map((cast) => ({ cast, children: [] }));
    return {
      cast: { ...post, replies: post.replies + under(post.hash).length },
      children: [...under(post.hash), { cast: bobs, children: under(bobs.hash) }],
    };
  };
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [post], notes: {}, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => route.fulfill({ json: { thread: thread(), notes: {}, meta } }));
  await page.route("**/api/v1/replies?*", (route) => route.fulfill({ json: { hash: post.hash, replies: [], more: false, meta } }));
  await page.route("**/api/v1/user?*", (route) =>
    route.fulfill({ json: { user: { ...me, bio: "", follower_count: 5, following_count: 3 }, pool: [], notes: {}, meta } }),
  );
  await page.route("**/api/v1/refresh", (route) => route.fulfill({ json: { expired: 1 } }));
  await page.route("**/api/v1/signer", (route) => route.fulfill({ json: { app_fid: 3356318 } }));
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  await page.route(`${NODE}/signersByFid?*`, (route) => route.fulfill({ headers: cors, json: { signers: [{ key }] } }));
  await page.route(`${NODE}/linkById?*`, (route) =>
    route.fulfill({ status: 400, headers: cors, json: { error: "Failed to get link", error_detail: 'status: NotFound, message: "not found"' } }),
  );
  await page.goto(`/p/${post.hash}`);
  await expect(page.locator(".post")).toBeVisible();
  key = await approvedKey(page, me.fid, 3_600_000);
  // Ready to write once the page has read the key: it then reads the
  // reader's likes and recasts from the node.
  const keyRead = page.waitForRequest((request) => request.url().includes("/reactionsByFid?"));
  await page.reload();
  await appCame(page);
  await keyRead;
  await expect(page.locator(".replies .comments > .comment")).toHaveCount(1);
  return { node, listing };
}

/** Reply to `whose` cast with `text`, from its Reply button. */
async function reply(at: At, whose: string, text: string) {
  await at.press(at.page.getByRole("button", { name: `Reply to @${whose}` }));
  const dialog = at.page.getByRole("dialog", { name: "Reply" });
  await dialog.getByRole("textbox").fill(text);
  await at.press(dialog.getByRole("button", { name: "Reply", exact: true }));
  return dialog;
}

const mine = (page: Page) => page.locator(".replies .comment.mine");
const count = (page: Page) => page.locator(".post .stat-replies");
const heard = (page: Page) => page.locator("p.heard[aria-live=polite]");

for (const width of [1280, 390] as const) {
  test(`at ${width}, a reply shows at the top of its post's replies at once, as posting, then as posted, and once when listed`, async ({ page: desktop, browser }, testInfo) => {
    const it = await at(width, desktop, browser, testInfo);
    const { page } = it;
    // The node takes its time to answer.
    const { node, listing } = await ready(page, undefined, 1500);
    await expect(count(page)).toContainText("1");
    const dialog = await reply(it, "alice", "Read Hyperion");
    // At once: the composer closed, and the reply at the top of the
    // replies, as posting, before the node has answered.
    await expect(dialog).toBeHidden();
    const card = page.locator(".replies .comments > .comment").first();
    await expect(card).toHaveClass(/\bmine\b/);
    await expect(card).toHaveClass(/\bpending\b/);
    expect(node.answers).toHaveLength(0);
    await expect(card.locator(".comment-text")).toHaveText("Read Hyperion");
    await expect(card.locator(".comment-head .when")).toHaveText("Posting…");
    await expect(card).toHaveAttribute("aria-busy", "true");
    await expect(card).toHaveCSS("opacity", "0.85");
    await expect(card.locator(".comment-foot")).toHaveJSProperty("inert", true);
    await expect(card.locator(".comment-head a.author")).toHaveText(`@${me.username}`);
    // One more reply, on the post and in the heading.
    await expect(count(page)).toContainText("2");
    await expect(page.locator(".replies-head h2")).toHaveText("Replies2");
    await page.screenshot({ path: testInfo.outputPath(`reply-in-thread-${width}-posting.png`) });

    // The node has it: a reply like the others, with its time, which links
    // to it, and its actions; screen readers hear so, and nothing pops up.
    await expect(card).not.toHaveClass(/\bpending\b/, { timeout: 15_000 });
    const hash = node.sent[0].hash;
    expect(castOf(node.sent[0])).toMatchObject({ text: "Read Hyperion", parent: { cast: { fid: alice.fid, hash: post.hash } } });
    expect(node.merged.has(hash)).toBe(true);
    await expect(card).toHaveAttribute("id", `c-${hash.slice(2, 10)}`);
    await expect(card.locator(".comment-head a.when")).toHaveAttribute("href", `/p/${hash}`);
    await expect(card.locator(".comment-head a.when time")).toBeVisible();
    await expect(card.locator(".comment-foot")).toHaveJSProperty("inert", false);
    await expect(card).toHaveCSS("opacity", "1");
    await expect(heard(page)).toHaveText("Reply posted.");
    await expect(page.locator(".toast.shown")).toHaveCount(0);
    await expect(count(page)).toContainText("2");
    await page.screenshot({ path: testInfo.outputPath(`reply-in-thread-${width}-posted.png`) });

    // In the feed, its post counts it too.
    await page.goto("/");
    await expect(page.locator(`.cast-card[data-hash="${post.hash}"] .stat-replies`)).toContainText("2");

    // Once the post's page lists it, it shows once: the page's own.
    listing.fresh = true;
    await page.goto(`/p/${post.hash}`);
    await expect(page.locator(`.replies #c-${hash.slice(2, 10)}`)).toBeVisible();
    await expect(mine(page)).toHaveCount(0);
    await expect(page.getByText("Read Hyperion")).toHaveCount(1);
    await expect(count(page)).toContainText("2");
    await it.close();
  });

  test(`at ${width}, a reply to a reply shows under it, at the top of its replies`, async ({ page: desktop, browser }, testInfo) => {
    const it = await at(width, desktop, browser, testInfo);
    const { page } = it;
    const { node } = await ready(page);
    const dialog = await reply(it, "bob", "Again and again");
    await expect(dialog).toBeHidden();
    const under = page.locator(`#c-${bobs.hash.slice(2, 10)} .children > .comment`).first();
    await expect(under).toHaveClass(/\bmine\b/);
    await expect(under.locator(".comment-text")).toHaveText("Again and again");
    await expect(under).not.toHaveClass(/\bpending\b/, { timeout: 15_000 });
    expect(castOf(node.sent[0]).parent).toEqual({ cast: { fid: bob.fid, hash: bobs.hash } });
    await expect(under.locator(".comment-head a.when")).toHaveAttribute("href", `/p/${node.sent[0].hash}`);
    // The post's own replies are as they were.
    await expect(page.locator(".replies .comments > .comment")).toHaveCount(1);
    await it.close();
  });

  test(`at ${width}, a reply the node refuses goes, the count goes back, and the composer has it back with why`, async ({ page: desktop, browser }, testInfo) => {
    const it = await at(width, desktop, browser, testInfo);
    const { page } = it;
    const { node } = await ready(page, () => ({ refuse: "message has a timestamp too far in the future" }), 2000);
    const dialog = await reply(it, "alice", "Read Hyperion");
    await expect(dialog).toBeHidden();
    await expect(mine(page)).toHaveCount(1);
    await expect(count(page)).toContainText("2");
    // Refused: gone, and the draft is back, open, with the node's words.
    await expect(dialog).toBeVisible();
    await expect(mine(page)).toHaveCount(0);
    await expect(count(page)).toContainText("1");
    await expect(dialog.locator(".compose-error")).toHaveText("Farcaster didn't take it: message has a timestamp too far in the future");
    await expectDraft(dialog.getByRole("textbox"), "Read Hyperion");
    // The same message would be refused again: no Retry; Reply sends it anew.
    await expect(dialog.getByRole("button", { name: "Retry" })).toHaveCount(0);
    expect(node.sent).toHaveLength(1);
    expect(node.reads.filter((read) => read.startsWith("castById"))).toHaveLength(0);
    await page.screenshot({ path: testInfo.outputPath(`reply-in-thread-${width}-refused.png`) });
    await it.close();
  });

  test(`at ${width}, a reply the node never shows goes after 15 s, the composer has it back, and Retry sends the same message`, async ({ page: desktop, browser }, testInfo) => {
    const it = await at(width, desktop, browser, testInfo);
    const { page } = it;
    const { node } = await ready(page, (n) => (n === 1 ? "never" : "merge"));
    const dialog = await reply(it, "alice", "Read Hyperion");
    await expect(dialog).toBeHidden();
    await expect(mine(page)).toHaveCount(1);
    // Taken, but never in the node's store: looked for until 15 s, then gone.
    await expect(dialog).toBeVisible({ timeout: 30_000 });
    await expect(mine(page)).toHaveCount(0);
    await expect(count(page)).toContainText("1");
    await expect(dialog.locator(".compose-error")).toContainText("Your reply didn't show up on Farcaster in time, so it's taken back.");
    await expectDraft(dialog.getByRole("textbox"), "Read Hyperion");
    const retry = dialog.getByRole("button", { name: "Retry" });
    await expect(retry).toBeVisible();
    expect(node.reads.filter((read) => read === `castById?fid=${me.fid}&hash=${node.sent[0].hash}`)).toHaveLength(6);
    // Changed, the draft is no longer what was signed: no Retry.
    await dialog.getByRole("textbox").fill("Read Hyperion, then Endymion");
    await expect(retry).toHaveCount(0);
    await dialog.getByRole("textbox").fill("Read Hyperion");
    await expect(retry).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath(`reply-in-thread-${width}-unseen.png`) });
    // Retry: the same message again, posting, then posted.
    await it.press(retry);
    await expect(dialog).toBeHidden();
    await expect(mine(page)).toHaveCount(1);
    await expect(mine(page)).not.toHaveClass(/\bpending\b/, { timeout: 15_000 });
    expect(node.sent).toHaveLength(2);
    expect(node.sent[1].hash).toBe(node.sent[0].hash);
    expect(node.merged.has(node.sent[0].hash)).toBe(true);
    await expect(count(page)).toContainText("2");
    await it.close();
  });
}
