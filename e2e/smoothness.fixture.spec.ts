// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type CDPSession, type Locator, type Page } from "@playwright/test";
import { readFileSync, writeFileSync } from "node:fs";
import { cpus, loadavg } from "node:os";
import { appCame } from "./helpers/app";
import { approvedKey, signIn } from "./helpers/farcaster";
import { standIn } from "./helpers/node";

/*
 * Smoothness (docs/PERFORMANCE.md#smoothness): how the app answers a
 * reader's taps, clicks and scrolls, on a phone (390 px, touch, the CPU
 * slowed 2×) and on a computer (1280 px, a mouse), on the local build with
 * the Worker's API, the node and the images as stand-ins. Signed in, with
 * an approved key, the reader:
 *   - taps the sort tabs (new, top, hot);
 *   - opens a likes count (a tap previews who liked; a click opens the list);
 *   - likes a cast, unlikes it, likes it again;
 *   - opens the composer, then in it the emoji picker and the sticker
 *     picker, and types a key in a draft;
 *   - opens a cast from the feed, and a profile, at three depths, and goes Back;
 *   - goes Back to the feed's second server page;
 *   - scrolls the feed;
 *   - presses 22 controls on the front page, the composer, a post and a
 *     profile, letting go elsewhere.
 *
 * Each interaction runs three times, and twice more when the median is
 * within a fifth of a timed budget (so five runs, not one noisy one, decide
 * a close call); each budget holds for the median:
 *   - INP (Event Timing: an interaction's slowest event) < 100 ms;
 *   - no long task or long animation frame (> 50 ms) overlapping the
 *     interaction, from its event until its handlers are done;
 *   - nothing moves after a navigation's first frame (layout shift < 0.01);
 *   - no blank frame: no frame of a navigation in which the page's main
 *     content is empty or too faint to read (rAF samples it each frame), or
 *     the page is white;
 *   - Back puts the card the reader was at within 2 px of where it was;
 *   - a press changes the pressed control's transform or opacity in the
 *     first frame after pointerdown, and moves nothing;
 *   - a scroll keeps its frames: the 95th percentile of their intervals at
 *     most 20 ms, and at most 5% of them dropped.
 *
 * A budget the app doesn't meet yet is an expected failure (`NOT_YET`), or,
 * where the app misses or meets it by less than a busy machine's noise, not
 * run (`TOO_CLOSE`), each with what was measured; a change that meets one
 * takes its line out.
 *
 * The timed budgets hold on a machine whose CPU the run has to itself, as a
 * CI runner's is. Between interactions the spec times how much of a CPU a
 * busy thread gets: away from CI, below `QUIET_SHARE` every number is
 * inflated by the machine, not the app, so those budgets are skipped, saying
 * why (the numbers are still reported).
 *
 *   E2E_PORT=8815 npx playwright test e2e/smoothness.fixture.spec.ts
 *   SMOOTH_OUT=numbers.json …    # every run's numbers, by profile
 *   SMOOTH_TRACE=trace.zip …     # a Playwright trace of the phone's run
 *   SMOOTH_CPU=4 …               # another slowdown for the phone
 *   SMOOTH_ONLY=390 …            # one profile
 *   SMOOTH_STRICT=1 …            # judge the timed budgets on a busy machine too
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract and sign-in");

/** How much slower the phone's CPU is than the test machine's: what the budgets were calibrated at (docs/PERFORMANCE.md#smoothness). */
const CALIBRATED_CPU = 2;
const PHONE_CPU = Number(process.env.SMOOTH_CPU ?? CALIBRATED_CPU);
const RUNS = 3;

const INP_BUDGET = 100;
const LONG_TASK = 50;
const SHIFT_BUDGET = 0.01;
const RESTORE_BUDGET_PX = 2;
/** The scroll's frames: the 95th percentile of their intervals, and the share dropped (median of the runs). */
const SCROLL_P95_MS = 20;
const SCROLL_DROPPED = 0.05;

/**
 * The least of a CPU a busy thread may get, in the median of the run's
 * samples, for the timed budgets to be judged: below it the browser's main
 * thread waits its turn a tenth of its time or more, and every latency
 * grows by as much, enough to take the slowest interaction that meets its
 * budget (88 ms, docs/PERFORMANCE.md#smoothness) over it. In CI, or with
 * SMOOTH_STRICT, they're judged however busy the machine is.
 */
const QUIET_SHARE = 0.9;
const STRICT = Boolean(process.env.CI || process.env.SMOOTH_STRICT);

type Profile = { name: string; width: number; height: number; touch: boolean; cpu: number };
const PHONE: Profile = { name: "390", width: 390, height: 844, touch: true, cpu: PHONE_CPU };
const DESKTOP: Profile = { name: "1280", width: 1280, height: 900, touch: false, cpu: 1 };

// ---------------------------------------------------------------- fixtures

const now = Math.floor(Date.now() / 1000);
const meta = { backend: "hypersnap", fetched_at: Date.now() };
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const people = Array.from({ length: 12 }, (_, i) => ({
  fid: 100 + i,
  username: `writer${i}`,
  display_name: `Writer ${i}`,
  pfp_url: `https://imagedelivery.net/fixture/pfp-${i}/original`,
}));
const WORDS =
  "the quiet morning light over the harbour reading notes from friends about tea books trains code music gardens rain small things done well and why it matters to keep going slowly".split(
    " ",
  );
/** Deterministic text of `sentences` sentences for cast `n`. */
function text(n: number, sentences: number) {
  const out: string[] = [];
  let seed = n * 7919 + 13;
  for (let s = 0; s < sentences; s++) {
    const words: string[] = [];
    const length = 6 + (seed % 11);
    for (let w = 0; w < length; w++) {
      seed = (seed * 1103515245 + 12345) % 2147483648;
      words.push(WORDS[seed % WORDS.length]);
    }
    out.push(words.join(" ").replace(/^./, (c) => c.toUpperCase()) + ".");
  }
  return out.join(" ");
}

type FixtureCast = Record<string, unknown> & { hash: string; author: (typeof people)[number] | typeof me };

/** A feed cast: text of one to five sentences, and every few casts a photo or a link card. */
function cast(n: number, ts: number, author = people[n % people.length]): FixtureCast {
  const embeds =
    n % 4 === 1
      ? [{ kind: "image", url: `https://imagedelivery.net/fixture/photo-${n}/original` }]
      : n % 7 === 3
        ? [{ kind: "link", url: `https://example.org/notes/${n}`, title: `Notes, part ${n}`, image_url: `https://imagedelivery.net/fixture/thumb-${n}/original` }]
        : [];
  return {
    hash: hex(n),
    author,
    text: text(n, 1 + (n % 5)),
    ts,
    likes: ((n * 37) % 90) + 3,
    recasts: (n * 11) % 20,
    replies: ((n * 5) % 12) + 1,
    ...(n % 3 === 0 ? { channel: { id: "books", label: "books" } } : {}),
    embeds,
  };
}

