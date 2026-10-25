// SPDX-License-Identifier: AGPL-3.0-or-later
import axe from "axe-core";
import { expect, test, type Locator, type Page } from "@playwright/test";
import { expectDraft } from "./helpers/composer";
import { approvedKey, cast as castOf, signIn } from "./helpers/farcaster";
import { standIn } from "./helpers/node";
import { NOW } from "./fixture-time";
import { showSeenPosts } from "./helpers/app";

/*
 * The composer's "@" suggestions (components/mention.rs): typing "@al"
 * lists the accounts search finds, people the reader follows first and
 * marked "Following"; Up and Down move, Enter or Tab picks, Escape closes
 * the list and not the composer; a click or a tap picks. The pick puts
 * "@username " in the draft, which posts as a mention. At 1280 and at 390.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const alfred = { fid: 77, username: "alfred", display_name: "Alfred" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const post = { hash: hex(1), author: alice, text: "What should I read next?", ts: (NOW - 19), likes: 3, replies: 0 };
const NODE = "https://haatz.quilibrium.com/v1";
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" };
const FIDS: Record<string, number> = { alice: alice.fid, alfred: alfred.fid };

async function ready(page: Page) {
  await signIn(page, me);
  let key = "";
  const node = await standIn(page, { fid: me.fid, key: () => key });
  // The reader follows Alfred, not Alice.
  node.had.follow(me.fid, alfred.fid);
  const searched: string[] = [];
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => route.fulfill({ json: { pool: [post], notes: {}, meta } }));
  await page.route("**/api/v1/signer", (route) => route.fulfill({ json: { app_fid: 9152 } }));
  await page.route("**/api/v1/search?*", (route) => {
    const q = new URL(route.request().url()).searchParams.get("q")!;
    searched.push(q);
    const users = [alice, alfred].filter((u) => u.username.startsWith(q));
    return route.fulfill({ json: { query: q, casts: [], users, channels: [], notes: {}, meta } });
  });
  await page.route(`${NODE}/signersByFid?*`, (route) => route.fulfill({ headers: cors, json: { signers: [{ key }] } }));
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  await page.route(`${NODE}/userNameProofByName?*`, (route) => {
    const name = new URL(route.request().url()).searchParams.get("name")!;
    return FIDS[name]
      ? route.fulfill({ headers: cors, json: { name, fid: FIDS[name], type: "USERNAME_TYPE_FNAME" } })
      : route.fulfill({ status: 400, headers: cors, json: { error: "Failed to get username proof", error_detail: "status: NotFound" } });
  });
  await page.goto("/");
  await showSeenPosts(page);
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  key = await approvedKey(page, me.fid, 3_600_000);
  await page.reload();
  // The first visit may have recorded the fixture's only cast as seen.
  // Recover it through the reader's control, after the real app arrives.
  await showSeenPosts(page);
  await expect(page.locator(".cast-card:not(.skeleton)").first()).toBeVisible();
  return { node, searched };
}

/** The list, and its options' names in order, once Alfred is known to be followed. */
async function listed(dialog: Locator) {
  const list = dialog.page().getByRole("listbox", { name: "People to mention" });
  await expect(list).toBeVisible();
  await expect(list.getByRole("option", { name: /Alfred/ })).toContainText("Following");
  await expect(list.getByRole("option")).toHaveText([/Alfred@alfredFollowing/, /Alice@alice$/]);
  return list;
}

const active = (box: Locator) =>
  box.evaluate((el) => document.getElementById(el.getAttribute("aria-activedescendant") ?? "")?.textContent ?? null);

async function axeViolations(page: Page) {
  await page.evaluate(axe.source);
  return page.evaluate(async () => {
    // @ts-expect-error axe is the injected script's global
    const result = await window.axe.run(".compose-modal, .mention-list", { resultTypes: ["violations"] });
    return result.violations.map((v: { id: string; nodes: { target: string[] }[] }) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`);
  });
}

test("at 1280, the keys move through the list, pick, and close it; the cast carries the mentions", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  const { node, searched } = await ready(page);
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  const box = dialog.getByRole("textbox", { name: "What's happening?" });
  await expect(box).toBeFocused();
  await page.keyboard.type("gm @al");
  const list = await listed(dialog);
  // Under the caret, in the window.
  const [boxRect, listRect] = [await box.boundingBox(), await list.boundingBox()];
  expect(listRect!.y).toBeGreaterThan(boxRect!.y);
  await expect(box).toHaveAttribute("aria-controls", "mention-list");
  expect(await active(box)).toContain("Alfred");
  expect(await axeViolations(page)).toEqual([]);
  if (process.env.SHOTS) await page.screenshot({ path: `${process.env.SHOTS}/1280.png` });

  // Escape closes the list, not the composer.
  await page.keyboard.press("Escape");
  await expect(list).toBeHidden();
  await expect(dialog).toBeVisible();
  await expect(box).not.toHaveAttribute("aria-activedescendant");

  // Typed again, Down and Up move, wrapping; Enter picks.
  await page.keyboard.press("Backspace");
  await page.keyboard.press("Backspace");
  await page.keyboard.type("al");
  await listed(dialog);
  await page.keyboard.press("ArrowDown");
  expect(await active(box)).toContain("Alice");
  await page.keyboard.press("ArrowDown");
  expect(await active(box)).toContain("Alfred");
  await page.keyboard.press("ArrowUp");
  expect(await active(box)).toContain("Alice");
  await page.keyboard.press("Enter");
  await expect(list).toBeHidden();
  await expectDraft(box, "gm @alice ");

  // Tab picks, and keeps the focus in the box.
  await page.keyboard.type("and @alf");
  await expect(page.getByRole("listbox", { name: "People to mention" }).getByRole("option")).toHaveText([/Alfred/]);
  await page.keyboard.press("Tab");
  await expectDraft(box, "gm @alice and @alfred ");
  await expect(box).toBeFocused();
  // One search per name, from the Worker's search payload.
  expect(searched.every((q) => ["al", "alf"].includes(q))).toBe(true);

  await dialog.getByRole("button", { name: "Cast", exact: true }).click();
  await expect(dialog).toBeHidden();
  expect(castOf(node.sent[0])).toMatchObject({ text: "gm  and ", mentions: [alice.fid, alfred.fid], positions: [3, 8] });
});

test("at 390, a tap picks from the list in the phone's sheet", async ({ browser }, testInfo) => {
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  const page = await context.newPage();
  const { node } = await ready(page);
  await page.getByRole("button", { name: "Write a cast" }).tap();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  const box = dialog.getByRole("textbox");
  await expect(box).toBeFocused();
  await page.keyboard.type("hi @al");
  const list = await listed(dialog);
  const rect = await list.boundingBox();
  expect(rect!.x).toBeGreaterThanOrEqual(0);
  expect(rect!.x + rect!.width).toBeLessThanOrEqual(390);
  for (const option of await list.getByRole("option").all()) expect((await option.boundingBox())!.height).toBeGreaterThanOrEqual(44);
  expect(await axeViolations(page)).toEqual([]);
  if (process.env.SHOTS) await page.screenshot({ path: `${process.env.SHOTS}/390.png` });
  await list.getByRole("option", { name: /Alice/ }).tap();
  await expect(list).toBeHidden();
  await expectDraft(box, "hi @alice ");
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Cast", exact: true }).tap();
  await expect(dialog).toBeHidden();
  expect(castOf(node.sent[0])).toMatchObject({ text: "hi ", mentions: [alice.fid], positions: [3] });
  await context.close();
});
