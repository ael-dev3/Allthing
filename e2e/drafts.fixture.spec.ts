// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";
import { expectDraft } from "./helpers/composer";
import { approvedKey, signIn } from "./helpers/farcaster";

/*
 * Drafts last, as in the Farcaster app: the composer keeps one for each
 * thing it is for (a reply to a cast, a quote of one, a cast in a channel or
 * on the front page) in the browser, so starting another, closing it or
 * reloading the page loses none. They last 14 days. Discard lets one go,
 * asked once, and so does a cast that goes out; the Drafts tab lists them,
 * takes one up again (a reply with what it answers) and deletes one.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no composer");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const now = Math.floor(Date.now() / 1000);
const day = 86_400;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const bob = { fid: 7, username: "bob", display_name: "Bob" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const post = { hash: hex(1), author: alice, text: "What should I read next?", ts: now - 3600, likes: 3, replies: 1 };
const reply = { hash: hex(2), author: bob, text: "Hyperion", ts: now - 600, likes: 0, replies: 0, parent_hash: post.hash };

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => route.fulfill({ json: { pool: [post], notes: {}, meta } }));
  await page.route("**/api/v1/cast?*", (route) =>
    route.fulfill({ json: { thread: { cast: post, children: [{ cast: reply, children: [] }] }, notes: {}, meta } }),
  );
  await page.route("https://haatz.quilibrium.com/**", (route) => route.fulfill({ headers: { "access-control-allow-origin": "*" }, json: { messages: [] } }));
}

/** The drafts the browser keeps, as their keys and texts, newest first (stored from the composer's part, a moment after a change: poll). */
const kept = (page: Page) =>
  page.evaluate(() => {
    const json = localStorage.getItem("allthing-drafts");
    return json ? (JSON.parse(json).drafts as { key: string; draft: { text: string } }[]).map((d) => [d.key, d.draft.text]) : [];
  });

async function onPost(page: Page) {
  await signIn(page, me);
  await serve(page);
  await page.goto(`/p/${post.hash}`);
  await expect(page.locator(".comment")).toHaveCount(1);
}

const replyTo = (page: Page, who: "alice" | "bob") =>
  page.locator(who === "alice" ? ".post" : ".comment").getByRole("button", { name: `Reply to @${who}` }).click();

test("a reply's draft is kept as it's written, comes back after a reload, and each reply keeps its own", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await onPost(page);
  const dialog = page.getByRole("dialog", { name: "Reply" });
  const box = dialog.getByRole("textbox", { name: "Cast your reply" });
  await replyTo(page, "alice");
  await page.keyboard.type("Hyperion, then the Fall");
  // Kept once typing pauses, before it's closed.
  await expect.poll(() => kept(page)).toEqual([[`reply:${post.hash}`, "Hyperion, then the Fall"]]);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("status").filter({ hasText: "Draft kept" })).toBeVisible();
  // Another reply has its own; the first isn't lost.
  await replyTo(page, "bob");
  await expectDraft(box, "");
  await page.keyboard.type("Agreed");
  await page.keyboard.press("Escape");
  await expect.poll(() => kept(page)).toEqual([
    [`reply:${reply.hash}`, "Agreed"],
    [`reply:${post.hash}`, "Hyperion, then the Fall"],
  ]);
  // After a reload, each comes back with its reply.
  await page.reload();
  await expect(page.locator(".comment")).toHaveCount(1);
  await replyTo(page, "alice");
  await expectDraft(box, "Hyperion, then the Fall");
  await expect(dialog.locator(".reply-parent-handle")).toHaveText("@alice");
  await page.keyboard.press("Escape");
  await replyTo(page, "bob");
  await expectDraft(box, "Agreed");
  // A new cast keeps its own too, from the front page.
  await page.keyboard.press("Escape");
  await page.goto("/");
  await page.getByRole("button", { name: "Write a cast" }).click();
  const casting = page.getByRole("dialog", { name: "New cast" });
  await expectDraft(casting.getByRole("textbox", { name: "What's happening?" }), "");
  await page.keyboard.type("Reading list");
  await page.keyboard.press("Escape");
  await expect.poll(async () => (await kept(page)).map(([key]) => key)).toEqual(["cast:home", `reply:${reply.hash}`, `reply:${post.hash}`]);
});