const home = Array.from({ length: 60 }, (_, i) => cast(i + 1, now - 300 - i * 420));
const later = Array.from({ length: 40 }, (_, i) => cast(200 + i, now - 30_000 - i * 600));
const latest = Array.from({ length: 30 }, (_, i) => cast(500 + i, now - 20 - i * 30));
const all = new Map([...home, ...later, ...latest].map((c) => [c.hash, c]));
const replies = (parent: FixtureCast) =>
  Array.from({ length: 6 }, (_, i) => ({
    cast: { ...cast(9000 + Number(BigInt(parent.hash) % 1000n) * 10 + i, now - 100 + i, people[(i + 3) % people.length]), parent_hash: parent.hash, embeds: [] },
    children: [],
  }));
const profile = (person: (typeof people)[number]) => ({
  user: {
    ...person,
    bio: text(person.fid, 2),
    follower_count: 1200 + person.fid,
    following_count: 300,
    registered_at: now - 400 * 86_400,
  },
  pool: Array.from({ length: 25 }, (_, i) => cast(20_000 + person.fid * 100 + i, now - 900 - i * 3600, person)),
  notes: {},
  meta,
});
/** Who liked or recasted a cast: `count` people. */
const reactors = (count: number) =>
  Array.from({ length: count }, (_, i) => ({ ...people[i % people.length], fid: 300 + i, username: `fan${i}`, display_name: `Fan ${i}` }));

const svg = (w: number, h: number, fill = "#9aa6c2") =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}"><rect width="${w}" height="${h}" fill="${fill}"/></svg>`;

const NODE = "https://haatz.quilibrium.com/v1";
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" };
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** Network time for the API's answers: what a page waits for when nothing was fetched ahead. */
const LATENCY = { feed: 60, page: 180, list: 90 };

/** The Worker, the node and the image hosts, as stand-ins. */
async function serve(page: Page) {
  const json = async (route: Parameters<Parameters<Page["route"]>[1]>[0], body: object, latency: number, status = 200) => {
    await wait(latency);
    return route.fulfill({ status, json: body }).catch(() => {});
  };
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const offset = Number(new URL(route.request().url()).searchParams.get("offset") ?? 0);
    return json(route, { pool: offset === 0 ? home : offset === 100 ? later : [], notes: {}, meta }, LATENCY.feed);
  });
  await page.route("**/api/v1/latest", (route) => json(route, { pool: latest, notes: {}, cursors: [], meta }, LATENCY.feed));
  await page.route("**/api/v1/cast?*", (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash")!;
    const found = all.get(hash) ?? cast(1, now);
    return json(route, { thread: { cast: found, children: replies(found) }, notes: {}, meta }, LATENCY.page);
  });
  await page.route("**/api/v1/user?*", (route) => {
    const name = new URL(route.request().url()).searchParams.get("name");
    const person = people.find((p) => p.username === name);
    return person ? json(route, profile(person), LATENCY.page) : json(route, { error: "No such account" }, LATENCY.page, 404);
  });
  await page.route("**/api/v1/reactions?*", (route) => {
    const url = new URL(route.request().url());
    // A preview's ten, and more; or the whole list, of twenty.
    const preview = Number(url.searchParams.get("limit") ?? 10) <= 10;
    return json(route, { hash: url.searchParams.get("hash"), kind: url.searchParams.get("type"), users: reactors(preview ? 10 : 20), more: preview, meta }, LATENCY.list);
  });
  await page.route("**/api/v1/engagement?*", (route) => {
    const url = new URL(route.request().url());
    const list = (kind: string) => (url.searchParams.get(kind) ?? "").split(",").filter(Boolean);
    const previews = ["likes", "recasts"].flatMap((kind) => list(kind).map((hash) => ({ hash, kind, users: reactors(3), more: true, meta })));
    return json(route, { previews, meta }, LATENCY.list);
  });
  await page.route("**/api/v1/replies?*", (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash")!;
    const found = all.get(hash) ?? cast(1, now);
    return json(route, { hash, replies: replies(found).map((r) => r.cast), more: false, meta }, LATENCY.list);
  });
  await page.route("**/api/v1/signer", (route) => route.fulfill({ json: { app_fid: 3356318 } }));
  await page.route("**/api/v1/refresh", (route) => route.fulfill({ json: { expired: 1 } }));
  // The node: what the reader liked (nothing yet), and their likes going out.
  await page.route(`${NODE}/**`, (route) =>
    route.request().method() === "OPTIONS"
      ? route.fulfill({ status: 204, headers: cors })
      : route.fulfill({ status: 400, headers: cors, json: { error: "Not found", error_detail: 'status: NotFound, message: "not found"' } }),
  );
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  // Each like as the node takes it, and shows it read back.
  const node = await standIn(page, { fid: me.fid });
  await page.route("https://imagedelivery.net/**", (route) => {
    const url = route.request().url();
    const body = url.includes("/pfp-") ? svg(64, 64, "#c9b8f0") : url.includes("/thumb-") ? svg(140, 140) : svg(720, 480);
    return route.fulfill({ body, contentType: "image/svg+xml", headers: { "access-control-allow-origin": "*", "cache-control": "max-age=3600" } });
  });
  await page.route("https://example.org/**", (route) => route.fulfill({ status: 404, body: "" }));
  return {
    get submitted() {
      return node.sent.length;
    },
  };
}

// ------------------------------------------------------- in-page harness

type Sample = {
  label: string;
  t0: number;
  t1: number;
  events: { id: number; name: string; start: number; dur: number; processed: number }[];
  longtasks: { start: number; dur: number }[];
  loafs: { start: number; dur: number; blocking: number; scripts: { invoker: string; dur: number; fn: string; src: string }[] }[];
  shifts: { start: number; value: number; recent: boolean; sources: string[] }[];
  frames: { t: number; blank: boolean; skeleton: boolean; placeholder: boolean; white: boolean; anchor: number | null; scrollY: number; path: string }[];
};

/**
 * Installed before the app: observers for Event Timing, long tasks, long
 * animation frames and layout shifts, a frame sampler, and the press probe.
 */
