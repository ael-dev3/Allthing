// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Page, type TestInfo } from "@playwright/test";
import { expectDraft } from "./helpers/composer";
import { approvedKey, cast as castOf, envelope, signIn } from "./helpers/farcaster";

/*
 * Scheduled casts. The clock in the composer picks when a cast goes out:
 * the browser signs it at once with the reader's key, for that time, and
 * hands the signed message to the Worker, which posts it then. The reader's
 * list (/scheduled) changes or cancels one. The Worker's endpoints are a
 * stand-in keeping the list in memory; every message handed to it is
 * decoded and its signature checked. The browser's clock is in Berlin.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");
test.use({ timezoneId: "Europe/Berlin", locale: "en-GB" });

const EPOCH = 1_609_459_200;
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const post = { hash: hex(1), author: alice, text: "What should I read next?", ts, likes: 3, replies: 0 };
const NODE = "https://haatz.quilibrium.com/v1";
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" };
const now = () => Math.floor(Date.now() / 1000);

type Kept = { id: number; at: number; hash: string; state: "waiting" | "posted" | "failed"; error?: string; draft: string };
type Asked = { message: string; draft: string; replaces?: number };

/** The Worker's scheduled casts, in memory, and the node, which must not be asked. */
async function serve(page: Page, list: Kept[] = [], refuse?: [number, string]) {
  const calls = { scheduled: [] as Asked[], cancelled: [] as number[], submitted: 0, listed: 0 };
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [post], notes: {}, meta } });
  });
  await page.route("**/api/v1/signer", (route) => route.fulfill({ json: { app_fid: 9152 } }));
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  await page.route(`${NODE}/userNameProofByName?*`, (route) => {
    const name = new URL(route.request().url()).searchParams.get("name")!;
    return name === "alice"
      ? route.fulfill({ headers: cors, json: { name, fid: alice.fid, type: "USERNAME_TYPE_FNAME" } })
      : route.fulfill({ status: 400, headers: cors, json: { error: "Failed", error_detail: 'status: NotFound, message: "Username proof not found"' } });
  });
  await page.route(`${NODE}/submitMessage`, (route) => {
    calls.submitted += 1;
    return route.fulfill({ headers: cors, json: {} });
  });
  await page.route("**/api/v1/scheduled", async (route) => {
    const request = route.request();
    if (request.method() === "GET") {
      calls.listed += 1;
      return route.fulfill({ json: { casts: [...list].sort((a, b) => a.at - b.at) } });
    }
    expect(request.headers()["content-type"]).toBe("application/json");
    const asked: Asked = request.postDataJSON();
    calls.scheduled.push(asked);
    if (refuse) return route.fulfill({ status: refuse[0], json: { error: refuse[1] } });
    const signed = envelope(Buffer.from(asked.message, "base64"));
    const fields = { at: signed.at + EPOCH, hash: signed.hash, draft: asked.draft, state: "waiting" as const, error: undefined };
    const kept = asked.replaces ? Object.assign(list.find((k) => k.id === asked.replaces)!, fields) : { id: 100 + calls.scheduled.length, ...fields };
    if (!asked.replaces) list.push(kept);
    return route.fulfill({ json: kept });
  });
  await page.route("**/api/v1/scheduled/cancel", (route) => {
    const { id } = route.request().postDataJSON();
    calls.cancelled.push(id);
    list.splice(list.findIndex((k) => k.id === id), 1);
    return route.fulfill({ json: { outcome: "cancelled" } });
  });
  return calls;
}

/** Signed in, with a key approved an hour ago. */
async function ready(page: Page, list: Kept[] = [], refuse?: [number, string]) {
  await signIn(page, me);
  const calls = await serve(page, list, refuse);
  await page.goto("/");
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  await approvedKey(page, me.fid, 3_600_000);
  await page.reload();
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  return calls;
}

