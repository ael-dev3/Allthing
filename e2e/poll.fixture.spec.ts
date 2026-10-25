// SPDX-License-Identifier: AGPL-3.0-or-later
import axe from "axe-core";
import { expect, test, type Page, type Request } from "@playwright/test";

/*
 * Allthing's poll on the front page: "Would you like an Allthing mobile
 * client?" Where the screen has room, it sits in the gutter beside the feed
 * and stays in view as the feed scrolls; where it hasn't, it is a card after
 * the feed's fourth. Either way it moves nothing: the feed's column stays
 * where it is, and the card is drawn with the cards around it. Before a vote
 * it shows no numbers. A vote shows the results at once, and this browser
 * keeps them: a later visit shows them without asking. A network that has
 * voted is told so, never how; a vote that doesn't go through gives the
 * buttons back, with why; and where the Worker keeps no votes, the poll goes.
 * The Worker's side: poll-worker.fixture.spec.ts. With POLL_SHOTS=<folder>,
 * screenshots of it there.
 */

const SHOTS = process.env.POLL_SHOTS;
const POLL = "mobile-client-v1";
const NOT_SET_UP = "Polls aren't set up here";

const now = Math.floor(Date.now() / 1000);
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const person = (fid: number) => ({ fid, username: `reader${fid}`, display_name: `Reader ${fid}`, follower_count: 10, following_count: 5, bio: "" });
/** Twenty casts, the busiest and newest first, a few lines each: a feed that scrolls. */
const pool = Array.from({ length: 20 }, (_, i) => ({
  hash: hex(i + 1),
  author: person(i + 1),
  text: `Cast ${i + 1}. A few words about something worth reading, and then a few more, so that the card takes a line or two.`,
  ts: now - 600 - i * 60,
  likes: 400 - i * 10,
  replies: 0,
}));

type Reply = { status?: number; json?: object; abort?: boolean; wait?: Promise<void> };

/** The front page's data, from fixtures; `home` adds to its first page's. */
async function serve(page: Page, home: object = {}) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: later ? { pool: [], notes: {}, meta } : { pool, notes: {}, meta, ...home } });
  });
  await page.route("**/api/v1/latest", (route) => route.fulfill({ json: { pool, notes: {}, cursors: [], meta } }));
}

/** Answers the page's votes with `replies`, in turn; what it sent, in order. */
async function answer(page: Page, replies: Reply[]) {
  const sent: unknown[] = [];
  await page.route("**/api/v1/poll", async (route) => {
    sent.push(route.request().postDataJSON());
    const reply = replies.shift() ?? { status: 500, json: { error: "No answer left" } };
    if (reply.wait) await reply.wait;
    if (reply.abort) return route.abort("failed");
    return route.fulfill({ status: reply.status ?? 200, json: reply.json, headers: { "cache-control": "no-store" } });
  });
  return sent;
}

/** A promise, and its resolve. */
function held() {
  let release = () => {};
  const wait = new Promise<void>((done) => (release = done));
  return { wait, release };
}

/** Layout shifts from the page's start, but those just after an input, with what moved. */
async function watchShifts(page: Page) {
  await page.addInitScript(() => {
    const shifts: { value: number; moved: string[] }[] = [];
    Object.assign(window, { pollShifts: shifts });
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries() as (PerformanceEntry & { value: number; hadRecentInput: boolean; sources?: { node?: Node; previousRect: DOMRectReadOnly; currentRect: DOMRectReadOnly }[] })[]) {
        if (entry.hadRecentInput) continue;
        const moved = (entry.sources ?? []).map((s) => `${(s.node as Element | undefined)?.className ?? s.node?.nodeName} ${Math.round(s.previousRect.y)}→${Math.round(s.currentRect.y)}`);
        shifts.push({ value: entry.value, moved });
      }
    }).observe({ type: "layout-shift", buffered: true });
  });
}

/** The page's cumulative layout shift so far, and what moved. */
const layoutShift = (page: Page) =>
  page.evaluate(() => {
    const shifts = (window as unknown as { pollShifts: { value: number; moved: string[] }[] }).pollShifts;
    return { total: shifts.reduce((sum, s) => sum + s.value, 0), moved: shifts.flatMap((s) => s.moved) };
  });

/** Once the page's entrances have run: boxes measured during one are moved by its transform. */
const settle = (page: Page) =>
  page.evaluate(() =>
    Promise.all(
      document
        .getAnimations()
        .filter((a) => a.effect?.getComputedTiming().endTime !== Infinity)
        .map((a) => a.finished.catch(() => undefined)),
    ),
  );