function harness() {
  type Any = any; // eslint-disable-line @typescript-eslint/no-explicit-any
  const w = window as Any;
  if (w.__smooth) return;
  const S: Any = (w.__smooth = { events: [], longtasks: [], loafs: [], shifts: [], run: null });
  const observe = (type: string, each: (e: Any) => void, extra: object = {}) => {
    try {
      new PerformanceObserver((list) => list.getEntries().forEach(each)).observe({ type, buffered: true, ...extra } as PerformanceObserverInit);
    } catch {
      /* not supported here */
    }
  };
  observe(
    "event",
    (e) => e.interactionId && S.events.push({ id: e.interactionId, name: e.name, start: e.startTime, dur: e.duration, processed: e.processingEnd }),
    { durationThreshold: 16 },
  );
  observe("longtask", (e) => S.longtasks.push({ start: e.startTime, dur: e.duration }));
  observe("long-animation-frame", (e) =>
    S.loafs.push({
      start: e.startTime,
      dur: e.duration,
      blocking: e.blockingDuration,
      scripts: (e.scripts ?? []).slice(0, 4).map((s: Any) => ({ invoker: String(s.invoker).slice(0, 80), dur: Math.round(s.duration), fn: s.sourceFunctionName, src: String(s.sourceURL).split("/").pop() })),
    }),
  );
  const describe = (n: Any) =>
    n?.nodeType === 1 ? `${n.tagName.toLowerCase()}${n.id ? `#${n.id}` : ""}${n.classList.length ? `.${[...n.classList].slice(0, 2).join(".")}` : ""}${n.dataset?.hash ? `[${n.dataset.hash.slice(-4)}]` : ""}` : String(n?.nodeName);
  observe("layout-shift", (e) =>
    S.shifts.push({
      start: e.startTime,
      value: e.value,
      recent: e.hadRecentInput,
      sources: (e.sources ?? []).slice(0, 3).map((s: Any) => `${describe(s.node)} ${Math.round(s.previousRect.y)}→${Math.round(s.currentRect.y)}`),
    }),
  );

  const white = (c: string) => /^rgba?\(255, 255, 255(, 1)?\)$/.test(c);
  const clear = (c: string) => c === "transparent" || /^rgba\(\d+, \d+, \d+, 0\)$/.test(c);
  /** What a frame shows: the main content (any of a grid of points on it that is readable, not a placeholder), and the page's ground. */
  S.state = (anchor: string | null) => {
    const ground = getComputedStyle(document.body).backgroundColor;
    const root = getComputedStyle(document.documentElement).backgroundColor;
    const isWhite = white(ground) || (clear(ground) && (white(root) || clear(root)));
    const main = document.getElementById("main");
    const header = document.querySelector("#app .site-header")?.getBoundingClientRect().bottom ?? 0;
    const opacity = new Map<Element, number>();
    const seen = (el: Element) => {
      let o = 1;
      for (let n: Element | null = el; n && n !== document.documentElement; n = n.parentElement) {
        let v = opacity.get(n);
        if (v === undefined) {
          const cs = getComputedStyle(n);
          v = cs.visibility === "hidden" ? 0 : parseFloat(cs.opacity);
          opacity.set(n, v);
        }
        o *= v;
        if (o < 0.05) break;
      }
      return o;
    };
    let content = 0;
    let placeholders = 0;
    let covered = 0;
    if (main) {
      const top = Math.max(0, header);
      for (const fx of [0.25, 0.5, 0.75])
        for (const fy of [0.15, 0.35, 0.55, 0.75, 0.92]) {
          const el = document.elementFromPoint(innerWidth * fx, top + (innerHeight - top) * fy);
          if (!el || el === main || el === document.body || el === document.documentElement) continue;
          // A dialog over the page, or the button over the feed: not the page's own.
          if (!main.contains(el)) {
            if (!el.closest("#boot")) covered += 1;
            continue;
          }
          if (el.matches(".columns, .content, .cast-list, .page")) continue;
          if (seen(el) < 0.5) continue;
          if (el.closest(".skeleton")) placeholders += 1;
          else content += 1;
        }
    }
    let anchorTop: number | null = null;
    if (anchor) {
      const card = document.querySelector(`#app .cast-card[data-hash="${anchor}"]`);
      if (card) anchorTop = card.getBoundingClientRect().top;
    }
    return {
      blank: content + placeholders + covered === 0,
      skeleton: content === 0 && covered === 0 && placeholders > 0,
      placeholder: placeholders > 0,
      white: isWhite,
      anchor: anchorTop,
      scrollY,
      path: location.pathname + location.search,
    };
  };
  /** Start a measured window: every frame is sampled until `end`. */
  S.begin = (label: string, anchor: string | null) => {
    const run = { label, t0: performance.now(), frames: [] as Any[], anchor };
    S.run = run;
    const tick = (t: number) => {
      if (S.run !== run) return;
      run.frames.push({ t, ...S.state(run.anchor) });
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
    return run.t0;
  };
  S.end = () => {
    const run = S.run;
    S.run = null;
    const t1 = performance.now();
    const inside = (e: Any) => e.start >= run.t0 - 1 && e.start <= t1;
    return {
      label: run.label,
      t0: run.t0,
      t1,
      events: S.events.filter(inside),
      longtasks: S.longtasks.filter((e: Any) => e.start + e.dur >= run.t0 && e.start <= t1),
      loafs: S.loafs.filter((e: Any) => e.start + e.dur >= run.t0 && e.start <= t1),
      shifts: S.shifts.filter(inside),
      frames: run.frames,
    };
  };
  /** Resolves once the page has been idle for a moment (two frames, then idle time). */
  S.idle = () =>
    new Promise((resolve) =>
      requestAnimationFrame(() =>
        requestAnimationFrame(() => ((window as Any).requestIdleCallback ? (window as Any).requestIdleCallback(resolve, { timeout: 1000 }) : setTimeout(resolve, 50))),
      ),
    );

  /*
   * The press probe: how `el` and what it holds are drawn at rest, then in
   * the first frame after the next pointerdown. How each is drawn: its
   * transform's scale and offset, its scale property, its opacity.
   */
  const drawn = (n: Element) => {
    const cs = getComputedStyle(n);
    const m = cs.transform === "none" ? new DOMMatrix() : new DOMMatrix(cs.transform);
    const scale = cs.scale === "none" ? 1 : parseFloat(cs.scale);
    return [Math.hypot(m.a, m.b) * scale, Math.hypot(m.c, m.d) * scale, m.e, m.f, parseFloat(cs.opacity)];
  };
  const looks = (el: Element) => [el, ...Array.from(el.querySelectorAll("*")).slice(0, 24)].map(drawn);
  const differs = (a: number[][], b: number[][]) =>
    a.some((v, i) => v.some((x, j) => Math.abs(x - b[i][j]) > (j === 2 || j === 3 ? 0.5 : 0.004)));
  const box = (el: HTMLElement) => [el.offsetLeft, el.offsetTop, el.offsetWidth, el.offsetHeight].join(",");
  /*
   * Where to press `el` once it has stayed put: its box the same for 200 ms
   * (and three frames), no image on screen still loading, and the point
   * pressed on it, not on something drawn over it. A scroll to it can bring
   * cards above it into view whose photos take their shape a moment later
   * (a square until the size comes), which moves it after the scroll is
   * done; a reader presses what has sat still.
   */
  S.steady = (el: Element) =>
    new Promise((resolve) => {
      const start = performance.now();
      let last = "";
      let since = start;
      let frames = 0;
      const frame = () => {
        const now = performance.now();
        const r = el.getBoundingClientRect();
        const point = { x: r.x + Math.min(r.width / 2, 20), y: r.y + r.height / 2 };
        const at = [r.x, r.y, r.width, r.height].map(Math.round).join(",");
        const loading = Array.from(document.images).some((img) => {
          if (img.complete) return false;
          const b = img.getBoundingClientRect();
          return b.bottom > 0 && b.top < innerHeight && b.right > 0 && b.left < innerWidth;
        });
        const hit = el.contains(document.elementFromPoint(point.x, point.y));
        if (at !== last || loading || !hit) {
          last = at;
          since = now;
          frames = 0;
        } else frames += 1;
        if ((frames >= 3 && now - since >= 200) || now - start > 3000) resolve(point);
        else requestAnimationFrame(frame);
      };
      requestAnimationFrame(frame);
    });
  S.armPress = (el: HTMLElement) => {
    const before = looks(el);
    const layout = box(el);
    S.press = null;
    document.addEventListener(
      "pointerdown",
      (event) => {
        const down = event.timeStamp;
        requestAnimationFrame(() => {
          const after = looks(el);
          S.press = { changed: differs(before, after), frameMs: performance.now() - down, moved: box(el) !== layout, before: before[0].map((x) => Math.round(x * 1000) / 1000).join(" "), after: after[0].map((x) => Math.round(x * 1000) / 1000).join(" ") };
        });
      },
      { capture: true, once: true },
    );
  };
}

// ---------------------------------------------------------------- results

type Measured = {
  inp: number;
  longTask: number; // the longest long task overlapping the interaction, 0 for none
  settleLongTask: number; // the longest long task in the second after it
  loaf: number; // the longest animation frame in the second after it
  shift: number; // layout shift after its first frame, input or not
  cls: number; // the same, without shifts the browser counts as answering input
  blankFrames: number;
  skeletonFrames: number; // frames showing placeholders and nothing else
  placeholderFrames: number; // frames showing a placeholder anywhere
  whiteFrames: number;
  frames: number;
  /** A navigation: from the input to the first frame of the next page with something on it, -1 if none in the window. */
  shownMs: number;
  restorePx?: number; // Back: how far the reader's card is from where it was
  restoreFrames?: number; // Back: frames showing the page before the card was back in place
  /** What moved, for a report: each shift's value and its first sources. Not a number, so no median. */
  moved?: string[];
};

const median = (values: number[]) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)];
/** `median`, for where a scenario's median has the name. */
const middle = median;
const round = (n: number, digits = 0) => Math.round(n * 10 ** digits) / 10 ** digits;