/** A moment as the page shows it ("Thu 22 Oct, 09:00") and as its date-and-time field holds it, in the browser's own zone. */
async function shown(page: Page, at: number) {
  return page.evaluate((at) => {
    const d = new Date(at * 1000);
    const pad = (n: number) => String(n).padStart(2, "0");
    const options = { weekday: "short", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit", hourCycle: "h23" } as const;
    return {
      text: d.toLocaleString("en-GB", options),
      input: `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`,
    };
  }, at);
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({
    baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true,
    timezoneId: "Europe/Berlin", locale: "en-GB",
  });
  return { context, page: await context.newPage() };
}

test("the clock schedules a cast: signed now for the time picked, kept by the Worker, and listed", async ({ page }) => {
  const calls = await ready(page);
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await dialog.getByRole("textbox", { name: "What's happening?" }).fill("Tomorrow, @alice");
  const clock = dialog.getByRole("button", { name: "Schedule for later" });
  await expect(clock).toHaveAttribute("aria-pressed", "false");
  await clock.click();
  await expect(clock).toHaveAttribute("aria-pressed", "true");
  // An hour or so from now to begin with, in the reader's own time.
  const field = dialog.getByLabel("Goes out");
  await expect(field).toHaveAttribute("type", "datetime-local");
  const start = await field.inputValue();
  const ahead = (await page.evaluate((v) => new Date(v).getTime() / 1000, start)) - now();
  expect(ahead).toBeGreaterThan(3_600);
  expect(ahead).toBeLessThanOrEqual(3_600 + 900);
  await expect(dialog.locator(".compose-zone")).toHaveText("Your time, Europe/Berlin");
  // Pick tomorrow at the next whole hour, as the field shows it.
  const at = (Math.floor(now() / 3600) + 25) * 3600;
  await field.fill((await shown(page, at)).input);
  await dialog.getByRole("button", { name: "Schedule", exact: true }).click();
  await expect(dialog).toBeHidden();
  // Signed for that time, by the reader, with the mention; not posted now.
  expect(calls.scheduled).toHaveLength(1);
  const signed = envelope(Buffer.from(calls.scheduled[0].message, "base64"));
  expect([signed.type, signed.fid, signed.network, signed.at + EPOCH]).toEqual([1, me.fid, 1, at]);
  expect(castOf(signed)).toMatchObject({ text: "Tomorrow, ", mentions: [alice.fid], positions: [10], parent: null });
  expect(JSON.parse(calls.scheduled[0].draft)).toEqual({ text: "Tomorrow, @alice" });
  expect(calls.scheduled[0].replaces).toBeUndefined();
  expect(calls.submitted).toBe(0);
  const when = await shown(page, at);
  const notice = page.getByRole("status").filter({ hasText: "Scheduled: it goes out" });
  await expect(notice).toContainText(`Scheduled: it goes out ${when.text}.`);
  // Its link lists it, waiting.
  await notice.getByRole("link", { name: "View" }).click();
  await expect(page).toHaveURL(/\/scheduled$/);
  await expect(page.getByRole("heading", { level: 1, name: "Scheduled casts" })).toBeVisible();
  const waiting = page.locator(".scheduled-group", { has: page.getByRole("heading", { name: /Waiting/ }) });
  await expect(waiting.locator(".scheduled-item")).toHaveCount(1);
  await expect(waiting.locator(".scheduled-when")).toContainText(`${when.text} · in 1 day`);
  await expect(waiting.locator(".scheduled-text")).toHaveText("Tomorrow, @alice");
  // Back on the feed, the next draft is new, posted now.
  await page.goBack();
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  await page.getByRole("button", { name: "Write a cast" }).click();
  await expectDraft(page.getByRole("dialog", { name: "New cast" }).getByRole("textbox", { name: "What's happening?" }), "");
  await expect(page.getByRole("button", { name: "Schedule for later" })).toHaveAttribute("aria-pressed", "false");
});

test("a time too soon, or too far, is refused before anything is signed; the Worker's refusal is said", async ({ page }) => {
  const calls = await ready(page, [], [409, "You have 25 casts waiting already: cancel one, or wait for one to go out"]);
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  const text = dialog.getByRole("textbox", { name: "What's happening?" });
  await text.fill("Too soon");
  await dialog.getByRole("button", { name: "Schedule for later" }).click();
  const field = dialog.getByLabel("Goes out");
  await field.fill((await shown(page, now())).input);
  await dialog.getByRole("button", { name: "Schedule", exact: true }).click();
  await expect(dialog.getByRole("alert")).toHaveText("Pick a time at least a minute from now.");
  await field.fill((await shown(page, now() + 40 * 86_400)).input);
  await dialog.getByRole("button", { name: "Schedule", exact: true }).click();
  await expect(dialog.getByRole("alert")).toHaveText("Pick a time within 30 days.");
  expect(calls.scheduled).toHaveLength(0);
  // Fine by the browser, refused by the Worker: the draft stays, with why.
  await field.fill((await shown(page, (Math.floor(now() / 3600) + 3) * 3600)).input);
  await dialog.getByRole("button", { name: "Schedule", exact: true }).click();
  await expect(dialog.getByRole("alert")).toHaveText("You have 25 casts waiting already: cancel one, or wait for one to go out");
  await expectDraft(text, "Too soon");
  expect(calls.scheduled).toHaveLength(1);
  // The clock again: posted now after all.
  await dialog.getByRole("button", { name: "Schedule for later" }).click();
  await expect(dialog.getByLabel("Goes out")).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: "Cast", exact: true })).toBeVisible();
});