/** Accessibility problems axe finds in the poll's card. */
async function axeViolations(page: Page) {
  await settle(page);
  await page.evaluate(axe.source);
  return page.evaluate(async () => {
    // @ts-expect-error axe is the injected script's global
    const result = await window.axe.run(".poll-card", { resultTypes: ["violations"] });
    return result.violations.map((v: { id: string; nodes: { target: string[] }[] }) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`);
  });
}

/** With POLL_SHOTS, a screenshot named `name`, of the poll in the middle of the window if it shows. */
async function shot(page: Page, name: string) {
  if (!SHOTS) return;
  if (await poll(page).count()) await poll(page).evaluate((card) => card.scrollIntoView({ block: "center" }));
  await settle(page);
  await page.evaluate(() => document.fonts.ready);
  await page.screenshot({ path: `${SHOTS}/${name}.png` });
}

/** The poll where it shows at this width: beside the feed, or in it. */
const poll = (page: Page) => page.locator(".poll-card:visible");
/** The feed's list (not its placeholders'). */
const list = (page: Page) => page.locator("#app .cast-list:not([aria-busy])");

/** What the list's first six places hold: casts' cards, and the poll. */
const order = (page: Page) =>
  list(page).evaluate((list) =>
    [...list.children].slice(0, 6).map((child) => (child.matches(".poll-card") ? "poll" : child.matches(".cast-card") ? "cast" : child.tagName)),
  );
/** Its first six cards are drawn (and the rest follow, a screenful at a time). */
const drawn = (page: Page) => expect(list(page).locator(".cast-card").nth(5)).toBeVisible();

async function open(page: Page, width: number) {
  await page.setViewportSize({ width, height: width < 600 ? 844 : 900 });
  await page.goto("/");
  await drawn(page);
  await expect(poll(page)).toBeVisible();
}

test.describe("the poll", () => {
  test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no poll");

  test("beside the feed at 1280 px, it moves nothing, keeps out of the feed's way and stays in view as the feed scrolls", async ({ page }) => {
    await watchShifts(page);
    await serve(page);
    await open(page, 1280);
    await settle(page);
    // In the gutter; the card in the feed is there for narrower screens only.
    await expect(page.locator(".poll-gutter .poll-card")).toBeVisible();
    await expect(list(page).locator(".poll-card")).toBeHidden();
    expect(await order(page)).toEqual(["cast", "cast", "cast", "cast", "poll", "cast"]);
    // The feed's column is where it always is: 770 px, in the middle.
    const middle = await page.evaluate(() => document.documentElement.clientWidth / 2);
    const feed = (await list(page).boundingBox())!;
    expect(feed.width).toBe(770);
    expect(Math.abs(feed.x + feed.width / 2 - middle)).toBeLessThan(1);
    // Beside it, clear of it and of the window's edge, level with its top.
    const card = (await poll(page).boundingBox())!;
    expect(card.x).toBeGreaterThanOrEqual(feed.x + feed.width + 16);
    expect(card.x + card.width).toBeLessThanOrEqual(middle * 2 - 8);
    expect(card.width).toBeGreaterThanOrEqual(200);
    const column = (await page.locator("#app .front-columns > .content").boundingBox())!;
    expect(Math.abs(card.y - column.y)).toBeLessThan(1);
    // Its buttons, once its code has come, are big enough to hit.
    await expect(poll(page).getByRole("button")).toHaveCount(2);
    for (const button of await poll(page).getByRole("button").all()) expect((await button.boundingBox())!.height).toBeGreaterThanOrEqual(44);
    // Down the feed, it stays in view under the header.
    await page.evaluate(() => window.scrollTo(0, 1600));
    await expect.poll(async () => Math.round((await poll(page).boundingBox())!.y)).toBe(78);
    const scrolled = (await poll(page).boundingBox())!;
    expect(scrolled.x).toBe(card.x);
    expect(scrolled.y + scrolled.height).toBeLessThan(900);
    const { total, moved } = await layoutShift(page);
    expect(total, moved.join("; ")).toBeLessThan(0.01);
  });

  test("at 390 px it is a card after the feed's fourth, drawn with them, and keeps its place as the feed is sorted and its range changed", async ({ page }) => {
    await watchShifts(page);
    await serve(page);
    await open(page, 390);
    await settle(page);
    await expect(page.locator(".poll-gutter .poll-card")).toBeHidden();
    expect(await order(page)).toEqual(["cast", "cast", "cast", "cast", "poll", "cast"]);
    // As wide as the cards, and its buttons are big enough to tap.
    const [card, cast] = [(await poll(page).boundingBox())!, (await list(page).locator(".cast-card").first().boundingBox())!];
    expect([card.x, card.width]).toEqual([cast.x, cast.width]);
    await expect(poll(page).getByRole("button")).toHaveCount(2);
    for (const button of await poll(page).getByRole("button").all()) expect((await button.boundingBox())!.height).toBeGreaterThanOrEqual(44);
    const { total, moved } = await layoutShift(page);
    expect(total, moved.join("; ")).toBeLessThan(0.01);
    // Top, then the past week, then new: still after the fourth card.
    const sort = page.getByRole("navigation", { name: "Sort", exact: true });
    await sort.getByRole("link", { name: "top", exact: true }).click();
    await expect(page).toHaveURL(/sort=top/);
    await expect.poll(() => order(page)).toEqual(["cast", "cast", "cast", "cast", "poll", "cast"]);
    await page.getByRole("combobox", { name: "Time range" }).selectOption("week");
    await expect(page).toHaveURL(/t=week/);
    await drawn(page);
    await expect.poll(() => order(page)).toEqual(["cast", "cast", "cast", "cast", "poll", "cast"]);
    await sort.getByRole("link", { name: "new", exact: true }).click();
    await expect(page).toHaveURL(/sort=new/);
    await drawn(page);
    await expect.poll(() => order(page)).toEqual(["cast", "cast", "cast", "cast", "poll", "cast"]);
  });

  test("its frame comes with the feed's cards, and its body once its code has come, at idle, moving nothing", async ({ page }) => {
    await watchShifts(page);
    await serve(page);
    const fetched: string[] = [];
    page.on("request", (request) => {
      if (request.url().endsWith("/split_poll.wasm")) fetched.push(request.url());
    });
    const part = held();
    await page.route("**/split_poll.wasm", async (route) => {
      await part.wait;
      await route.continue();
    });
    await open(page, 390);
    const card = poll(page);
    // The frame, with its question and its body's box, before the code: asked for once the page is idle.
    await expect(card.getByRole("heading", { name: "Would you like an Allthing mobile client?" })).toBeVisible();
    await expect.poll(() => fetched.length).toBe(1);
    await expect(card.getByRole("button")).toHaveCount(0);
    await settle(page);
    await shot(page, "poll-frame-390");
    const fifth = list(page).locator(".cast-card").nth(4);
    const before = [await card.boundingBox(), await fifth.boundingBox()];
    // The code: Yes and No in the box, and nothing moved.
    part.release();
    await expect(card.getByRole("button", { name: "Yes" })).toBeVisible();
    expect([await card.boundingBox(), await fifth.boundingBox()]).toEqual(before);
    const { total, moved } = await layoutShift(page);
    expect(total, moved.join("; ")).toBeLessThan(0.01);
    // The card beside the feed fills from the same code, fetched once.
    await page.setViewportSize({ width: 1280, height: 900 });
    await expect(poll(page).getByRole("button", { name: "Yes" })).toBeVisible();
    expect(fetched).toHaveLength(1);
  });

  for (const width of [1280, 390]) {
    test(`a vote shows the results at once, which a later visit shows with no request, at ${width} px`, async ({ page }) => {
      await watchShifts(page);
      await serve(page);
      const answered = held();
      const sent = await answer(page, [{ json: { counted: true, counts: { yes: 3, no: 1 } }, wait: answered.wait }]);
      await open(page, width);
      const card = poll(page);
      const [yes, no] = [card.getByRole("button", { name: "Yes" }), card.getByRole("button", { name: "No" })];
      // Before a vote: the question and its two answers, and no numbers.
      await expect(card.getByRole("heading", { name: "Would you like an Allthing mobile client?" })).toBeVisible();
      await expect(card).toContainText("Allthing poll");
      await expect(yes).toHaveAttribute("aria-pressed", "false");
      await expect(yes).toBeEnabled();
      expect(await card.innerText()).not.toMatch(/\d/);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `poll-unvoted-${width}`);
      const height = (await card.boundingBox())!.height;

      // Yes: pressed at once, and both wait for the Worker.
      await yes.click();
      await expect(yes).toHaveAttribute("aria-pressed", "true");
      await expect(no).toHaveAttribute("aria-pressed", "false");
      await expect(yes).toBeDisabled();
      await expect(no).toBeDisabled();
      await expect.poll(() => sent).toEqual([{ poll: POLL, choice: "yes" }]);
      answered.release();

      // Counted: how many voted each way, and the reader's vote, said to
      // screen readers too, in a card of the same height.
      await expect(card.locator(".poll-result")).toHaveText(["Yes 75% (3)", "No 25% (1)"]);
      await expect(card.locator(".poll-total")).toHaveText("4 votes");
      await expect(card.locator(".poll-note")).toHaveText("You voted Yes.");
      await expect(card.locator(".poll-result.mine")).toHaveText("Yes 75% (3)");
      await expect(card.getByRole("status")).toHaveText("Yes 75% (3), No 25% (1), 4 votes. You voted Yes.");
      await expect(card.getByRole("button")).toHaveCount(0);
      expect((await card.boundingBox())!.height).toBe(height);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `poll-voted-${width}`);
      const kept = await page.evaluate(() => JSON.parse(localStorage.getItem("allthing-poll") ?? "null"));
      expect(kept).toMatchObject({ poll: POLL, choice: "yes", counts: { yes: 3, no: 1 } });
      expect(kept.at).toBeGreaterThan(Date.now() - 60_000);
      const { total, moved } = await layoutShift(page);
      expect(total, moved.join("; ")).toBeLessThan(0.01);

      // A later visit: the results, without asking.
      const asked: string[] = [];
      page.on("request", (request) => {
        if (new URL(request.url()).pathname === "/api/v1/poll") asked.push(request.method());
      });
      await page.reload();
      await drawn(page);
      await expect(poll(page).locator(".poll-result")).toHaveText(["Yes 75% (3)", "No 25% (1)"]);
      await expect(poll(page).locator(".poll-note")).toHaveText("You voted Yes.");
      await page.waitForTimeout(500);
      expect(asked).toEqual([]);
      expect(sent).toHaveLength(1);
    });

    test(`a network that has voted is told so, never how, at ${width} px`, async ({ page }) => {
      await serve(page);
      const sent = await answer(page, [{ json: { counted: false, already: true, counts: { yes: 7, no: 2 } } }]);
      await open(page, width);
      const card = poll(page);
      await card.getByRole("button", { name: "No" }).click();
      await expect(card.locator(".poll-result")).toHaveText(["Yes 78% (7)", "No 22% (2)"]);
      await expect(card.locator(".poll-total")).toHaveText("9 votes");
      await expect(card.locator(".poll-note")).toHaveText("A vote from your network was already counted.");
      await expect(card.getByRole("status")).toHaveText("Yes 78% (7), No 22% (2), 9 votes. A vote from your network was already counted.");
      // Neither answer is marked as the reader's, and nothing says how the network voted.
      await expect(card.locator(".poll-result.mine")).toHaveCount(0);
      await expect(card).not.toContainText("You voted");
      expect(sent).toEqual([{ poll: POLL, choice: "no" }]);
      const kept = await page.evaluate(() => JSON.parse(localStorage.getItem("allthing-poll") ?? "null"));
      expect(kept).toMatchObject({ poll: POLL, counts: { yes: 7, no: 2 } });
      expect(kept.choice ?? null).toBeNull();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `poll-already-${width}`);
    });
  }

  test("a vote that doesn't go through gives the buttons back, with why, until one does", async ({ page }) => {
    await serve(page);
    const sent = await answer(page, [
      { abort: true },
      { status: 503, json: { error: "Votes can't be counted just now; try again soon", retry_after: 30 } },
      { status: 500, json: { error: "Internal error" } },
      { json: { counted: true, counts: { yes: 0, no: 1 } } },
    ]);
    await open(page, 1280);
    const card = poll(page);
    const [yes, no] = [card.getByRole("button", { name: "Yes" }), card.getByRole("button", { name: "No" })];
    const height = (await card.boundingBox())!.height;
    for (const why of ["Allthing couldn't be reached. Try again.", "Votes can't be counted just now; try again soon", "Your vote didn't go through. Try again."]) {
      await no.click();
      await expect(card.locator(".poll-note")).toHaveText(why);
      await expect(card.getByRole("status")).toHaveText(why);
      await expect(no).toBeEnabled();
      await expect(yes).toBeEnabled();
      await expect(no).toHaveAttribute("aria-pressed", "false");
      await expect(card.locator(".poll-result")).toHaveCount(0);
      expect((await card.boundingBox())!.height).toBe(height);
      expect(await page.evaluate(() => localStorage.getItem("allthing-poll"))).toBeNull();
    }
    await no.click();
    await expect(card.locator(".poll-result")).toHaveText(["Yes 0% (0)", "No 100% (1)"]);
    await expect(card.locator(".poll-note")).toHaveText("You voted No.");
    expect(sent).toHaveLength(4);
  });

  test("where the Worker keeps no votes, the poll goes from both its places", async ({ page }) => {
    await serve(page);
    await answer(page, [{ status: 503, json: { error: NOT_SET_UP } }]);
    await open(page, 390);
    await poll(page).getByRole("button", { name: "Yes" }).click();
    await expect(page.locator(".poll-card")).toHaveCount(0);
    expect(await order(page)).toEqual(["cast", "cast", "cast", "cast", "cast", "cast"]);
    // As the front page was before the poll.
    await page.evaluate(() => window.scrollTo(0, 0));
    await shot(page, "poll-none-390");
    await page.setViewportSize({ width: 1280, height: 900 });
    await expect(page.locator(".poll-card")).toHaveCount(0);
    await shot(page, "poll-none-1280");
    expect(await page.evaluate(() => localStorage.getItem("allthing-poll"))).toBeNull();
  });

  test("the front page's newer counts show in place of the reader's own; older ones don't", async ({ page }) => {
    const earlier = Date.now() - 3_600_000;
    // The reader's vote, an hour ago (on the first visit only: the reload below sets its own).
    await page.addInitScript((kept) => {
      if (sessionStorage.getItem("seeded")) return;
      sessionStorage.setItem("seeded", "1");
      localStorage.setItem("allthing-poll", JSON.stringify(kept));
    }, { poll: POLL, choice: "no", counts: { yes: 1, no: 1 }, at: earlier });
    // Newer: counted after the reader's vote.
    await serve(page, { poll: { yes: 5, no: 3 } });
    await open(page, 1280);
    await expect(poll(page).locator(".poll-result")).toHaveText(["Yes 63% (5)", "No 37% (3)"]);
    await expect(poll(page).locator(".poll-total")).toHaveText("8 votes");
    await expect(poll(page).locator(".poll-note")).toHaveText("You voted No.");
    expect(await page.evaluate(() => JSON.parse(localStorage.getItem("allthing-poll")!))).toMatchObject({ choice: "no", counts: { yes: 5, no: 3 }, at: meta.fetched_at });

    // Older: a cached page from before the reader's vote leaves it as it is.
    await page.evaluate((kept) => localStorage.setItem("allthing-poll", JSON.stringify(kept)), { poll: POLL, choice: "yes", counts: { yes: 9, no: 1 }, at: Date.now() + 60_000 });
    await page.reload();
    await drawn(page);
    await expect(poll(page).locator(".poll-result")).toHaveText(["Yes 90% (9)", "No 10% (1)"]);
    await expect(poll(page).locator(".poll-note")).toHaveText("You voted Yes.");
  });
});

for (const width of [1280, 390]) {
  test(`the static copy, which has no Worker to vote with, has no poll, at ${width} px`, async ({ page }, info) => {
    test.skip(!process.env.E2E_PAGES, "the static copy, under /Allthing");
    // Keep transport/startup evidence if this fails before the app mounts.
    // Never retain bodies, console arguments, cookies or URL queries.
    const started = Date.now();
    const safeURL = (raw: string) => {
      try {
        const url = new URL(raw);
        return /^https?:$/.test(url.protocol) ? `${url.origin}${url.pathname}`.slice(0, 240) : url.protocol;
      } catch { return "unparseable URL"; }
    };
    const categories = (message: string) => ["wasm", "compile", "instantiate", "import", "fetch", "network", "cors", "csp", "mime", "panic", "unreachable", "memory", "timeout", "abort"].filter(word => message.toLowerCase().includes(word));
    const events: object[] = [];
    let dropped = 0;
    const record = (event: object) => {
      if (events.length === 64) { events.shift(); dropped++; }
      events.push({ at: Date.now() - started, ...event });
    };
    const ids = new WeakMap<Request, number>();
    let nextId = 0;
    const id = (request: Request) => {
      if (!ids.has(request)) ids.set(request, ++nextId);
      return ids.get(request)!;
    };
    const pending = new Map<Request, object>();
    let pendingTotal = 0;
    page.on("request", request => {
      const entry = { id: id(request), method: request.method(), url: safeURL(request.url()) };
      pendingTotal++;
      if (pending.size < 32) pending.set(request, entry);
      record({ kind: "request", ...entry });
    });
    page.on("response", response => {
      const headers = response.headers();
      record({ kind: "response", id: id(response.request()), status: response.status(), contentType: headers["content-type"]?.slice(0, 100), contentLength: /^\d+$/.test(headers["content-length"] ?? "") ? Number(headers["content-length"]) : undefined });
    });
    page.on("requestfinished", request => {
      pendingTotal--;
      pending.delete(request);
      const timing = request.timing();
      record({ kind: "finished", id: id(request), requestStart: timing.requestStart, responseStart: timing.responseStart, responseEnd: timing.responseEnd });
    });
    page.on("requestfailed", request => {
      pendingTotal--;
      pending.delete(request);
      record({ kind: "failed", id: id(request), code: request.failure()?.errorText.match(/net::ERR_[A-Z_]+/)?.[0] ?? "unclassified network failure" });
    });
    page.on("pageerror", error => record({ kind: "pageerror", name: ["Error", "TypeError", "ReferenceError", "SyntaxError", "RangeError", "CompileError", "LinkError", "RuntimeError"].includes(error.name) ? error.name : "Error", categories: categories(error.message), scripts: [...(error.stack ?? "").split("\n").filter(line => /^\s*at /.test(line)).join("\n").matchAll(/https?:\/\/[^\s)]+/g)].slice(0, 4).map(match => safeURL(match[0])) }));
    page.on("console", message => {
      const location = message.location();
      record({ kind: "console", type: message.type(), categories: categories(message.text()), url: safeURL(location.url), line: location.lineNumber, column: location.columnNumber });
    });
    try {
      await page.setViewportSize({ width, height: 844 });
      // Its trending casts, from Hypersnap in the browser: more than four.
      await page.route("https://haatz.quilibrium.com/**", (route) => {
        const url = new URL(route.request().url());
        const casts = url.pathname.endsWith("/feed/trending") && !url.searchParams.has("cursor")
          ? pool.slice(0, 8).map((cast) => ({
              hash: cast.hash, author: cast.author, text: cast.text, timestamp: new Date(cast.ts * 1000).toISOString(),
              embeds: [], replies: { count: 0 }, reactions: { likes_count: cast.likes, recasts_count: 0 },
            }))
          : null;
        return casts
          ? route.fulfill({ json: { casts }, headers: { "access-control-allow-origin": "*" } })
          : route.fulfill({ status: 404, json: {}, headers: { "access-control-allow-origin": "*" } });
      });
      await page.goto("/Allthing/");
      await expect(page.locator("#app .cast-card:not(.skeleton)").nth(5)).toBeVisible();
      await expect(page.locator(".poll-card, .poll-gutter")).toHaveCount(0);
    } catch (error) {
      // Diagnostic failure must not replace the original assertion failure.
      try {
        const state = await Promise.race([page.evaluate(() => {
          const safe = (raw: string) => {
            try { const url = new URL(raw); return /^https?:$/.test(url.protocol) ? `${url.origin}${url.pathname}`.slice(0, 240) : url.protocol; }
            catch { return "unparseable URL"; }
          };
          const boot = document.getElementById("boot"), app = document.getElementById("app");
          return {
            url: safe(location.href), readyState: document.readyState, visibility: document.visibilityState, online: navigator.onLine,
            boot: { present: Boolean(boot), painted: boot?.hasAttribute("data-painted"), hidden: boot?.getAttribute("aria-hidden") },
            app: { present: Boolean(app), children: app?.childElementCount, cards: app?.querySelectorAll(".cast-card:not(.skeleton)").length },
            resources: (performance.getEntriesByType("resource") as PerformanceResourceTiming[]).slice(-48).map(entry => ({
              url: safe(entry.name), initiator: entry.initiatorType, start: entry.startTime, duration: entry.duration,
              responseStart: entry.responseStart, responseEnd: entry.responseEnd, transferSize: entry.transferSize, encodedBodySize: entry.encodedBodySize,
            })),
          };
        }).catch(() => ({ unavailable: true })), new Promise(resolve => setTimeout(() => resolve({ diagnosticTimeout: true }), 1000))]);
        await info.attach("static-startup-diagnostics", { contentType: "application/json", body: Buffer.from(JSON.stringify({ elapsed: Date.now() - started, dropped, events, pendingTotal, pendingEntriesTruncated: pendingTotal > pending.size, pending: [...pending.values()], state })) });
      } catch { /* The original failure remains authoritative. */ }
      throw error;
    }
  });
}