/** What a sample says about one interaction (or a navigation with none, such as Back). */
function measure(sample: Sample, back?: { top: number; path: string }): Measured {
  // An interaction's latency is its slowest event's, from the event to the
  // frame painted after it (pointerdown, pointerup and click are each timed).
  const inp = Math.max(0, ...sample.events.map((e) => e.dur));
  // The main thread's part of it: from the event until its handlers are done.
  // A long task or a long animation frame overlapping that held the input
  // back or ran with it (a long frame includes its rendering). The time the
  // frame then takes to reach the screen isn't the page's work.
  const windows = sample.events.map((e) => ({ start: e.start, end: Math.max(e.processed, e.start + 1) }));
  const overlaps = (t: { start: number; dur: number }, w: { start: number; end: number }) => t.start < w.end && t.start + t.dur > w.start;
  const longTask = Math.max(0, ...[...sample.longtasks, ...sample.loafs].filter((t) => windows.some((w) => overlaps(t, w))).map((t) => t.dur));
  const settle = { start: sample.t0, end: sample.t0 + 1000 };
  const settleLongTask = Math.max(0, ...sample.longtasks.filter((t) => overlaps(t, settle)).map((t) => t.dur));
  const loaf = Math.max(0, ...sample.loafs.filter((t) => overlaps(t, settle)).map((t) => t.dur));
  // The interaction's own frame may move what it answers; what moves after it is a shift.
  const firstFrame = Math.max(sample.t0, ...sample.events.map((e) => e.start + e.dur));
  const after = sample.shifts.filter((s) => s.start > firstFrame);
  const shift = after.reduce((sum, s) => sum + s.value, 0);
  const cls = after.filter((s) => !s.recent).reduce((sum, s) => sum + s.value, 0);
  const frames = sample.frames;
  const from = frames[0]?.path;
  const shown = frames.find((f) => (back ? f.path === back.path : f.path !== from) && !f.blank);
  const out: Measured = {
    inp: round(inp),
    longTask: round(longTask),
    settleLongTask: round(settleLongTask),
    loaf: round(loaf),
    shift: round(shift, 4),
    cls: round(cls, 4),
    blankFrames: frames.filter((f) => f.blank).length,
    skeletonFrames: frames.filter((f) => f.skeleton).length,
    placeholderFrames: frames.filter((f) => f.placeholder).length,
    whiteFrames: frames.filter((f) => f.white).length,
    frames: frames.length,
    shownMs: shown ? round(shown.t - Math.max(sample.t0, ...sample.events.filter((e) => e.start <= shown.t).map((e) => e.start))) : -1,
    moved: after.map((s) => `${round(s.value, 4)}${s.recent ? " (input)" : ""}: ${s.sources.join(", ")}`).slice(0, 4),
  };
  if (back) {
    const last = [...frames].reverse().find((f) => f.anchor !== null);
    // In whole pixels, as the budget is.
    out.restorePx = last ? round(Math.abs(last.anchor! - back.top)) : 9999;
    // From the first frame of the page gone back to: frames that showed it
    // before the card was back where it was.
    const returned = frames.filter((f) => f.path === back.path);
    const placed = returned.findIndex((f) => f.anchor !== null && Math.abs(f.anchor - back.top) <= RESTORE_BUDGET_PX);
    out.restoreFrames = (placed < 0 ? returned : returned.slice(0, placed)).filter((f) => !f.blank).length;
  }
  return out;
}

type Scenario = { runs: Measured[]; median: Measured };
type Press = { control: string; changed: boolean; frameMs: number; moved: boolean; before: string; after: string };
type Machine = { share: number; shares: number[]; load: number; cpus: number };
type Results = { profile: Profile; scenarios: Record<string, Scenario>; presses: Press[]; scroll: ScrollRun[]; machine: Machine };
type ScrollRun = { intervals: number[]; p95: number; max: number; dropped: number; longTask: number; loaf: number };

function summarize(runs: Measured[]): Scenario {
  const keys = (Object.keys(runs[0]) as (keyof Measured)[]).filter((k) => typeof runs[0][k] === "number");
  const mid = Object.fromEntries(keys.map((k) => [k, median(runs.map((r) => r[k] as number))])) as Measured;
  return { runs, median: mid };
}

/** Whether a median is near enough a timed budget (within a fifth either side) for a busy machine's noise to decide it. */
const close = (m: Measured) =>
  (m.inp >= INP_BUDGET * 0.8 && m.inp < INP_BUDGET * 1.25) || (m.longTask >= LONG_TASK * 0.8 && m.longTask < LONG_TASK * 1.25);

/**
 * Runs an interaction `RUNS` times, and twice more when the median of what
 * `judged` picks out of each run is close to a budget, so that five runs, not
 * one noisy one, decide a close call. `once` gets the run's index.
 */
async function repeat<T>(once: (i: number) => Promise<T>, judged: (run: T) => Measured): Promise<T[]> {
  const runs: T[] = [];
  for (let i = 0; i < RUNS; i++) runs.push(await once(i));
  if (close(summarize(runs.map(judged)).median)) for (let i = RUNS; i < RUNS + 2; i++) runs.push(await once(i));
  return runs;
}

const itself = (m: Measured) => m;

/**
 * How much of a CPU a busy thread gets on this machine now: the CPU time a
 * 300 ms spin gets over its wall time. Near 1 when the run has a CPU to
 * itself; on a machine busy with other work, the browser's main thread gets
 * as little.
 */
function cpuShare(ms = 300) {
  const from = process.cpuUsage();
  const start = performance.now();
  let x = 0;
  while (performance.now() - start < ms) x += Math.sqrt(x + 1);
  const used = process.cpuUsage(from);
  return (used.user + used.system) / 1000 / (performance.now() - start);
}

// ---------------------------------------------------------------- drivers

const cards = (page: Page) => page.locator("#app .cast-card[data-hash]:not(.skeleton)");