test("a draft lasts 14 days", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  const draft = (hash: string, text: string, at: number) => ({
    key: `reply:${hash}`,
    draft: { text, reply: { hash, fid: 7, username: "x", excerpt: "" } },
    updated_at: at,
  });
  await page.addInitScript(
    (drafts) => localStorage.setItem("allthing-drafts", JSON.stringify({ drafts })),
    [draft(post.hash, "fifteen days old", now - 15 * day), draft(reply.hash, "thirteen days old", now - 13 * day)],
  );
  await onPost(page);
  const box = page.getByRole("dialog", { name: "Reply" }).getByRole("textbox", { name: "Cast your reply" });
  await replyTo(page, "alice");
  await expectDraft(box, "");
  await page.keyboard.press("Escape");
  await replyTo(page, "bob");
  await expectDraft(box, "thirteen days old");
  await page.keyboard.press("Escape");
  // The old one is gone from the browser too.
  await expect.poll(async () => (await kept(page)).map(([key]) => key)).toEqual([`reply:${reply.hash}`]);
});

test("Discard asks once, on Keep, and lets the draft go here and in the browser", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await onPost(page);
  const dialog = page.getByRole("dialog", { name: "Reply" });
  const box = dialog.getByRole("textbox", { name: "Cast your reply" });
  await replyTo(page, "alice");
  // Nothing to discard yet.
  await expect(dialog.getByRole("button", { name: "Discard" })).toHaveCount(0);
  await page.keyboard.type("Not this one");
  const discard = dialog.getByRole("button", { name: "Discard", exact: true });
  await discard.click();
  const ask = dialog.getByRole("group", { name: "Discard this draft? It won't be kept." });
  await expect(ask.getByRole("button", { name: "Keep" })).toBeFocused();
  await ask.getByRole("button", { name: "Keep" }).click();
  await expect(ask).toHaveCount(0);
  await expect(discard).toBeFocused();
  await expectDraft(box, "Not this one");
  await discard.click();
  await ask.getByRole("button", { name: "Discard", exact: true }).click();
  await expect(ask).toHaveCount(0);
  await expectDraft(box, "");
  await expect(box).toBeFocused();
  await expect.poll(() => kept(page)).toEqual([]);
  // Closed now, nothing is said kept.
  await page.keyboard.press("Escape");
  await expect(page.getByRole("status").filter({ hasText: "Draft kept" })).toHaveCount(0);
});

test("a reply that goes out lets its draft go", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await onPost(page);
  // A key approved an hour ago, and a node that takes what it signs.
  const node = "https://haatz.quilibrium.com/v1";
  const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" };
  let key = "";
  let sent = 0;
  await page.route("**/api/v1/signer", (route) => route.fulfill({ json: { app_fid: 9152 } }));
  await page.route(`${node}/signersByFid?*`, (route) => route.fulfill({ headers: cors, json: { signers: [{ key }] } }));
  await page.route(`${node}/submitMessage`, (route) => {
    if (route.request().method() === "OPTIONS") return route.fulfill({ status: 204, headers: cors });
    sent += 1;
    return route.fulfill({ headers: cors, json: {} });
  });
  key = await approvedKey(page, me.fid, 3_600_000);
  await page.reload();
  await expect(page.locator(".comment")).toHaveCount(1);
  await replyTo(page, "alice");
  await page.keyboard.type("Hyperion, then the Fall");
  await expect.poll(() => kept(page)).toEqual([[`reply:${post.hash}`, "Hyperion, then the Fall"]]);
  const dialog = page.getByRole("dialog", { name: "Reply" });
  await dialog.getByRole("button", { name: "Reply", exact: true }).click();
  await expect(dialog).toBeHidden();
  expect(sent).toBe(1);
  await expect.poll(() => kept(page)).toEqual([]);
  await expect(page.getByRole("status").filter({ hasText: "Draft kept" })).toHaveCount(0);
  // Opened again, it's empty.
  await replyTo(page, "alice");
  await expectDraft(dialog.getByRole("textbox", { name: "Cast your reply" }), "");
  await expect(dialog.getByRole("tab", { name: "Drafts" })).toHaveText("Drafts");
});