test("the list: one waiting to change or cancel, one that didn't go out, one posted", async ({ page }) => {
  const hour = Math.floor(now() / 3600) * 3600;
  const draft = (d: object) => JSON.stringify(d);
  const list: Kept[] = [
    { id: 1, at: hour + 3 * 3600, hash: hex(11), state: "waiting", draft: draft({ text: "Later reply", reply: { hash: post.hash, fid: alice.fid, username: "alice", excerpt: "What should I read next?" } }) },
    { id: 2, at: hour - 3600, hash: hex(12), state: "failed", error: "The key it was signed with isn't on your Farcaster account anymore. Edit it to sign it again.", draft: draft({ text: "Lost one" }) },
    { id: 3, at: hour - 7200, hash: hex(13), state: "posted", draft: draft({ text: "Out already", channel: "books", channel_url: "https://farcaster.xyz/~/channel/books" }) },
  ];
  const calls = await ready(page, list);
  // From the account menu.
  await page.locator(".account-button").hover();
  await page.getByRole("link", { name: "Scheduled casts" }).click();
  await expect(page).toHaveURL(/\/scheduled$/);
  const group = (name: RegExp) => page.locator(".scheduled-group", { has: page.getByRole("heading", { name }) });
  await expect(group(/^Waiting/).locator(".scheduled-item")).toHaveCount(1);
  await expect(group(/^Didn't go out/).locator(".scheduled-item")).toHaveCount(1);
  await expect(group(/^Posted this week/).locator(".scheduled-item")).toHaveCount(1);
  const waiting = group(/^Waiting/).locator(".scheduled-item");
  await expect(waiting.locator(".scheduled-context")).toHaveText("Reply to @alice");
  await expect(waiting.locator(".scheduled-when")).toContainText(`${(await shown(page, list[0].at)).text} · in 2 h`);
  await expect(group(/^Didn't go out/)).toContainText("The key it was signed with isn't on your Farcaster account anymore.");
  const posted = group(/^Posted this week/);
  await expect(posted.locator(".scheduled-context")).toHaveText("In /books");
  await expect(posted.getByRole("link", { name: "View cast" })).toHaveAttribute("href", `/p/${hex(13)}`);
  await expect(posted.getByRole("button")).toHaveCount(0);

  // Edit: the composer has it as it was, still scheduled, saved in its place.
  await waiting.getByRole("button", { name: "Edit" }).click();
  const dialog = page.getByRole("dialog", { name: "Scheduled cast" });
  await expect(dialog).toContainText("Replying to @alice");
  const reply = dialog.getByRole("textbox", { name: "Cast your reply" });
  await expectDraft(reply, "Later reply");
  await expect(dialog.getByRole("button", { name: "Schedule for later" })).toBeDisabled();
  const field = dialog.getByLabel("Goes out");
  await expect(field).toHaveValue((await shown(page, list[0].at)).input);
  await reply.fill("Later reply, changed");
  const at = list[0].at + 3600;
  await field.fill((await shown(page, at)).input);
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect(dialog).toBeHidden();
  expect(calls.scheduled).toHaveLength(1);
  expect(calls.scheduled[0].replaces).toBe(1);
  const signed = envelope(Buffer.from(calls.scheduled[0].message, "base64"));
  expect(signed.at + EPOCH).toBe(at);
  expect(castOf(signed)).toMatchObject({ text: "Later reply, changed", parent: { cast: { fid: alice.fid, hash: post.hash } } });
  await expect(page.getByRole("status")).toContainText(`Changed: it goes out ${(await shown(page, at)).text}.`);
  await expect(waiting.locator(".scheduled-text")).toHaveText("Later reply, changed");
  expect(calls.submitted).toBe(0);

  // Cancel, after asking.
  await waiting.getByRole("button", { name: "Cancel" }).click();
  const asking = waiting.getByRole("group", { name: "Cancel this cast? It won't go out." });
  await asking.getByRole("button", { name: "Keep it" }).click();
  await expect(asking).toHaveCount(0);
  await waiting.getByRole("button", { name: "Cancel" }).click();
  await waiting.getByRole("button", { name: "Yes, cancel it" }).click();
  await expect(group(/^Waiting/)).toHaveCount(0);
  expect(calls.cancelled).toEqual([1]);

  // One that didn't go out comes off the list the same way.
  const lost = group(/^Didn't go out/).locator(".scheduled-item");
  await expect(lost.getByRole("button", { name: "Edit to try again" })).toBeVisible();
  await lost.getByRole("button", { name: "Remove" }).click();
  await lost.getByRole("button", { name: "Yes, remove it" }).click();
  await expect(group(/^Didn't go out/)).toHaveCount(0);
  expect(calls.cancelled).toEqual([1, 2]);
});

test("signed out, the list asks to sign in first; with nothing scheduled, it says how", async ({ page }) => {
  await serve(page);
  await page.goto("/scheduled");
  await expect(page.locator(".scheduled-empty")).toContainText("Sign in to see your scheduled casts.");
  await page.locator(".scheduled-empty").getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByRole("dialog", { name: "Sign in with Farcaster" })).toBeVisible();

  await signIn(page, me);
  await page.goto("/scheduled");
  await expect(page.locator(".scheduled-empty")).toContainText("Nothing scheduled.");
});

test("at 390, the clock's row and the list fit the screen", async ({ browser }, testInfo) => {
  const { context, page } = await phone(browser, testInfo);
  const list: Kept[] = [{ id: 1, at: now() + 7200, hash: hex(11), state: "waiting", draft: JSON.stringify({ text: "A long one ".repeat(20) }) }];
  await ready(page, list);
  await page.getByRole("button", { name: "Write a cast" }).first().tap();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await dialog.getByRole("button", { name: "Schedule for later" }).tap();
  const row = (await dialog.locator(".compose-when").boundingBox())!;
  expect(row.x).toBeGreaterThanOrEqual(0);
  expect(row.x + row.width).toBeLessThanOrEqual(390);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await dialog.getByRole("button", { name: "Close" }).tap();
  await page.goto("/scheduled");
  const item = (await page.locator(".scheduled-item").first().boundingBox())!;
  expect(item.x + item.width).toBeLessThanOrEqual(390);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await context.close();
});

test("the real Worker keeps scheduled casts only for a session, and only from Allthing", async ({ request, baseURL }) => {
  expect((await request.get("/api/v1/scheduled")).status()).toBe(401);
  const foreign = await request.post("/api/v1/scheduled", { data: { message: "AA==", draft: "{}" }, headers: { origin: "https://elsewhere.example" } });
  expect(foreign.status()).toBe(403);
  const ours = await request.post("/api/v1/scheduled/cancel", { data: { id: 1 }, headers: { origin: new URL(baseURL!).origin } });
  expect(ours.status()).toBe(401);
  expect((await request.delete("/api/v1/scheduled")).status()).toBe(405);
});