/**
 * Between measurements: the mouse over the page's margin (no hover card or
 * preview open), the garbage of what came before collected (so a pause for
 * it doesn't land in the next measurement), and the page idle.
 */
async function settle(page: Page, ms = 250) {
  const viewport = page.viewportSize()!;
  if (viewport.width > 600) await page.mouse.move(6, viewport.height - 6);
  await sessions.get(page)?.send("HeapProfiler.collectGarbage").catch(() => {});
  await page.waitForTimeout(ms);
  await page.evaluate(() => (window as any).__smooth.idle());
}

async function begin(page: Page, label: string, anchor: string | null = null) {
  await page.evaluate(([label, anchor]) => (window as any).__smooth.begin(label, anchor), [label, anchor] as const);
}

async function end(page: Page, ms = 1100): Promise<Sample> {
  await page.waitForTimeout(ms);
  return page.evaluate(() => (window as any).__smooth.end());
}

/**
 * A press where it shows, without scrolling first, as a reader's: on the
 * phone a finger down for 80 ms, on a computer a mouse that comes to rest on
 * the target for 250 ms (which is when hovering prefetches) and clicks, its
 * button down for 80 ms. (Down and up at once, as this did before, the click
 * waited for the frame that showed the press, as no reader's does.)
 */
async function press(page: Page, target: Locator, profile: Profile) {
  await expect(target).toBeVisible();
  const box = (await target.boundingBox())!;
  const viewport = page.viewportSize()!;
  const x = box.x + Math.min(box.width / 2, 24);
  const y = box.y + box.height / 2;
  expect(y, "the control is on screen").toBeGreaterThan(0);
  expect(y, "the control is on screen").toBeLessThan(viewport.height);
  if (profile.touch) {
    const cdp = sessions.get(page)!;
    await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ x, y }] });
    await page.waitForTimeout(80);
    await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
  } else {
    await page.mouse.move(x, y, { steps: 4 });
    await page.waitForTimeout(250);
    await page.mouse.down();
    await page.waitForTimeout(80);
    await page.mouse.up();
  }
}

/** Each page's DevTools session: its touches, and its garbage collection between measurements. */
const sessions = new WeakMap<Page, CDPSession>();

/** The card at the reading line (120 px down once scrolled), below the feed toolbar at the top. */
async function cardAtReadingLine(page: Page) {
  return page.evaluate(() => {
    const list = document.querySelector("#app .cast-list")!.getBoundingClientRect();
    // For you / Following adds a row above the cards. At scroll zero the
    // reading line must start inside the list, not inside those controls.
    const line = Math.max(120, list.top + 16);
    const el = [line, line + 30, line + 60].map((y) => document.elementFromPoint(list.left + 24, y)?.closest(".cast-card[data-hash]")).find(Boolean);
    return el ? { hash: el.getAttribute("data-hash")!, top: el.getBoundingClientRect().top } : null;
  });
}

async function scrollTo(page: Page, y: number) {
  await page.evaluate((y) => window.scrollTo(0, y), y);
  await page.waitForTimeout(150);
  await settle(page);
}

async function open(browser: Browser, profile: Profile) {
  const context = await browser.newContext({
    baseURL: test.info().project.use.baseURL,
    viewport: { width: profile.width, height: profile.height },
    isMobile: profile.touch,
    hasTouch: profile.touch,
  });
  const page = await context.newPage();
  await page.addInitScript(harness);
  await signIn(page, me);
  const calls = await serve(page);
  await page.goto("/");
  await expect(cards(page).first()).toBeVisible();
  // An approved key, so a like goes out from here.
  await approvedKey(page, me.fid, 3_600_000);
  const cdp = await context.newCDPSession(page);
  sessions.set(page, cdp);
  if (profile.cpu > 1) await cdp.send("Emulation.setCPUThrottlingRate", { rate: profile.cpu });
  await page.reload();
  await appCame(page);
  await expect(cards(page)).toHaveCount(25);
  // As a reader would: a few seconds on the page before the first tap, in
  // which the post page's code comes ahead of use.
  await page.waitForTimeout(3500);
  await settle(page);
  return { context, page, calls, cdp };
}

async function sortTabs(page: Page, profile: Profile) {
  // Each tab a different list from the one before: new and top are lists of
  // their own, hot re-sorts top's.
  const order = ["new", "top", "hot", "new", "top"];
  const runs = await repeat(async (i) => {
    const name = order[i];
    await settle(page);
    await begin(page, `sort ${name}`);
    await press(page, page.locator("#app .sort-tabs a", { hasText: name }), profile);
    await expect(page.locator("#app .sort-tabs a.selected")).toHaveText(name);
    await expect(cards(page)).toHaveCount(25);
    return measure(await end(page, 700));
  }, itself);
  // Back to hot, the front page's own.
  if (page.url().includes("sort=")) await page.locator("#app .sort-tabs a", { hasText: "hot" }).click();
  await expect(page.locator("#app .sort-tabs a.selected")).toHaveText("hot");
  return summarize(runs);
}

async function likesPopover(page: Page, profile: Profile) {
  const runs = await repeat(async (i) => {
    const count = cards(page).nth(i).locator(".stat-likes .stat-count");
    await count.scrollIntoViewIfNeeded();
    await settle(page);
    await begin(page, `likes ${i}`);
    await press(page, count, profile);
    // A tap previews who liked; a click opens the whole list.
    const opened = profile.touch ? cards(page).nth(i).getByRole("dialog", { name: "Liked by" }) : page.locator(".who-modal");
    await expect(opened).toBeVisible();
    await expect(opened.getByRole("link").first()).toBeVisible();
    const measured = measure(await end(page, 600));
    await page.keyboard.press("Escape");
    await expect(opened).toHaveCount(0);
    return measured;
  }, itself);
  await scrollTo(page, 0);
  return summarize(runs);
}

async function like(page: Page, profile: Profile, calls: { submitted: number }) {
  const heart = cards(page).first().getByRole("button", { name: "Like", exact: true });
  await heart.scrollIntoViewIfNeeded();
  // Like, unlike, like…
  const runs = await repeat(async (i) => {
    await settle(page, 1100); // a like and its undo are a second apart
    const sent = calls.submitted;
    await begin(page, `like ${i}`);
    await press(page, heart, profile);
    await expect(heart).toHaveAttribute("aria-pressed", i % 2 === 0 ? "true" : "false");
    await expect.poll(() => calls.submitted).toBe(sent + 1);
    return measure(await end(page, 600));
  }, itself);
  return summarize(runs);
}

async function composer(page: Page, profile: Profile) {
  const button = page.locator(profile.touch ? ".cast-fab" : ".cast-pill");
  const runs = await repeat(async (i) => {
    await settle(page);
    await begin(page, `composer ${i}`);
    await press(page, button, profile);
    const dialog = page.getByRole("dialog", { name: "New cast" });
    await expect(dialog).toBeVisible();
    await expect(dialog.locator(".compose-loading")).toHaveCount(0);
    const measured = measure(await end(page, 600));
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    return measured;
  }, itself);
  return summarize(runs);
}

/** What a cast says as it's typed: a line with a Pepe in it, as most drafts have about as much. */
const DRAFT = "Walked the long way home tonight :pepe_smug: and the light on the water was something else, ";

