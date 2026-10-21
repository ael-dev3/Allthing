// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Page } from "@playwright/test";
import { expectDraft } from "./helpers/composer";
import { signIn } from "./helpers/farcaster";

/*
 * A cast, a reply, a quote or a Pepe reaction handed to the Farcaster app,
 * as on allthing.social (where the Worker endorses no keys): signed in, it
 * shows here at once, where it will be, and as posted once the node lists
 * it among the reader's latest casts. The node and farcaster.xyz are
 * stubbed; what the reader "posts in the app" is pushed to `posted`.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no Worker to hand off with");

const NODE = "https://haatz.quilibrium.com/v1";
const EPOCH = 1_609_459_200;
const cors = { "access-control-allow-origin": "*" };
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const theirs = { hash: hex(1), author: alice, text: "Hand this off", ts, likes: 3, recasts: 1, replies: 1 };
const reply = { hash: hex(5), author: alice, text: "A reply already there", ts: ts + 30, likes: 0, recasts: 0, replies: 0, parent_hash: theirs.hash };

/** A cast as the node lists it: `parent` and `quote` by hash. */
type Posted = { hash: string; text: string; parent?: string; quote?: string };

async function serve(page: Page) {
  const posted: Posted[] = [];
  const asked = { casts: 0 };
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [theirs], notes: {}, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) =>
    route.fulfill({ json: { thread: { cast: theirs, children: [{ cast: reply, children: [] }] }, notes: {}, meta } }),
  );
  await page.route("**/api/v1/signer", (route) => route.fulfill({ status: 404, json: { error: "not set up" } }));
  await page.route("**/api/v1/auth/signer", (route) => route.fulfill({ status: 404, json: { error: "not set up" } }));
  await page.route("**/api/v1/auth/qr", (route) => route.continue());
  await page.route("https://farcaster.xyz/**", (route) => route.fulfill({ body: "<title>Farcaster</title>", contentType: "text/html" }));
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  await page.route(`${NODE}/reactionById?*`, (route) =>
    route.fulfill({ status: 400, headers: cors, json: { error: "Failed to get reaction", error_detail: 'status: NotFound, message: "not found"' } }),
  );
  // The reader's latest casts, newest first.
  await page.route(`${NODE}/castsByFid?*`, (route) => {
    asked.casts += 1;
    const now = Math.floor(Date.now() / 1000) - EPOCH;
    const messages = [...posted].reverse().map((cast) => ({
      data: {
        type: "MESSAGE_TYPE_CAST_ADD",
        fid: me.fid,
        timestamp: now,
        castAddBody: {
          text: cast.text,
          embeds: cast.quote ? [{ castId: { fid: alice.fid, hash: cast.quote } }] : [],
          ...(cast.parent ? { parentCastId: { fid: alice.fid, hash: cast.parent } } : {}),
        },
      },
      hash: cast.hash,
    }));
    return route.fulfill({ headers: cors, json: { messages, nextPageToken: "" } });
  });
  return { posted, asked };
}

const outgoing = (page: Page) => page.locator(".cast-card.outgoing");