for (const width of [1280, 390] as const) {
  test(`at ${width}, the Drafts tab lists them, takes one up with its reply's cast, and deletes one`, async ({ browser }, testInfo) => {
    const touch = width === 390;
    const context: Awaited<ReturnType<Browser["newContext"]>> = await browser.newContext({
      baseURL: testInfo.project.use.baseURL,
      viewport: { width, height: touch ? 844 : 900 },
      isMobile: touch,
      hasTouch: touch,
    });
    const page = await context.newPage();
    await onPost(page);
    const dialog = page.getByRole("dialog", { name: "Reply" });
    await replyTo(page, "bob");
    await page.keyboard.type("Agreed, and Endymion");
    await page.keyboard.press("Escape");
    await expect(page.getByRole("status").filter({ hasText: "Draft kept" })).toBeVisible();
    await replyTo(page, "alice");
    // On a phone, the notice that it was kept doesn't sit over the sheet.
    if (touch) await expect(page.locator(".toast")).toBeHidden();
    await page.keyboard.type("Hyperion");
    // Drafts: both, newest first, with what each is for, its start and when.
    const drafts = dialog.getByRole("tab", { name: /Drafts/ });
    await expect(drafts).toContainText("2");
    if (touch) {
      // Cancel and the post button (here Post in Farcaster, the widest) on
      // the head's first row; the tabs on a row of their own under them.
      await dialog.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished.catch(() => {}))));
      const box = async (selector: string) => (await dialog.locator(selector).first().boundingBox())!;
      const [head, cancel, post, tabs] = await Promise.all([box(".compose-head"), box(".compose-cancel"), box(".compose-post"), box(".compose-tabs")]);
      await expect(dialog.locator(".compose-post")).toHaveText("Post in Farcaster");
      expect(tabs.y).toBeGreaterThanOrEqual(Math.max(cancel.y + cancel.height, post.y + post.height));
      expect(tabs.y + tabs.height).toBeLessThanOrEqual(head.y + head.height + 0.5);
      expect(tabs.x).toBeGreaterThanOrEqual(head.x);
      // Taps as tall as the bar's tools take on a phone.
      expect(Math.round(tabs.height)).toBe(40);
      expect(Math.round((await box(".compose-discard")).height)).toBe(40);
    }
    await drafts.click();
    await expect(drafts).toHaveAttribute("aria-selected", "true");
    const items = dialog.locator(".draft-item");
    await expect(items).toHaveCount(2);
    await expect(items.nth(0).locator(".draft-label")).toContainText("Reply to @alice");
    await expect(items.nth(0).locator(".draft-snippet")).toHaveText("Hyperion");
    await expect(items.nth(1).locator(".draft-label")).toContainText("Reply to @bob");
    await expect(items.nth(1).locator("time")).toHaveText("just now");
    await page.screenshot({ path: testInfo.outputPath(`drafts-${width}.png`) });
    // Bob's again, with what it answers.
    await items.nth(1).locator(".draft-open").click();
    await expect(dialog.getByRole("tab", { name: "Compose" })).toHaveAttribute("aria-selected", "true");
    await expect(dialog.locator(".reply-parent-handle")).toHaveText("@bob");
    await expectDraft(dialog.getByRole("textbox", { name: "Cast your reply" }), "Agreed, and Endymion");
    // Delete Alice's from the list.
    await drafts.click();
    await dialog.getByRole("button", { name: "Delete the draft: Reply to @alice" }).click();
    await expect(items).toHaveCount(1);
    await expect.poll(async () => (await kept(page)).map(([key]) => key)).toEqual([`reply:${reply.hash}`]);
    if (touch) {
      // Discard's question on a line of its own, Keep and Discard together under it.
      await dialog.getByRole("tab", { name: "Compose" }).click();
      await dialog.getByRole("button", { name: "Discard", exact: true }).click();
      const ask = dialog.getByRole("group", { name: "Discard this draft? It won't be kept." });
      const [question, keep, discard] = await Promise.all(
        [ask.locator("span"), ask.getByRole("button", { name: "Keep" }), ask.getByRole("button", { name: "Discard", exact: true })].map(async (l) => (await l.boundingBox())!),
      );
      expect(Math.round(keep.y)).toBe(Math.round(discard.y));
      expect(keep.y).toBeGreaterThanOrEqual(question.y + question.height - 1);
      expect(keep.x).toBeLessThan(discard.x);
    }
    await context.close();
  });
}