/**
 * In the composer: the emoji picker and the sticker picker, each opened by
 * its button (and closed again), and a key typed at the end of a draft.
 */
async function inComposer(page: Page, profile: Profile) {
  await page.locator(profile.touch ? ".cast-fab" : ".cast-pill").click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await expect(dialog).toBeVisible();
  await expect(dialog.locator(".compose-loading")).toHaveCount(0);
  const picker = async (name: string, tile: string) => {
    const button = dialog.getByRole("button", { name });
    const runs = await repeat(async (i) => {
      await settle(page);
      await begin(page, `${name} ${i}`);
      await press(page, button, profile);
      await expect(dialog.locator(`#emoji-picker ${tile}`).first()).toBeVisible();
      const measured = measure(await end(page, 600));
      await button.click();
      await expect(dialog.locator("#emoji-picker")).toHaveCount(0);
      return measured;
    }, itself);
    return summarize(runs);
  };
  const emoji = await picker("Add an emoji", ".emoji-tile");
  const stickers = await picker("Add a sticker", ".sticker-tile");
  const text = dialog.locator(".compose-text");
  await text.focus();
  await page.keyboard.insertText(DRAFT);
  await expect(text).toContainText("the light on the water");
  const runs = await repeat(async (i) => {
    await settle(page);
    await begin(page, `key ${i}`);
    await page.keyboard.press("abcde"[i]);
    await expect(text).toContainText(`else, ${"abcde".slice(0, i + 1)}`);
    return measure(await end(page, 600));
  }, itself);
  // Emptied, so what comes next opens a new cast as before.
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.press("Delete");
  await expect(text).toHaveText("");
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  return { emoji, stickers, keystroke: summarize(runs) };
}

/** Open a page from the feed at three depths, then go Back: the opening, and the return. */
async function openAndBack(page: Page, profile: Profile, kind: "cast" | "profile") {
  const depths = [0, 1.3, 2.6, 0.65, 1.95];
  const runs = await repeat(
    async (i) => {
      await scrollTo(page, depths[i] * profile.height);
      const at = (await cardAtReadingLine(page))!;
      expect(at, "a card at the reading line").not.toBeNull();
      const feed = await page.evaluate(() => location.pathname + location.search);
      const card = page.locator(`#app .cast-card[data-hash="${at.hash}"]`);
      await begin(page, `open ${kind} ${i}`);
      if (kind === "cast") {
        await press(page, card.locator(".cast-text"), profile);
        await expect(page.locator("#app .post .post-text")).toBeVisible();
        await expect(page.locator("#app .replies .comments")).toBeVisible();
      } else {
        const name = card.locator(".cast-meta .display-name");
        // On a touch screen a tap shows the account's card; its link opens the profile.
        await press(page, name, profile);
        if (profile.touch) await press(page, card.locator(".profile-card .popover-foot"), profile);
        await expect(page.locator("#app .profile .profile-name")).toBeVisible();
        await expect(cards(page).first()).toBeVisible();
      }
      const open = measure(await end(page));
      await settle(page);
      await begin(page, `back from ${kind} ${i}`, at.hash);
      await page.goBack({ waitUntil: "commit" });
      await expect(card).toBeVisible();
      const back = measure(await end(page, 1500), { top: at.top, path: feed });
      return { open, back };
    },
    (run) => run.open,
  );
  await scrollTo(page, 0);
  return { open: summarize(runs.map((r) => r.open)), back: summarize(runs.map((r) => r.back)) };
}

/** Deep in the feed (its second server page), open a cast and come back. */
async function deepBack(page: Page, profile: Profile) {
  const backs: Measured[] = [];
  for (let i = 0; i < RUNS; i++) {
    // Down to the cards the next page brought, past the first 50.
    for (let n = 0; n < 12 && (await cards(page).count()) < 75; n++) {
      await page.evaluate(() => window.scrollBy(0, innerHeight * 2));
      await page.waitForTimeout(250);
    }
    await page.evaluate((at) => window.scrollTo(0, document.documentElement.scrollHeight * at), [0.55, 0.65, 0.75][i]);
    await settle(page, 400);
    const at = (await cardAtReadingLine(page))!;
    const feed = await page.evaluate(() => location.pathname + location.search);
    const card = page.locator(`#app .cast-card[data-hash="${at.hash}"]`);
    await press(page, card.locator(".cast-text"), profile);
    await expect(page.locator("#app .post .post-text")).toBeVisible();
    await settle(page, 600);
    await begin(page, `deep back ${i}`, at.hash);
    await page.goBack({ waitUntil: "commit" });
    await expect(card).toBeVisible();
    backs.push(measure(await end(page, 1500), { top: at.top, path: feed }));
  }
  await scrollTo(page, 0);
  return summarize(backs);
}

/**
 * A long scroll down the feed, a finger's on the phone and the wheel's on a
 * computer (6,000 px at 2,500 px/s, past the cards drawn first, so the list
 * draws more as it goes), timing each frame by rAF.
 */