test("a reply shows above the replies at once, then as posted, with the count", async ({ page }, info) => {
  await signIn(page, me);
  const { posted } = await serve(page);
  await page.goto(`/p/${theirs.hash}`);
  const post = page.locator(".post");
  await expect(post.locator(".stat-replies")).toContainText("1");
  await post.getByRole("button", { name: "Reply to @alice" }).click();
  await page.keyboard.type("so true");
  await page.getByRole("dialog", { name: "Reply" }).getByRole("button", { name: "Post in Farcaster" }).click();
  const dialog = page.getByRole("dialog", { name: "Post in Farcaster" });
  await expect(dialog).toContainText("Your reply, ready to post.");
  // At once, under the dialog: the reply above the replies, and one more.
  await expect(outgoing(page)).toHaveCount(1);
  await expect(outgoing(page)).toContainText("so true");
  await expect(outgoing(page)).toContainText("Posting in Farcaster");
  await expect(post.locator(".stat-replies")).toContainText("2");
  await expect(page.locator(".replies-head h2")).toHaveText("Replies2");
  // Once the dialog has faded in over the composer.
  await page.waitForTimeout(500);
  await page.screenshot({ path: info.outputPath("handoff-posts-1280-waiting.png") });
  // Scanned with the phone: Done hands the draft over.
  await dialog.getByRole("button", { name: "Done" }).click();
  await expect(page.getByRole("dialog", { name: "Reply" })).toHaveCount(0);
  await expect(outgoing(page)).toContainText("Posting in Farcaster");
  // The reader posts it in the app, a little changed.
  posted.push({ hash: hex(9), text: "so true!", parent: theirs.hash });
  const done = outgoing(page).getByRole("link", { name: "Posted" });
  await expect(done).toBeVisible({ timeout: 10_000 });
  await expect(done).toHaveAttribute("href", /\/p\/0x0+9$/);
  await expect(outgoing(page)).not.toContainText("Posting in Farcaster");
  await expect(page.getByRole("status").filter({ hasText: "Posted." })).toBeVisible();
  await expect(page.getByRole("status").getByRole("link", { name: "View" })).toHaveAttribute("href", /\/p\/0x0+9$/);
  await expect(post.locator(".stat-replies")).toContainText("2");
  await page.mouse.move(640, 880);
  await page.screenshot({ path: info.outputPath("handoff-posts-1280-posted.png") });
});

test("a Pepe reaction shows as the reader's chip at once, then as posted", async ({ page }) => {
  await signIn(page, me);
  const { posted } = await serve(page);
  await page.goto(`/p/${theirs.hash}`);
  const post = page.locator(".post");
  await post.getByRole("button", { name: "React to @alice's cast with an emoji" }).click();
  await page.getByRole("dialog", { name: "React with a Pepe" }).getByRole("button", { name: /^Pepe/ }).first().click();
  const dialog = page.getByRole("dialog", { name: "React in Farcaster" });
  const href = (await dialog.getByRole("link", { name: /Open on farcaster\.xyz/ }).getAttribute("href"))!;
  const text = new URL(href).searchParams.get("text")!;
  const chip = post.locator(":scope > .reaction-chips .reaction-chip");
  await expect(chip).toHaveCount(1);
  await expect(chip).toHaveAttribute("aria-pressed", "true");
  await expect(chip).toHaveClass(/\bpending\b/);
  posted.push({ hash: hex(10), text, parent: theirs.hash });
  await expect(chip).not.toHaveClass(/\bpending\b/, { timeout: 10_000 });
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole("status").filter({ hasText: "Reacted." })).toBeVisible();
});

test("a cast and a quote show above the feed at once, then as posted; closed unsent, a draft stays", async ({ page }) => {
  await signIn(page, me);
  const { posted } = await serve(page);
  await page.goto("/");
  // Closed without going to the app: nothing shows, and the draft is kept.
  await page.getByRole("button", { name: "Write a cast" }).click();
  await page.keyboard.type("gm everyone");
  await page.getByRole("dialog", { name: "New cast" }).getByRole("button", { name: "Post in Farcaster" }).click();
  await expect(outgoing(page)).toHaveCount(1);
  await page.keyboard.press("Escape");
  await expect(outgoing(page)).toHaveCount(0);
  await expectDraft(page.getByRole("dialog", { name: "New cast" }).getByRole("textbox"), "gm everyone");
  // Sent: it shows above the feed, then as posted.
  await page.getByRole("dialog", { name: "New cast" }).getByRole("button", { name: "Post in Farcaster" }).click();
  await page.getByRole("dialog", { name: "Post in Farcaster" }).getByRole("button", { name: "Done" }).click();
  await expect(outgoing(page)).toHaveCount(1);
  await expect(outgoing(page)).toContainText("gm everyone");
  posted.push({ hash: hex(11), text: "gm everyone" });
  await expect(outgoing(page).getByRole("link", { name: "Posted" })).toBeVisible({ timeout: 10_000 });

  // A quote, from the post: above the feed too, found by the cast it quotes.
  await page.goto(`/p/${theirs.hash}`);
  await page.locator(".post").getByRole("button", { name: "Quote @alice's cast" }).click();
  await page.keyboard.type("look");
  await page.getByRole("dialog", { name: "Quote" }).getByRole("button", { name: "Post in Farcaster" }).click();
  await page.getByRole("dialog", { name: "Post in Farcaster" }).getByRole("button", { name: "Done" }).click();
  // Back to the feed, in the app.
  await page.getByRole("link", { name: "Home", exact: true }).first().click();
  await expect(outgoing(page).filter({ hasText: "quoting @alice" })).toHaveCount(1);
  posted.push({ hash: hex(12), text: "look", quote: theirs.hash });
  await expect(outgoing(page).filter({ hasText: "quoting @alice" }).getByRole("link", { name: "Posted" })).toBeVisible({ timeout: 10_000 });
});