async function scrollFeed(page: Page, profile: Profile, cdp: CDPSession) {
  const runs: ScrollRun[] = [];
  for (let i = 0; i < RUNS; i++) {
    await scrollTo(page, 0);
    await page.evaluate(() => {
      const w = window as any;
      w.__frames = [];
      const tick = (t: number) => {
        if (!w.__frames) return;
        w.__frames.push(t);
        requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });
    const t0 = await page.evaluate(() => performance.now());
    await cdp.send("Input.synthesizeScrollGesture", {
      x: profile.width / 2,
      y: profile.height * 0.7,
      yDistance: -6000,
      speed: 2500,
      gestureSourceType: profile.touch ? "touch" : "mouse",
    });
    const { frames, tasks, loafs } = await page.evaluate((t0) => {
      const w = window as any;
      const frames = w.__frames as number[];
      w.__frames = null;
      const t1 = performance.now();
      const within = (e: { start: number; dur: number }) => e.start + e.dur > t0 && e.start < t1;
      return { frames, tasks: w.__smooth.longtasks.filter(within), loafs: w.__smooth.loafs.filter(within) };
    }, t0);
    const intervals = frames.slice(1).map((t, k) => t - frames[k]);
    const sorted = [...intervals].sort((a, b) => a - b);
    runs.push({
      intervals: intervals.map((v) => round(v, 1)),
      p95: round(sorted[Math.floor(sorted.length * 0.95)] ?? 0, 1),
      max: round(sorted[sorted.length - 1] ?? 0, 1),
      dropped: intervals.reduce((sum, v) => sum + Math.max(0, Math.round(v / (1000 / 60)) - 1), 0),
      longTask: round(Math.max(0, ...tasks.map((t: { dur: number }) => t.dur))),
      loaf: round(Math.max(0, ...loafs.map((t: { dur: number }) => t.dur))),
    });
  }
  await scrollTo(page, 0);
  return runs;
}

/** Controls a reader presses, and whether each answers in the first frame. */
async function presses(page: Page, profile: Profile, cdp: CDPSession) {
  const out: Press[] = [];
  /** Press `target`, and let go over `away`: off it, so nothing is opened, and inside any dialog it's in, which a click on its backdrop would close. */
  const probe = async (control: string, target: Locator, away?: Locator) => {
    if (!(await target.count())) return;
    const el = target.first();
    await el.scrollIntoViewIfNeeded();
    // A touch goes to a point, and unlike a click doesn't wait for what is
    // there to be still: pressed where it is once it has stayed put, or it
    // lands on whatever moved there (the next card's picture, once a photo
    // above the link card had taken its shape).
    const { x, y }: { x: number; y: number } = await el.evaluate((node) => (window as any).__smooth.steady(node));
    if (!profile.touch) {
      // Pointing at it first, as a mouse does, and letting its hover settle.
      await page.mouse.move(x, y);
      await page.waitForTimeout(350);
    }
    await el.evaluate((node) => (window as any).__smooth.armPress(node));
    if (profile.touch) await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ x, y }] });
    else await page.mouse.down();
    await expect.poll(() => page.evaluate(() => (window as any).__smooth.press)).not.toBeNull();
    const result = await page.evaluate(() => (window as any).__smooth.press);
    if (profile.touch) await cdp.send("Input.dispatchTouchEvent", { type: "touchCancel", touchPoints: [] });
    else {
      const off = away ? (await away.boundingBox())! : { x: 2, y: profile.height - 3, width: 0, height: 0 };
      await page.mouse.move(off.x + 1, off.y + off.height / 2);
      await page.mouse.up();
    }
    out.push({ control, ...result, frameMs: round(result.frameMs, 1) });
    await page.waitForTimeout(150);
  };
  const first = cards(page).first();
  // The front page's.
  await probe("sort tab", page.locator("#app .sort-tabs a:not(.selected)"));
  await probe("time range", page.locator("#app .time-filter select"));
  await probe(profile.touch ? "Cast (round button)" : "Cast (pill)", page.locator(profile.touch ? ".cast-fab" : ".cast-pill"));
  await probe("like", first.getByRole("button", { name: "Like", exact: true }));
  await probe("recast", first.getByRole("button", { name: "Recast", exact: true }));
  await probe("likes count", first.locator(".stat-likes .stat-count"));
  await probe("replies count", first.locator(".stat-replies"));
  await probe("account picture", page.locator(".account-button"));
  await probe("author name", first.locator(".cast-meta .display-name"));
  await probe("link card", cards(page).locator("a.link-card").first());
  // The composer's.
  await page.locator(profile.touch ? ".cast-fab" : ".cast-pill").click();
  const composer = page.getByRole("dialog", { name: "New cast" });
  await expect(composer).toBeVisible();
  await expect(composer.locator(".compose-loading")).toHaveCount(0);
  const title = composer.locator("#compose-title");
  await probe("emoji (composer)", composer.getByRole("button", { name: "Add an emoji" }), title);
  await probe("sticker (composer)", composer.getByRole("button", { name: "Add a sticker" }), title);
  await probe("schedule (composer)", composer.getByRole("button", { name: "Schedule for later" }), title);
  await probe("close (composer)", composer.getByRole("button", { name: "Close" }), title);
  await page.keyboard.press("Escape");
  await expect(composer).toHaveCount(0);
  // A post's.
  await first.locator("a.when").click();
  await expect(page.locator("#app .post .post-text")).toBeVisible();
  await expect(page.locator("#app .replies .comments")).toBeVisible();
  const post = page.locator("#app .post");
  await probe("reply", post.locator(".reply-button").first());
  await probe("quote", post.locator(".quote-button"));
  await probe("react", post.locator(".react-button"));
  await probe("replies' sort tab", page.locator("#app .replies-head .sort-tabs a:not(.selected)"));
  // A profile's.
  await page.goto("/u/writer1");
  await expect(page.locator("#app .profile .profile-name")).toBeVisible();
  await expect(page.locator("#app .follow-button")).toBeVisible();
  await probe("profile tab", page.locator("#app .tabs a:not(.selected)"));
  await probe("Favorite", page.locator("#app .favorite-button"));
  await probe("Show less", page.locator("#app .show-less-button"));
  await probe("Follow", page.locator("#app .follow-button"));
  return out;
}

// ------------------------------------------------------------------ tests

const results: Partial<Record<string, Results>> = {};

async function run(browser: Browser, profile: Profile): Promise<Results> {
  const tracing = traced(profile);
  const { context, page, calls, cdp } = await open(browser, profile);
  // Its screencast, actions and network; DOM snapshots would slow it more.
  if (tracing) await context.tracing.start({ screenshots: true, snapshots: false });
  try {
    // How busy the machine is, before each interaction.
    const shares: number[] = [];
    const timed = <T>(work: () => Promise<T>) => {
      shares.push(cpuShare());
      return work();
    };
    const scenarios: Record<string, Scenario> = {};
    scenarios["sort tab"] = await timed(() => sortTabs(page, profile));
    scenarios["likes popover"] = await timed(() => likesPopover(page, profile));
    scenarios["like"] = await timed(() => like(page, profile, calls));
    scenarios["composer"] = await timed(() => composer(page, profile));
    const typing = await timed(() => inComposer(page, profile));
    scenarios["emoji picker"] = typing.emoji;
    scenarios["sticker picker"] = typing.stickers;
    scenarios["keystroke"] = typing.keystroke;
    const castNav = await timed(() => openAndBack(page, profile, "cast"));
    scenarios["open cast"] = castNav.open;
    scenarios["back from cast"] = castNav.back;
    const profileNav = await timed(() => openAndBack(page, profile, "profile"));
    scenarios["open profile"] = profileNav.open;
    scenarios["back from profile"] = profileNav.back;
    scenarios["back deep in the feed"] = await timed(() => deepBack(page, profile));
    const scroll = await timed(() => scrollFeed(page, profile, cdp));
    const pressed = await timed(() => presses(page, profile, cdp));
    const machine = { share: round(median(shares), 2), shares: shares.map((s) => round(s, 2)), load: round(loadavg()[0], 1), cpus: cpus().length };
    return { profile, scenarios, presses: pressed, scroll, machine };
  } catch (error) {
    // Where it stopped, for the report CI uploads.
    await page.screenshot({ path: test.info().outputPath(`stopped-${profile.name}.png`) }).catch(() => {});
    throw error;
  } finally {
    if (tracing) await context.tracing.stop({ path: process.env.SMOOTH_TRACE });
    await context.close();
  }
}

function report(r: Results) {
  const lines = [`smoothness at ${r.profile.name} px${r.profile.cpu > 1 ? `, CPU ${r.profile.cpu}× slower` : ""} (medians of ${RUNS} runs, or 5 where close to a budget):`];
  for (const [name, s] of Object.entries(r.scenarios)) {
    const m = s.median;
    const parts = [
      // Back is the browser's, not an interaction with the page.
      name.startsWith("back") ? "" : `INP ${String(m.inp).padStart(3)} ms`,
      name.startsWith("back") ? "" : `long task ${String(m.longTask).padStart(3)} ms`,
      `the next second's longest task ${m.settleLongTask} ms and frame ${m.loaf} ms`,
      `shift ${m.shift}`,
      `blank ${m.blankFrames}`,
      `placeholders ${m.placeholderFrames}`,
      m.shownMs >= 0 && (name.startsWith("open") || name.startsWith("back")) ? `shown in ${m.shownMs} ms` : "",
      m.restorePx !== undefined ? `card back within ${m.restorePx} px` : "",
    ];
    lines.push(`  ${name.padEnd(22)} ${parts.filter(Boolean).join(", ")}${s.runs.length > RUNS ? ` (${s.runs.length} runs)` : ""}`);
  }
  for (const s of r.scroll) lines.push(`  scroll: p95 frame ${s.p95} ms, longest ${s.max} ms, ${s.dropped} of ${s.intervals.length} frames dropped, long task ${s.longTask} ms`);
  const answered = r.presses.filter((p) => p.changed && !p.moved).map((p) => p.control);
  const not = r.presses.filter((p) => !p.changed || p.moved).map((p) => `${p.control}${p.moved ? " (moves)" : ""}`);
  lines.push(`  presses answered in their first frame: ${answered.join(", ") || "none"}; not: ${not.join(", ") || "none"}`);
  const m = r.machine;
  lines.push(`  machine: a busy thread got ${m.share} of a CPU (median of ${m.shares.length}; ${m.shares.join(", ")}), load ${m.load} on ${m.cpus} CPUs`);
  console.log(lines.join("\n"));
}

/** What each kind of interaction is held to. */
const ANSWERS = ["sort tab", "likes popover", "like", "composer", "emoji picker", "sticker picker", "keystroke", "open cast", "open profile"];
const NAVIGATIONS = ["sort tab", "open cast", "open profile", "back from cast", "back from profile", "back deep in the feed"];
const BACKS = ["back from cast", "back from profile", "back deep in the feed"];

/**
 * Budgets the app doesn't meet yet, by profile and check, with what it
 * measured. Each runs as an expected failure (`test.fail`): CI stays green,
 * and a change that meets one fails CI until it takes its line out of here.
 * Opening a cast or a profile in the tap's own task, and the frames with
 * nothing to read as a page or a list came in from transparent, were here
 * until perf/instant-nav (docs/PERFORMANCE.md#smoothness).
 */
const NOT_YET: Record<string, string> = {};

/**
 * Budgets the app misses, or only just meets, by less than a busy
 * machine's noise: as expected failures they would pass now and then and
 * fail CI, so they don't run (`test.fixme`) until a change meets them with
 * room to spare.
 */
const TOO_CLOSE: Record<string, string> = {
  // A tap on a name opens the account's card first, which is drawn in the
  // tap's own task (popover.rs); its "View profile" then opens the profile.
  "390 open profile INP": "80–112 ms over five runs of five",
  "390 open profile long task": "0–59 ms over five runs of five",
};

/** Whether the profile's run is recorded in a trace, which slows it: its numbers are for reading, not for budgets. */
const traced = (profile: Profile) => profile.touch && Boolean(process.env.SMOOTH_TRACE);

/**
 * Marks a check the app doesn't meet yet (see `NOT_YET` and `TOO_CLOSE`),
 * or one a trace, another slowdown or, for a `timed` one, a busy machine
 * made moot.
 */
function known(profile: Profile, check: string, timed = false) {
  const key = `${profile.name} ${check}`;
  test.skip(traced(profile), "recording a trace slows the run");
  test.skip(profile.cpu !== (profile.touch ? CALIBRATED_CPU : 1), `the budgets hold at a ${CALIBRATED_CPU}× slowdown`);
  const machine = results[profile.name]?.machine;
  test.skip(
    timed && !STRICT && machine !== undefined && machine.share < QUIET_SHARE,
    `a busy machine: a thread got ${machine?.share} of a CPU (load ${machine?.load} on ${machine?.cpus} CPUs), under the ${QUIET_SHARE} the budgets need; SMOOTH_STRICT=1 judges them anyway`,
  );
  test.fixme(key in TOO_CLOSE, TOO_CLOSE[key]);
  test.fail(key in NOT_YET, NOT_YET[key]);
}

/** Merges this profile's numbers into SMOOTH_OUT's file. */
function save(r: Results) {
  const file = process.env.SMOOTH_OUT;
  if (!file) return;
  let all: Record<string, unknown> = {};
  try {
    all = JSON.parse(readFileSync(file, "utf8"));
  } catch {
    /* a new file */
  }
  all[r.profile.name] = { ...r, measured: new Date().toISOString() };
  writeFileSync(file, JSON.stringify(all, null, 1));
}

for (const profile of [PHONE, DESKTOP].filter((p) => !process.env.SMOOTH_ONLY || p.name === process.env.SMOOTH_ONLY)) {
  test.describe(`smoothness at ${profile.name} px`, () => {
    test.describe.configure({ mode: "serial" });
    const got = () => results[profile.name]!;
    const median = (scenario: string) => got().scenarios[scenario].median;

    test(`measure: taps, navigations, scrolling and presses at ${profile.name} px`, async ({ browser }) => {
      test.setTimeout(300_000);
      const r = await run(browser, profile);
      results[profile.name] = r;
      report(r);
      save(r);
    });

    for (const scenario of ANSWERS) {
      test(`${scenario}: answers within ${INP_BUDGET} ms (INP)`, () => {
        known(profile, `${scenario} INP`, true);
        expect(median(scenario).inp).toBeLessThan(INP_BUDGET);
      });
      test(`${scenario}: no long task overlaps it`, () => {
        known(profile, `${scenario} long task`, true);
        expect(median(scenario).longTask).toBeLessThan(LONG_TASK);
      });
    }
    for (const scenario of NAVIGATIONS) {
      test(`${scenario}: no blank or white frame`, () => {
        known(profile, `${scenario} blank`);
        expect(median(scenario).blankFrames).toBe(0);
        expect(median(scenario).whiteFrames).toBe(0);
      });
      test(`${scenario}: nothing moves after its first frame (layout shift < ${SHIFT_BUDGET})`, () => {
        known(profile, `${scenario} shift`);
        // A sort tab's answer is the list in its new order, a frame after the
        // tap: as CLS does, shifts that answer input don't count there.
        const m = median(scenario);
        expect(scenario === "sort tab" ? m.cls : m.shift).toBeLessThan(SHIFT_BUDGET);
      });
    }
    for (const scenario of BACKS) {
      test(`${scenario}: the reader's card is back within ${RESTORE_BUDGET_PX} px`, () => {
        known(profile, `${scenario} restore`);
        expect(median(scenario).restorePx).toBeLessThanOrEqual(RESTORE_BUDGET_PX);
      });
    }
    test(`scrolling keeps its frames (95th percentile at most ${SCROLL_P95_MS} ms, at most ${SCROLL_DROPPED * 100}% dropped)`, () => {
      known(profile, "scroll", true);
      const runs = got().scroll;
      expect(middle(runs.map((r) => r.p95)), "the 95th percentile of a scroll's frame intervals, in ms").toBeLessThanOrEqual(SCROLL_P95_MS);
      const dropped = runs.map((r) => r.dropped / (r.intervals.length + r.dropped || 1));
      expect(middle(dropped), "the share of a scroll's frames dropped").toBeLessThanOrEqual(SCROLL_DROPPED);
    });
    test("every press answers in its first frame, and moves nothing", () => {
      known(profile, "press");
      const late = got().presses.filter((p) => !p.changed || p.moved).map((p) => `${p.control}${p.moved ? " (moves)" : ""}`);
      expect(late, "controls with no press feedback in the first frame").toEqual([]);
    });
  });
}