test("on a phone, a reply sent by its link shows at once, and as posted on coming back", async ({ browser }, info) => {
  const context = await browser.newContext({ baseURL: info.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  const page = await context.newPage();
  await signIn(page, me);
  const { posted } = await serve(page);
  await page.goto(`/p/${theirs.hash}`);
  await page.locator(".post").getByRole("button", { name: "Reply to @alice" }).tap();
  await page.keyboard.type("from my phone");
  const [app] = await Promise.all([
    context.waitForEvent("page"),
    page.getByRole("dialog", { name: "Reply" }).getByRole("link", { name: /Post in Farcaster/ }).tap(),
  ]);
  await expect(outgoing(page)).toContainText("from my phone");
  await expect(outgoing(page)).toContainText("Posting in Farcaster");
  await page.screenshot({ path: info.outputPath("handoff-posts-390-waiting.png") });
  posted.push({ hash: hex(13), text: "from my phone", parent: theirs.hash });
  await app.close();
  await page.bringToFront();
  await expect(outgoing(page).getByRole("link", { name: "Posted" })).toBeVisible({ timeout: 10_000 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await page.screenshot({ path: info.outputPath("handoff-posts-390-posted.png") });
  await context.close();
});

test("not posted in time, it goes, and the draft is back in the composer", async ({ page }) => {
  await page.clock.install();
  await signIn(page, me);
  const { asked } = await serve(page);
  await page.goto(`/p/${theirs.hash}`);
  await page.locator(".post").getByRole("button", { name: "Reply to @alice" }).click();
  await page.keyboard.type("never sent");
  await page.getByRole("dialog", { name: "Reply" }).getByRole("button", { name: "Post in Farcaster" }).click();
  await page.getByRole("dialog", { name: "Post in Farcaster" }).getByRole("button", { name: "Done" }).click();
  await expect(outgoing(page)).toHaveCount(1);
  await expect.poll(() => asked.casts).toBeGreaterThan(0);
  // Ten minutes on.
  await page.clock.fastForward("10:30");
  await expect(outgoing(page)).toHaveCount(0, { timeout: 10_000 });
  await expect(page.locator(".post .stat-replies")).toContainText("1");
  await page.locator(".post").getByRole("button", { name: "Reply to @alice" }).click();
  await expectDraft(page.getByRole("dialog", { name: "Reply" }).getByRole("textbox"), "never sent");
});

test("signed out, a draft handed off shows nothing and asks the node nothing", async ({ page }) => {
  const { asked } = await serve(page);
  await page.goto(`/p/${theirs.hash}`);
  await page.locator(".post").getByRole("button", { name: "Reply to @alice" }).click();
  await page.keyboard.type("hello");
  await page.getByRole("dialog", { name: "Reply" }).getByRole("button", { name: "Post in Farcaster" }).click();
  await page.getByRole("dialog", { name: "Post in Farcaster" }).getByRole("button", { name: "Done" }).click();
  await page.waitForTimeout(4_000);
  await expect(outgoing(page)).toHaveCount(0);
  expect(asked.casts).toBe(0);
});
