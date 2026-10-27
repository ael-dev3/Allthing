<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Performance

Navigation should feel instant: a post opens as soon as it is clicked, the feed keeps scrolling without waiting, and nothing a reader does takes long enough to notice. This page sets the budget, explains how the app meets it within the Workers Free plan ([BUDGET.md](BUDGET.md)), and records measurements.

## Budget

| Metric | Target | Measured by |
|---|---|---|
| Opening a post from the feed (click → post text on screen) | **< 100 ms** | `e2e/perf.spec.ts` |
| Largest Contentful Paint, home, cold load | **< 1.5 s** (median of 3) | `e2e/perf.spec.ts` (the later of the LCP entry and the first cast card) |
| Interaction to Next Paint (slowest interaction) | **< 100 ms** | `e2e/perf.spec.ts` (Event Timing entries with an `interactionId`) |
| API response from the edge cache | < 50 ms server time | the `Server-Timing` header of `x-allthing-cache: fresh` responses |
| Smoothness: taps, navigations and presses at 390 and 1280 px | INP < 100 ms, no long task, no blank frame, layout shift < 0.01, Back within 2 px, a press shown in its first frame | `e2e/smoothness.fixture.spec.ts` ([Smoothness](#smoothness)) |

The CI `perf` job runs `e2e/perf.spec.ts` against https://allthing.social after every push to `main` (and on demand through `workflow_dispatch`). Before measuring, it waits for `/version` to report the checked-out commit (the triggering commit for a push, or current `main` for a manual run), or a later commit on `main` that includes it. Workers Builds skips a commit that a newer push supersedes: of five merges made within 33 seconds on 19 October 2026 it built the first and the last, so a commit can be live only as part of a later one, which the job tells from the repository's history. It polls every 10 seconds for at most 30 minutes, aborts an individual version read after five seconds, and fails without benchmarking if that release never arrives. Workers Builds takes 6 to 16 minutes from a push until the commit is live, and about 25 when a second merge queues behind the first: the five-minute wait it replaced gave up 21 seconds before #144 went live on 19 October 2026, and from its introduction (#133) the job failed on every push to `main` it ran for, without measuring anything. This keeps a pending deployment from passing the budget with the previous release's results.

Each cold-home sample logs its first-card and LCP times, the LCP element and image dimensions/loading hints, navigation and resource phases, cache/Server-Timing metadata, and main-thread long tasks. The JSON attachment `home-lcp-diagnostics.json` is retained in the `playwright-performance` Actions artifact for seven days, even when readiness or the budget fails. URLs retain only origin and pathname; credentials, query strings, fragments and Server-Timing descriptions are omitted. Cross-origin timing phases can be zero when the image host sends no Timing-Allow-Origin header. These observations add no warm-up fetch, wait, cache override or resource priority change: the same three cold contexts and 1.5-second median budget apply.

The post-opening check also retains `post-open-diagnostics.json` for each attempt, including failures, in that artifact. It records the selected cast's hover and completed resource phases, up to 64 existing measurement frames, 64 overlapping long tasks, 32 long animation frames with four scripts each, and 128 resources, prioritizing the selected post and its code. Counts report omitted entries; in-flight or evicted resource entries are unavailable. URLs omit credentials, query strings and fragments. Collection and sanitization finish after the original assertions; the probe adds some instrumentation cost but no extra frame loop, request or readiness wait. The same third card, 600 ms hover, click capture, post-text frame check, replies assertion and strict 100 ms limit apply.

## Smoothness

CI runs functional fixture files with two workers, then runs `smoothness.fixture.spec.ts` alone with one worker (`cargo xtask browser-fixtures`). Both phases run even if the first fails, and either failure fails the existing browser job. Their HTML reports and test results are kept separately, including from the later static checks; the production performance job, test retries, timing assertions and 45-minute job cap are unchanged.

Before installing Playwright’s dependencies, GitHub Linux runners use `cargo xtask browser-mirror` to replace active Azure Ubuntu archive URLs with Canonical’s HTTPS archive in the supported Ubuntu APT mirror configuration. The native tool preserves the remaining configuration, including package suites, components, security URLs and signing settings; the existing `playwright install --with-deps chromium` still installs all dependencies and fonts. A recorded runner fetched 25.9 MB through Azure’s mirror in 10 minutes 11 seconds, versus one second for the same package set on an earlier run, leaving too little time for the complete suite. Mirror selection addresses that acquisition path without increasing the job cap or changing test limits. Future mirror throughput remains external, so a complete normal CI run is still required.

How the app answers a reader's taps, clicks and scrolls: `e2e/smoothness.fixture.spec.ts`, which CI's browser job runs on every pull request and push to `main`, on the local build with the Worker's API, the node and the images as stand-ins (the API answers after 60 to 180 ms). A signed-in reader with an approved key, on a phone (390 × 844, touch, the CPU slowed 2×) and on a computer (1280 × 900, a mouse):

- taps the sort tabs: new, top, hot;
- opens a likes count (a tap previews who liked; a click opens the whole list);
- likes a cast, unlikes it and likes it again;
- opens the composer, then in it the emoji picker and the sticker picker, and types a key at the end of a draft a line long;
- opens a cast from the feed, and a profile, from three places in the feed, and goes Back each time (on the phone a tap on a name opens the profile card, and its "View profile" the profile);
- goes Back to a cast deep in the feed, in its second server page;
- scrolls the feed 6,000 px, past the cards drawn first, so the list draws more as it goes;
- presses 22 controls and lets go away from them: on the front page a sort tab, the time range, Cast, a like, a recast, a likes count, a replies count, the account's picture, an author's name and a link card; in the composer its emoji, sticker, schedule and close buttons; on a post Reply, Quote, React and a replies sort tab; on a profile a tab, Favorite, Show less and Follow.

A finger stays down 80 ms, and the mouse rests on its target for 250 ms and holds its button down 80 ms as it clicks, as readers' do. Each interaction runs three times, with the garbage of what came before collected first, and twice more when the median is within a fifth of a timed budget (INP, long tasks), so that five runs, not one noisy one, decide a close call. The budgets hold for the median:

| Check | Budget | How |
|---|---|---|
| Interaction to Next Paint | < 100 ms | the interaction's slowest Event Timing entry |
| Long tasks | none | no `longtask` or `long-animation-frame` entry (> 50 ms) overlapping the interaction, from its input until its handlers are done |
| Layout shift on navigations | < 0.01 | every `layout-shift` after the navigation's first frame, whether the browser counts it as answering input or not; on a sort tab, whose new order is its answer, only those CLS counts |
| Blank frames | none | each frame of a navigation, sampled by `requestAnimationFrame`: whether any of 15 points on the page's main content shows something readable (at least half opaque) or a placeholder, or something over it, and whether the page is white |
| Back | the reader's card within 2 px | the card at the line `nav.rs` reads the reader's place at, before and after |
| Press feedback | shown in the first frame, nothing moved | the computed transform, scale and opacity of the control and what it holds, in the first frame after `pointerdown`, and its laid-out box; pressed once it has stayed put for 200 ms with the images on screen loaded, as a reader presses what they see, since a scroll to it can bring photos above it into view that take their shape a moment later and move it |
| Scrolling | 95th percentile frame ≤ 20 ms, ≤ 5% of frames dropped | the intervals between `requestAnimationFrame` callbacks through a 6,000 px scroll at 2,500 px/s (a finger's on the phone, the wheel's on the computer), the median of three scrolls; a frame dropped is a 16.7 ms interval missed |

It also reports, with no budget yet: when a navigation first shows the next page, the longest task and frame in the second after an interaction, and frames that show placeholders. `SMOOTH_OUT=numbers.json` writes every run's numbers, and `SMOOTH_TRACE=trace.zip` records a Playwright trace of the phone's run.

A budget the app doesn't meet yet runs as an expected failure (`NOT_YET` in the spec, with what it measured), so CI stays green and the change that meets it takes its line out. One it misses or only just meets, by less than a busy machine's noise, would pass now and then as an expected failure and fail CI, so it doesn't run until a change meets it with room to spare (`TOO_CLOSE`).

**A busy machine.** The timed budgets (INP, long tasks) hold when the run has a CPU to itself, as on a CI runner. Before each interaction the spec times how much of a CPU a busy thread gets (a 300 ms spin's CPU time over its wall time), and reports the median with the load average. On a 4-core machine with other builds and browsers at load 9, a thread got 0.4 to 0.6 of a CPU, and the phone's sort tab, 40 to 56 ms in the calibration below, answered in 104 ms; at 0.8 the computer's took 88 ms. So away from CI, below 0.9 (a tenth more for every latency, enough to take the slowest interaction that meets its budget over it) the timed budgets are skipped, saying why, and their numbers are still reported; in CI, or with `SMOOTH_STRICT=1`, they're judged however busy the machine is.

**Why the phone's CPU is slowed 2×.** Slowed, it stands for a phone slower than a CI runner, as most readers' are; but a budget only means something if CI can tell a slower app from a busy runner. Calibrated on 21 October 2026 on a 4-core machine shared with other builds and browsers (load average 2 to 11), each figure the median of three, from two to six runs:

| Phone CPU | Sort tab | Likes popover | Like | Composer | Open a cast | Open a profile |
|---|---|---|---|---|---|---|
| 1× | 32–56 ms | 40–112 ms | 40–56 ms | 48–64 ms | 96–104 ms | 80–128 ms |
| 2× | 40–56 ms | 64–88 ms | 48–64 ms | 48–88 ms | 136–216 ms | 128–232 ms |
| 3× | 48–72 ms | 96 ms | 48–64 ms | 80–88 ms | 184–192 ms | 176–256 ms |
| 4× | 80 ms | 96–160 ms | 64–96 ms | 88 ms | 216–264 ms | 304–312 ms |

At 2× what the app answers in time stays under the budget even on a busy machine (40 to 88 ms), and what it doesn't (opening a cast or a profile) misses it by far. At 3× and 4× (4× is the speed report's phone) the likes popover and the composer reach 80 to 160 ms, too close to the budget, or over it, to tell a regression from a busy machine. CI's runners are about as fast as that machine: `e2e/perf.spec.ts`'s interactions on production measured 56 to 112 ms on four of them on 21 October, and 48 to 56 ms on it when quiet (136 ms under load).

**Before, 21 October 2026** (main at 32407b7, three runs, and at 87beef5, one; each the median of three, or of five where close to a budget):

| Phone, 390 px, CPU 2× | INP | Long task | Blank frames | Next page shown | Back |
|---|---|---|---|---|---|
| Sort tab | 40–56 ms | none | 4–6 | | |
| Likes popover | 64–88 ms | none | 0 | | |
| Like | 48–64 ms | none | 0 | | |
| Composer | 48–88 ms | none | 0 | | |
| Open a cast | 136–216 ms | 93–168 ms | 5–6 | 190–258 ms | |
| Back from it | | 110–132 ms in the next second | 2 | 245–392 ms | card within 1–2 px |
| Open a profile | 184–232 ms | 136–206 ms | 4–6 | 230–283 ms | |
| Back from it | | 178–260 ms | 2 | 363–409 ms | within 1 px |
| Back deep in the feed | | 166–245 ms | 2 | 276–455 ms | within 0–1 px |

| Computer, 1280 px | INP | Long task | Blank frames | Next page shown | Back |
|---|---|---|---|---|---|
| Sort tab | 24–32 ms | none | 4–8 | | |
| Likes list | 40–48 ms | none | 0 | | |
| Like | 24–32 ms | none | 0 | | |
| Composer | 48–56 ms | none | 0 | | |
| Open a cast | 64–96 ms | 0–81 ms | 5 | 109–139 ms | |
| Back from it | | none in the next second | 6–7 | 146–179 ms | card within 1–2 px |
| Open a profile | 64–96 ms | 50–72 ms | 4–6 | 106–141 ms | |
| Back from it | | 0–55 ms | 5–7 | 145–194 ms | within 1–2 px |
| Back deep in the feed | | none | 5–7 | 177–190 ms | within 1–2 px |

- **No control answers a press in its first frame**, of the nine, at either width: a touch screen never shows `:active` there, and where a control has a press style (Cast, the like icon) it eases in over 200 to 300 ms, so its first frame is unchanged.
- **Every navigation has frames with nothing to read**: the page drawn anew fades in from transparent (`.page > *`), and a list's cards rise in from transparent, one after another.
- **Opening a cast or a profile draws the page in the tap's own task**: 93 to 206 ms on the phone, most of its INP, and 0 to 81 ms on a computer, which sometimes takes it over the budget there too.
- **Back draws the feed again all at once**, its 25 to 75 cards in one task: 110 to 260 ms on the phone, and the feed shows 245 to 455 ms after Back.
- **Nothing moves after a navigation's first frame, and Back puts the reader's place back** within 2 px, even after the feed's second server page.
- **Scrolling** keeps its frames: 16.7 to 16.8 ms at the 95th percentile, at most 7 of 166 frames dropped.

**Press feedback** (`press.rs`, since 21 October 2026). All 22 controls answer in the first frame after the press, at both widths, and none moves: that frame came 18 to 69 ms after `pointerdown` on the phone (median 28 to 32 ms, the CPU slowed 2×) and 3 to 68 ms after it on the computer (median 9 to 11 ms), on a machine at load 8 to 12. The taps answer as they did, alternating this build and the one before it, 12 to 16 runs each on a quiet machine (medians, before in brackets): on the phone the sort tab in 60 ms (68), the likes popover in 76 (84), a like in 56 (64), the composer in 88 (72); on the computer, its button held down 80 ms as a reader's is, in 44 (44), 40 (44), 48 (48) and 56 ms (56). The spec's click, down and up at once, waits for the frame that shows the press: a like there took 64 ms against 36 (6 runs). The release starts once the frame that answers the press is painted: started with the click, it played on a sort tab the click was drawing anew, and that answer came later (median 76 against 44 ms, 12 to 14 runs on a busy machine).

**After perf/instant-nav, 21 October 2026** (three runs on a machine at load 5 to 8; each the median of three, or of five where close to a budget; before in brackets). A page opens once the press has painted ([client-side navigation](#3-client-side-navigation)), the front page is kept while another page shows, nothing comes in from transparent, and Back puts the card back within a pixel of its laid-out place:

| Phone, 390 px, CPU 2× | INP | Long task | Blank frames | Next page shown | Back |
|---|---|---|---|---|---|
| Sort tab | 64–80 ms (40–56) | none | 0 (4–6) | | |
| Open a cast | 48–72 ms (136–216) | none (93–168 ms) | 0 (5–6) | 52–59 ms (190–258) | |
| Back from it | | none in the next second (110–132 ms) | 0 (2) | 41 ms (245–392) | card within 0 px (1–2) |
| Open a profile | 88–112 ms (184–232) | 0–70 ms (136–206) | 0 (4–6) | 57–73 ms (230–283) | |
| Back from it | | 65 ms (178–260) | 0 (2) | 59 ms (363–409) | within 0 px (1) |
| Back deep in the feed | | none (166–245 ms) | 0 (2) | 45 ms (276–455) | within 0 px (0–1) |

| Computer, 1280 px | INP | Long task | Blank frames | Next page shown | Back |
|---|---|---|---|---|---|
| Sort tab | 32–48 ms (24–32) | none | 0 (4–8) | | |
| Open a cast | 32–48 ms (64–96) | none (0–81 ms) | 0 (5) | 20–23 ms (109–139) | |
| Back from it | | none (none) | 0 (6–7) | 23 ms (146–179) | card within 0 px (1–2) |
| Open a profile | 40–64 ms (64–96) | none (50–72 ms) | 0 (4–6) | 20–25 ms (106–141) | |
| Back from it | | none (0–55 ms) | 0 (5–7) | 27 ms (145–194) | within 0 px (1–2) |
| Back deep in the feed | | none (none) | 0 (5–7) | 13 ms (177–190) | within 0 px (1–2) |

- **Opening a profile on the phone is still close to the budget**: its first tap opens the account's card, drawn in the tap's own task (`popover.rs`), and only the card's "View profile" opens the profile, so it doesn't run (`TOO_CLOSE`).
- **Back to the front page draws nothing**: the page is put back as it was (`kept.rs`), its cards where they were. What remains in the second after Back is the page left being taken down (a profile's, 65 ms on the phone).
- **Opening a post still draws it in one task**, after the press's frame: 82 to 151 ms on the phone, where the reader sees the card held, then the post.
- **What the computer's figures measure**: since this change the spec's mouse holds its button down 80 ms as it clicks, as a reader's does; before, its click went down and up at once.

## How the app meets it

### 1. Reuse what the page already has

- Every cast that a list renders, and every cast it quotes, goes into a client-side store (`api::remember_casts`, at most 2,000 entries).
- `/p/<hash>` reads that store first (`api::seen_cast`). If the cast is there, the post renders at once with its text, media, author and counts, the quoted casts the feed carried, and reply skeletons. The full payload (thread, the casts the reply answers, quotes) loads behind it and replaces the skeletons. A reply with a parent shows one placeholder card above it, which the chain replaces. The chain is read in turn on the Worker, so a deep chain adds one upstream round trip per cast above the reply (up to eight) to a cold build; the payload stays cached as the cast's is.
- A direct visit with nothing in the store falls back to the normal one-request load.
- `/u/<name>` does the same for profiles. The store also keeps each cast's author by username (`api::seen_person`), so a profile opened from a feed shows its header at once: the picture, names and fid, and, from the author's profile card (`api::seen_card`), the start of the bio, the counts and the join date (`allthing_core::profile::ProfilePreview`). Its casts load behind it. Without a card, the bio and counts are placeholders; a join date that the profile would correct (an account from before November 2023, whose card can't date it) is left out until the payload arrives.
- **Profile cards** open with what the page's payload already says about the account (`notes.users`: the start of its bio, its counts, when it joined), or what an earlier page of the visit showed (`api::seen_card`), so they show at once. The vote and wallet activity are preloaded for the accounts a reader is likely to open (the authors of feed cards in view, the people of a [preview](#popover-previews) the reader comes into) and read synchronously, so the card shows them in its first frame. On a touch screen the same holds for a card tapped open: its authors' facts load for cards the reader stops at (still in view after 600 ms), 6 authors per 5 minutes at most, against 12 with a mouse. A card whose vote or wallets are still on their way waits for them out of sight (`settling`), for half a second at most; a read slower than that shows its placeholders, which are the size of what replaces them, so the card never grows when they come (it grew by 27 px for two chains, and by 115 px for five chains and a note, before 19 October 2026). Either answer is reused for 5 minutes.

- **A page's data is requested by a one-line inline script** at the top of `index.html`, so it starts while the HTML is still being parsed, alongside the WebAssembly download. `api.rs` (`early_response`) takes that pending response instead of fetching again.
  - It covers the pages people arrive on: the front page (its sort and time range), `/p/<hash>`, `/u/<name>` and `/c/<id>`. It normalizes the hash, name or id as the app does (lowercase; an `0x` or `@` prefix), so the paths match; anything it doesn't recognize, such as a name with other characters, is left to the app.
  - On production the post, profile and channel requests used to start only once the app had booted, about 450 ms in (the front page's always started at once). The response is still the page's one API call.
  - The script is allowed by its hash in the CSP (`web/_headers`), and `cargo xtask check-repo` fails if the two disagree.
  - Starting the request from `boot.js` was tried first, but a module script has to download before it runs, so the request still began about when the app's own JS arrived (700–990 ms on production).
  - The static copy, served under `/Allthing/`, skips it.
- **The front page's first cards are drawn from that data before the app has come** (`web/paint.js`). On a slow line the data comes seconds before the WebAssembly (on the historical speed report's phone, at about 1.0 s against 3.1 s), so a small script draws the cards the app draws first: in the app's order (hot or top, with the reader's favorites and those they see less of), with the app's markup and the usage bar above them, under the boot screen's header. They only show: they're inert and hidden from screen readers. Once mounted, the app is drawn hidden under them, and when its own cards are drawn, in the same boxes, they take their place (`boot.rs`), without their entrance animation again. The [speed report](#speed-report) times both: the first cast on screen, and the app's own first.
  - Only the front page, for hot and top over the past hour, 6 hours, 12 hours, day or week, and only from a fresh answer (not one the Worker marks stale). At most six cards at 1280 px and above, or four below it, where the app inserts its poll. Quotes use the app's depth limit; a video, sticker or single image it cannot size stops the prefix. Following, new, client filters, long ranges and browsers with local seen history remain with the app.
  - A single image's tile has its final box before the image comes. Cloudflare Images describes the variant a feed shows (`…/fit=scale-down,w=720,format=json`, about 110 bytes, sent with `access-control-allow-origin: *`), the tile takes that size, and so does the app's tile for the same image (`media::known_size`). A card with one is drawn once its description has come, a round trip after the data. Nothing on the first screen moves as its images come, there or after the app takes over; without this, a single image grew its card when it loaded, pushing the cards below it down.
  - Its images (avatars, emoji, link thumbnails and media) wait until the WebAssembly is in, and the app's own cards then find them on their way. Asked for at once, in a trial on the speed report's phone, they shared the slow line with it and the app's first card came 1.4 s later.
  - If the app can't load, `boot.js`'s panel replaces the cards. Leaving the front page, or its data failing in the app, hands over at once, and the app's cards replace them at most 4 s after the app has mounted, whatever else.
  - It adds no request to the Worker. The static copy doesn't load it. Historical JavaScript size and timing measurements below describe their recorded builds.

#### Measured exception: the JavaScript first-card renderer

The Rust-first tooling work also tried an independent Rust Wasm first-card renderer. The owner chose to keep `web/paint.js` after the JavaScript version was faster in a paired cold local comparison. Three alternating rounds per profile used the same synthetic 40-cast Home response and Brotli delivery; medians, rounded to milliseconds:

| Measurement | Retained JavaScript renderer | Rust Wasm prototype |
|---|---:|---:|
| Desktop first card (1280 px, 10 Mbps, 40 ms RTT, native CPU) | 215 ms | 376 ms |
| Phone first card (390 px, 1.6 Mbps, 150 ms RTT, 4× CPU slowdown) | 944 ms | 2,027 ms |
| Phone app's own first cards visible after handover | 4,009 ms | 4,540 ms |

The JavaScript baseline used preserved pre-migration local assets; it was not independently rebuilt from an exact source commit. These are local prototype results, not production release evidence or a general Rust-versus-JavaScript benchmark. The Rust prototype needed an additional Wasm/glue download; the retained renderer does not. This supports keeping this measured exception, not a claim that native Rust tooling improves browser startup. Preserve the existing early-renderer regression tests and require matched performance evidence before revisiting it.

#### Retained Node static test server

The small `e2e/pages-server.mjs` and its three existing Node oracles remain. In [migration CI 40232102342](https://github.com/ael-dev3/Allthing/actions/runs/40232102342), the first static poll fixture at 1280 px stalled on the boot screen before the app mounted, then passed on retry. That first attempt remains rejected. The pending resource was not recorded, so the cause was not established. Keeping the existing test server is a conservative rollback while startup diagnostics improve, with no claim that Rust caused the stall or that the rollback improves performance. The existing assertions and their 25-second expectation deadline remain.

### 2. Prefetch, under a strict budget

- **Hover or touch** on a card prefetches `/api/v1/cast?hash=…` (`pointerenter`, `touchstart`).
- **Links** to a post or a channel prefetch the data of the page they open as they're pressed, touched or focused, and once a mouse has rested on one for 100 ms (`split.rs`, `prefetch_page_data`), so a mouse passing over a link asks for nothing. Not a profile's: a name pointed at or tapped opens the account's card, which never reads the profile's payload, and a profile opened from a feed shows its header at once anyway. On a phone the press comes about 80 ms before the tap that opens the page, and a mouse rests on a link for a few hundred ms before its click.
- **Viewport**: a shared `IntersectionObserver` notices cards that are at least 60% visible for 600 ms. It then prefetches them in idle time (`requestIdleCallback`), at most 6 per minute.
- **Speculation Rules** (`web/index.html`) let supporting browsers prefetch `/p/*` documents with `moderate` eagerness (on hover). The document is the static app shell, so this costs no Worker request. The CSP allows it with `'inline-speculation-rules'`.
- **Limits**, in `crates/allthing-web/src/api.rs`:
  - at most 2 prefetches in flight, [popover preview](#popover-previews) batches included;
  - duplicates are skipped, both in flight and when already cached;
  - a page request that matches an in-flight prefetch waits for it instead of fetching twice;
  - nothing is prefetched with `Save-Data` or on `2g`/`slow-2g` connections;
  - the static GitHub Pages copy, which has no shared cache, never prefetches from the viewport.
- **Cache**: answers stay in a client cache for 5 minutes, up to 200 entries shared with static text. On reaching capacity, expired entries go first, then the oldest remaining answer. Back, forward and reopened posts reuse retained answers; an evicted page is requested again.

#### Popover previews

Who liked and who recasted a feed card, and its replies, show the moment their popover opens: the feed preloads them (`crates/allthing-web/src/preload.rs`).

- **Cards in view.** The shared observer above reports cards coming into view and leaving it. Once the cards in view have stayed put for 200 ms, the topmost one whose batch this page hasn't asked for brings it.
- **Then the next run, ahead of the reader.** Once the runs in view have their batches, the run after the last card in view brings its own when its first card is less than a screen below the viewport, so a reader reading down finds its popovers already loaded. It waits until all six of its cards are on the page: a run the next page is still to fill would be asked for twice. A run the reader scrolls past before its turn isn't loaded; its popovers load when they open.
- **Batches follow the list.** A card brings its run of six cards, in list order (cards 1–6, 7–12, …), as one `/api/v1/engagement` request, with a list only for counts above zero, and the replies of the cards with 1 to 100. Readers of the same feed ask for the same batches, so the edge cache answers most of them, and upstream reads don't grow with readers.
- **Filed as single answers.** Each list goes into the client cache under its own `/api/v1/reactions?…&limit=10` path, and each card's replies under its own `/api/v1/replies?hash=…`, with the batch's arrival time and age, so it expires, and reports its age, as if it had been asked for alone. An answer already kept that was read upstream later stays. A popover that opens while its batch is on its way waits for it instead of asking again, and a list or replies the batch lacks (its read failed, the Worker kept Hypersnap's bucket for page builds, or the replies didn't fit in the batch's share of CPU) loads when its popover opens, as before.
- **No "Loading…" flash.** A preview reads the client cache synchronously, so its people or replies are there in the frame the popover opens, hovered or tapped. A resource would resolve a tick later, even from the cache.
- **Replies** come with the batch, built as `/api/v1/replies` builds them: the ten most engaged direct replies, without spam or mini apps. Hypersnap sends a conversation with every direct reply, about 1 KB a reply, and a reply is parsed as a whole cast, about 2.5 times a reaction list's cost a byte, so a batch parses its cards' replies only within what its lists leave of its share of CPU, the smallest first, and the client doesn't ask for those of a card with over 100 ([BUDGET.md](BUDGET.md#4-cpu-time)). Replies that link to a page not checked yet for a mini app wait for it, as `/replies` does: the batch comes up to 2.5 s later, once a day per link in an isolate, and a popover opened meanwhile waits for it. A card whose replies didn't come takes them from the post payload that hovering it prefetched, whose thread holds the direct replies, or loads them when the popover opens.
- **The people in a preview.** As a likes, recasts or replies preview's list opens, the vote and wallet activity of everyone it names load (`api::prefetch_row_facts`), so the card a name opens is complete. A list opens on a tap, or once the pointer has rested on its count for 280 ms, so a count the pointer only crosses loads nothing. They go ahead of feed authors' in the card queue, in the list's order, two in flight with the other prefetches. The name the reader points at or touches goes first (`api::prefetch_facts_first`), about 100 ms before its card opens, and outside the 5-minute count, as the card would read its facts then anyway; one opened before its turn reads its own at once. Anyone whose facts are kept or on their way is skipped, and at most 30 people (three lists) are queued per 5 minutes (60 requests, both endpoints edge-cached for an hour or more). The full Liked by and Recasted by lists, up to 100 people, aren't preloaded: their cards read their facts when they open.
- **Limits**, per page view (a new URL, or a list rendered anew, as after Back):
  - one batch on its way at a time, and batches start at least 2 s apart;
  - at most 8 batches, 48 cards, about the front page's first two pages; later cards load their previews when they open;
  - batches share the prefetch cap (2 in flight);
  - nothing is preloaded with `Save-Data` or on `2g`/`slow-2g` connections;
  - nor in the static copy, whose cards aren't observed.
- **Why these numbers.**
  - A batch is at most 18 Hypersnap reads (six cards, likes, recasts and replies): 20 subrequests with the cache read and write, 26 with the replies' link pages, against a budget of 45. Its parsing stays where twelve full lists took it, about 3 ms natively: its replies get only what its lists leave ([BUDGET.md](BUDGET.md#4-cpu-time)). A typical batch of the day's trending casts went from 0.8–1.2 ms to 1.3–1.9 ms with its replies.
  - An isolate's Hypersnap bucket holds 40 tokens and refills 10 a second. One reader's batches, at most 18 reads every 2 s, ask for 9 a second at most, so even if every batch missed the edge cache and reached the same isolate, a scrolling reader couldn't drain it. Eight batches cap a page view at 144 reads.
  - The Worker adds its own floor: previews never take the bucket below 16 tokens, so page builds keep a share when many readers' batches meet in one isolate. A batch with less room reads what fits (likes first, then recasts, then replies), is kept for 15 s, and is rebuilt.
- **Cost**: one Worker request per batch, so a feed costs one to eight more requests (usually one to three), with a mouse or on a touch screen, while hovering or tapping a preloaded count costs none, a replies count included. See [BUDGET.md](BUDGET.md).

### 3. Client-side navigation

- Links are handled by the Leptos router, so there are no full reloads.
- Clicking anywhere on a card navigates (`nav::card_click`), unless the click lands on a link or button, or text is being selected. A click on the card of a cast it quotes opens the quoted cast instead. Ctrl-, ⌘- and shift-click open a new tab as with any link.
- **The press first, the page after.** A link or a card opens its page once the press has painted, not in the press's own task (`nav::go`; `InstantLinks` takes every click on a link the router would follow to another page, and leaves it one to the same page with another query, as a sort tab is, which it answers in the same task). Drawing a post or a profile, and tearing the feed down, took 93 to 206 ms of the tap's task on the phone profile, so opening one answered in 136 to 232 ms (INP); now the link or card shows pressed in the next frame (`data-press="held"`, a card tinted) and the page follows.
  - A page with something to show opens then: its data kept, or a post or a person a page has shown ([above](#1-reuse-what-the-page-already-has)). One with nothing yet (a channel, or a profile only linked to) waits for its data, which the press asked for (`api::fetch_ahead`, past the prefetch cap), with the page left on screen and the press held, so it opens complete; after 250 ms it opens anyway, with its placeholders. A page whose code is its own waits for that too, which the press also asks for (`split::page_came`): a channel's data that came first opened it on its code's placeholder for 1 to 6 frames. Another press, Back or Forward cancels one still waiting.
  - The part of a page still to come (a post's replies, a profile's casts) shows its placeholders only after 250 ms (`.later`), so a part that comes sooner just appears.
  - A post's first replies, about a screen of them (six with those under them, `FIRST_REPLIES`), come with the post, and the rest a frame after (`pages/post.rs`): drawn with it, they held the frame that showed it for 82 to 151 ms on the phone profile. Only those out of view wait, so the page's foot doesn't move when they come. Back draws them with the post, since the reader's place may be among them (`nav::restoring`).
- **Entrances that never hide the page.** What a page draws anew (a post, a list's cards) rises 8 px into place on a spring-like curve (`--spring`), readable from its first frame. The page faded in from transparent (`.page > *`) and a list's cards rose in from transparent, one after another, so every navigation showed 2 to 8 frames with nothing to read (the smoothness spec's blank frames); now none. There's no view transition between pages: tried with this change (the page left crossfading into the next as it rose into place, 220 ms), it showed the next page 450 to 740 ms after the tap on the smoothness spec's profiles, against 18 to 72 ms without it, both pages captured before it starts. The one card clicks used until 18 October 2026 waited for an animation frame in its update, which doesn't come during a transition, and hung for Chrome's 4 s timeout.
- **Scroll**: `history.scrollRestoration` is `manual`, and `nav.rs` (`ScrollMemory`) restores each page's place on back and forward.
  - **What it saves**: the card under the line just below the header and where its box is laid out in the viewport (without the card's own transform, so a hovered card's 1 px lift or its entrance doesn't move the place kept), plus the pixel position for pages without cards. It saves on scroll and, in the capture phase, on every click, so that the scrolls a navigation causes don't overwrite it.
  - **What comes back**: the front page itself, kept while another page shows (`kept.rs`): Back puts the same cards back, where they were and each the height it had, with nothing drawn or asked for again; out for over 5 minutes, it's drawn anew from the client cache, with the later trending pages it had loaded. Other lists show as many cards as before (`CastList` remembers per page).
  - **How long it holds**: after Back, the card is put back, within a pixel, and held there for 6 s against late changes (images loading above it, lists growing back, a late scroll-to-top). The reader's own wheel, touch, key or mouse input ends the hold.
  - **Images keep their size**: a single image's natural size is remembered once it has loaded, and the image is drawn at that size when the list is rendered again. Back therefore doesn't make the page grow while images arrive, including after the hold ends. A single GIF, drawn at most 300 px each way (70% of the card's width on a phone), takes its smaller box at once the same way.
  - Sort tabs and reply toggles use `data-noscroll`, so they never jump to the top.
- **Images have their boxes before they come.** An image a cast shows alone takes its own shape, so the page can't lay it out until it knows the image's size. The browser asks Cloudflare Images for it (the inline variant's description, about 110 bytes; `media::size_up`) as a list shows its cards and as a post opens, each image once a visit and six at a time, and an image hosted elsewhere that is about to show is read until its first bytes tell its size. A list draws its next screenful of cards once their images' sizes have come, or a second has passed (`CastList`, `media::sizing`), where the list ends in view: further down, cards come at once, and their images' sizes long before the reader does. The screenful comes together: drawn one card at a time, a short list let the footer into view and out again. A post's page, and a channel's with a pinned cast, keep their placeholders until the images at their top have their sizes. Before, a post loaded cold showed its replies, then pushed them most of a screen down as its image came, and after another time range the front page's cards jumped down 300 to 450 px at a time. A size still unknown when the wait is over holds a square, the shape least wrong on average, and an image that can't load shows its alt text. The sizes cost no Worker request; each answer is cached for two days.
- **Sorting** reorders the existing list (a keyed `For` over a derived signal) instead of rebuilding the page. A post's replies' sort tabs redraw the replies only, not the post (the whole page was drawn again: 192 to 224 ms at 390 px with the CPU slowed 4×), and take each other's place in the history, so Back leaves the post.
- **Drawing a screenful at a time.** A list rendered anew (a sort tab, another page) draws 6 cards, about a screenful, in the task that asked for it, and 6 more in a task after each paint until it has drawn what it shows (`CastList`, `DRAW_STEP`). A re-sort that changes the top of the list starts again from there. The tap's frame paints at once: building all 25 cards in the click took 13–39 ms of WebAssembly on a fast desktop CPU, which put the slowest interaction over 100 ms on CI runners. Back and forward draw the whole list at once while they restore the reader's place (`nav::restoring`).
- **The tap first, the feed after.** A sort tab answers in the frame after the tap: the tab shows as chosen at once, and the feed follows in a task of its own once that frame is painted (`after_a_paint` in `pages/home.rs`), then draws a screenful at a time as above. Switching between hot and new swaps the whole feed, tearing down the old cards and building new ones. On a phone with the CPU slowed 4×, that work made the tap wait for its next frame. New also read its client filter inside its view. A tap that changed only the sort changes the query too, so new redrew its 25 cards just before they were torn down. The filter is now a memo of its own. Measured on 20 October 2026 as the speed report measures, with the two builds served locally on the same data, alternating, median of 9:
  - the slowest of the new and hot taps went from 144 ms to 48 ms (40–56) on the phone profile;
  - at 1280 px it went from 56 ms to 32 ms (median of 7);
  - the first cast came 7 ms later (4,003 → 4,010 ms), the time the module's extra 0.7 KB takes.

### The composer and its pickers

- **The emoji picker a group at a time.** It draws the groups in view as it opens (about 48 emoji) and the rest one a frame after (`draw_rest`), and an emoji's picture loads as it comes into the picker's view, a row or so ahead (`load_in_view`, as the stickers already did). It drew all 219 in the click that opened it, which took 280 to 344 ms on a computer in the polish audit, and loaded all their pictures, 150 to 400 ms of the page's time on the phone profile for emoji mostly never seen. A group not drawn yet that the reader jumps to is drawn at once.
- **The composer opens in one frame under 50 ms on the phone.** Cast, Reply and Quote open it in the press's own task, so the box has focus, and a phone's keyboard is held for it, in the first frame, with the bar. In that task, with the CPU slowed 2×, a trace (Chrome's `Tracing`, main at ea312b1) found 46 to 59 ms of main-thread work, median 49, and the frame around it over 50 ms in most opens (the smoothness check's median 51 ms here, 51 to 56 ms in CI): the press's other listeners 2 ms, taking the draft up 4 ms, the dialog's frame and text box 7 ms, the bar 14 ms (2 of them starting its two asks of the Worker, the GIF key and whether files can go, and 3 to 4 more answering them from the client cache), focusing the box 10 ms, of which styling and laying out the new dialog took 5 to 6, and what's under the text (the GIF, the pickers, the time, the notes) the rest. Now what's under the text comes in a task of its own once that frame is painted, unless the draft has a GIF, files or a time to show there; the bar asks the Worker then too, and shows what it was told last meanwhile (the GIF key and the limits are kept from one opening to the next), so it is drawn as before; and the header stands still under a dialog from the frame after it opens (`header-still`), not by `html:has(.modal-backdrop)`, which had the browser style the page's root, the line and the wordmark again as the dialog came in: the dialog's style took 1.8 to 2.2 ms for 34 elements instead of 3.3 to 4.1 for 45. The task is 36 to 45 ms in the same trace, median 40, and no open's frame took 50 ms in nine runs of the check, two of them with two other CPUs kept busy.

### Animations that never stop

- An animation that runs all the time moves by `transform` or `opacity`, which the compositor animates without the main thread. The header's line of light slides a strip twice its gradient's width by one gradient (`.portal-line::before`).
- When it animated `background-position` instead, every frame repainted the header and redid its backdrop blur, and the frame after each tap or keystroke waited behind that work: interactions took 40–64 ms in a headless browser without a GPU, and 16–32 ms after the change.
- The wordmark's sheen still animates its background, but it holds still for most of its 14 s cycle.
- Under a dialog both stand still, from the frame after it opens (`html.header-still`, set by `util::dialog_open`); its backdrop was blurred then. Moving, they had the browser blur the whole screen again in every frame: with the composer open in a headless browser without a GPU, that took 540 of every 600 ms of the display compositor's thread, and each key typed waited 60 to 130 ms for its frame. Standing still, 70 to 120 ms of 600, and a key shows in 24 to 56 ms on the phone profile. A phone's GPU blurs faster, but no more often.

### 4. Edge

- **Stale-while-revalidate** on every API payload, with request coalescing per isolate and stale-if-error. Policies are in `allthing_core::api` and listed in [BUDGET.md](BUDGET.md#2-edge-cache-one-upstream-fetch-serves-everyone-nearby).
- **Server time**: every API answer says how long the Worker spent on it in a `Server-Timing` header, named after how it was served (`fresh;dur=3`, or `miss;dur=1840` for a build the request waited for). Browsers show it in their network panel and in `PerformanceResourceTiming.serverTiming`. Clocks in a Worker only advance across I/O, so it measures waiting on the cache and upstream, not CPU time.
- **Compact JSON**: payloads skip empty and default fields (`skip_serializing_if`). Cloudflare compresses responses with Brotli for clients that accept it.
- **Immutable assets**: the app's `.wasm`, `.js` and `.css` are fingerprinted and served with `Cache-Control: public, max-age=31536000, immutable` (`web/_headers`).
- **The WebAssembly at brotli's highest level**: the native Rust build writes optional `<name>.wasm.br` copies at quality 11, window 24, the same settings previously used by Node's zlib. `/wasm/<name>` serves those bytes with `Content-Encoding: br` to browsers that accept Brotli, and the original module otherwise. If that route fails, the loaders fall back to `/assets/<name>`. In an earlier build, fast edge compression left a 1.5 MB app module at about 550 KB, compared with about 434 KB at quality 11. Those sizes describe that historical build, not a measured improvement from changing compression implementations.
- **The Worker's WebAssembly without the relocations**: the workspace links all its WebAssembly with `--emit-relocs` (`.cargo/config.toml`), which the browser app's wasm-split reads. The Worker's module carried those sections too, `linking` (2.5 MB) and `reloc.CODE` (0.4 MB), more than half its bytes: nothing reads them there, and once worker-build's wasm-opt has rewritten the code they no longer describe it. `cargo xtask strip-worker`, which the native `cargo xtask build` runs after worker-build, drops them and keeps every other byte. Measured on main at 74bc5ca (21 October 2026):
  - `wrangler deploy --dry-run`: 2,568.39 KiB and 794.12 KiB gzip, where it was 5,442.53 KiB and 1,403.01 KiB, against the Free plan's 3 MB compressed limit;
  - compiling the module in Node 22's V8: 8.3 ms of CPU where it took 11.3 (medians of 15);
  - the first request to `/api/v1/status` under `wrangler dev`, which loads the Worker: 822 ms where it took 1,123 ms (medians of 5, alternating, on a busy machine).

  The function names (`name`, 0.8 MB, 93 KiB compressed) stay. V8 names the frames of a trap's stack trace from them, and with `panic = "abort"` a panic is such a trap, so Cloudflare's logs say which functions it went through rather than `wasm-function[1234]`. Dropping them as well would save those 93 KiB and nothing measurable at start (701.15 KiB gzip; 7.9 ms of CPU to compile; 926 ms for the first request).
- **Type-erased views**: the browser app is built with Leptos's `--cfg erase_components` (`.cargo/config.toml`), so each component's view and each reactive closure in it is boxed instead of monomorphizing the view machinery for it. The module went from 1,524,962 to 1,395,957 bytes, and from 440,631 to 393,099 bytes at brotli 11 (47.5 KB, a tenth, less to download before anything renders: about 240 ms on a 1.6 Mbps "slow 4G" link, 40 ms on 9 Mbps). The cost is a dynamic call per closure. Measured on 19 October 2026 on a CI runner, the two builds served locally with the same live data, alternating, median of 9: the first cards drew 3–6 ms later unthrottled (69 → 72 ms at 1280 px, 62 → 68 ms at 390 px, from instantiation) and 15–21 ms later with the CPU slowed 4×; switching sort tabs took the same 56 ms at 1280 px and 32 → 48 ms at 390 px, within the 100 ms budget. A release `opt-level = "s"` was tried too, and was larger (1,695 KB).
- **Sign-in costs browsers little**: its cryptography and QR encoder run in the Worker, and the browser draws the code from the SVG path the Worker sends. The header button, dialog and account menu added 15.2 KB to the module at brotli 11 (395,279 → 410,508 bytes, measured on 19 October 2026 from release builds), about 76 ms more on a 1.6 Mbps "slow 4G" first visit and 12 ms on 10 Mbps. A signed-in page view still makes one API call: the header reads who is signed in from a cookie.
- **Liking and recasting** cost every reader 29.5 KB of the module at brotli 11 (434,181 → 463,702 bytes, release builds on 19 October 2026): the approval dialog, the key's storage and signing, the message encoding and its BLAKE3 hash. That is about 150 ms more on a 1.6 Mbps "slow 4G" first visit and 24 ms on 10 Mbps. The browser's own WebCrypto does the Ed25519 signing, and answers from the Farcaster app's API and the node are read as `serde_json::Value`, one JSON reader instead of a deserializer per answer, which saved 3.5 KB of the 33 it first took. A signed-in reader's page view makes no more API calls: `/api/v1/signer` is asked once an hour, and what they liked is read from Hypersnap by the browser.
- **Casting, replying and quoting** with the reader's key added 8.1 KB more (463,702 → 471,779 bytes at brotli 11): the cast's encoding, mentions and the composer's post button. Mentions are looked up only when a cast is posted.
- **Following and deleting** added 7.1 KB more (471,779 → 478,838 bytes at brotli 11): Follow and its lookup, Delete and its question, and a deleted cast shown as deleted in feeds, posts and threads. A profile view costs a reader who can follow one Hypersnap read from the browser, and no Worker request.
- **Emoji** added 2.9 KB to the module at brotli 11 (483,806 → 486,790 bytes, release builds on 20 October 2026): the pack's table, the shortcodes and their view. The images cost nothing until a cast shows one. Each is a static file under `/assets/emoji/v1/`, cached for a year and loaded lazily: a median of 17 KB and 133 KB at most, or with reduced motion a still of about 2 KB. A page view makes no more Worker requests.
- **The emoji picker** added 8.6 KB (486,790 → 495,615 bytes at brotli 11): its grid, search and recently used, and the composer's caret handling. Two choices kept it small:
  - its tiles share six listeners on the grid, instead of six each;
  - its results are drawn anew for each search instead of as a keyed list, which saved 3 KB.

  Opening it the first time loads the stills in view, about 110 KB at 1280 px (all 219 are 509 KB), from the year-long cache after that. It makes no Worker requests.
- **Stickers** added 4.7 KB more (495,615 → 500,468 bytes at brotli 11): the sticker button and the picker's Stickers tab with its recently used, a sticker as the cast's attachment, and stickers in casts. A cast's sticker loads its GIF from the site itself, cached for a year: a median of 74 KB (12 to 269 KB), or with reduced motion a still of about 4 KB. The picker's tab shows the stills until a sticker is pointed at. No Worker requests.
- **Handing writes to the Farcaster app** added 5.8 KB (551,831 → 557,797 bytes at brotli 11, release builds on 20 October 2026): its dialog, the links to casts, profiles and drafts, and the like, recast, follow, delete and composer buttons shown to every reader. The QR code is drawn by the Worker, as sign-in's is, so the browser ships no encoder; a phone asks for none.
- **A bigger Pepe pack**, 179 emoji where there were 103, added 1.1 KB (558,162 → 559,284 bytes at brotli 11, release builds on 20 October 2026): the table's 76 new rows, and each emoji's group. The new images, a median of 16 KB and 100 KB at most, cost nothing until a cast or the picker shows them.
- **The emoji picker's groups** added 2.8 KB (559,284 → 562,128 bytes at brotli 11): headings, the bar of groups with its faces, and following the grid's scroll to mark the group in view. The bar's faces are 11 of the stills the grid shows anyway.
- **Whole GIFs in the picker** added 1.3 KB to the module (2,080,264 → 2,081,551 bytes) and nothing measurable at brotli 11 (562,328 → 562,200 bytes, release builds on 20 October 2026; on the main before the groups' bar the same change measured +1.0 KB, as a small change reorders the module and moves brotli's result by about a kilobyte either way): each tile's shape and the rows' CSS, Enter's search, and leaving a phone's keyboard down. The tiles load what they did.
- **Preconnects** to `imagedelivery.net` and `stream.farcaster.xyz`, where nearly all media is hosted.
- **The sound badge and looping** added 1.7 KB to the module (2,080,264 → 2,081,974 bytes) and nothing measurable at brotli 11 (562,328 → 562,142 bytes, release builds on 20 October 2026; on the main before the groups' bar the same change measured +1.0 KB, as a small change reorders the module and moves brotli's result by about a kilobyte either way): the badge's speaker, and the player's going on segment by segment and ending a stream once its last segment is in. A playlist that drifts from its media now costs one request per segment, where the player had fetched its last two segments in turn without end.
- **40 more Pepes**, 219 where there were 179, added 4.3 KB to the module and 0.8 KB at brotli 11 (2,083,253 → 2,087,558 bytes, and 562,884 → 563,723 at brotli 11, release builds on 20 October 2026): the table's 40 new rows. The new images, a median of 7 KB and 77 KB at most, cost nothing until a cast or the picker shows them.
- **Showing what the reader did in the Farcaster app** added 6.0 KB to the module and 1.1 KB at brotli 11 (2,086,767 → 2,092,752 bytes, and 563,650 → 564,705 at brotli 11, release builds on 20 October 2026; a small change moves brotli's result by about a kilobyte either way): after a hand-off, asking the node whether the reader did it there, and showing it when they have; and the dialog's words for taking a like or a recast back. A signed-in reader whose writes go to the Farcaster app now reads their latest likes and recasts from the node, as a reader with a key does: 2 browser requests a page load, 2 more on a post page. After a hand-off, the browser asks at once, then every 3 seconds, for 3 minutes at most. None of these go through the Worker.
- **Hand-offs that show at once** added 29.0 KB to the module and 9.5 KB at brotli 11 (2,092,752 → 2,121,781 bytes, and 564,705 → 574,181 at brotli 11, release builds on 20 October 2026): a write's pending state and taking it back, the reader's casts on their way above a post's replies and the feed, finding a cast among the reader's latest on the node, and reply counts that follow along. That is about 48 ms more on a 1.6 Mbps "slow 4G" first visit and 8 ms on 10 Mbps. A page view makes no more requests. The reply counts are memos, so the reader's likes loading doesn't redraw every card's.
- **Stickers that move, as on Telegram** added 12.2 KB to the module and 2.3 KB at brotli 11 (2,121,781 → 2,134,007 bytes, and 574,181 → 576,471 at brotli 11, release builds on 20 October 2026): the tiles that load as they come into view, the taller panel, and a held sticker's large preview. Opening the Stickers tab the first time loads the 20 moving tiles in view, 482 KB at 1280 and at 390 px (the stills it loaded before were 132 KB for all 32), then from the year-long cache; all 32 are 951 KB, about a third of their GIFs. With reduced motion or Save-Data, the stills, as before. No Worker requests.
- **Favorite and Show less on profile cards** added 13.0 KB to the module and 3.7 KB at brotli 11 (1,781,484 → 1,794,487 bytes, and 494,423 → 498,156 at brotli 11, release builds on 21 October 2026): the two buttons and their toggling, which only a profile's part had before (`split_user.wasm` went from 67,721 to 58,462 bytes; with the other parts and the chunks they share, the app is 4.8 KB bigger in all), the card's row, and keeping a feed's order under a card. That is about 19 ms more on a 1.6 Mbps "slow 4G" first visit. A page view makes no more requests.
- **Allthing's poll** is a part too (`split_poll.wasm`, 23.4 KB, 10.2 KB at brotli 11, with 3.1 KB at brotli 11 in six small chunks it shares with other parts): Yes and No, the results, voting, and reading and keeping the vote. The front page fetches it once it is idle, so the feed's cards and their images come first. The card's frame, with its question and its body's box (112 px), stays in the module and is drawn with the cards, and the body fills in place, so nothing moves. The module grew by 15.4 KB and 3.0 KB at brotli 11 (1,780,647 → 1,796,081 bytes, and 494,439 → 497,450 at brotli 11, release builds on 21 October 2026), about 15 ms more on a 1.6 Mbps "slow 4G" first visit: the frame, the state its two places share, the counts in the front page's payload, and the feed's place for the card. Inlined, it added 32.5 KB and 7.0 KB at brotli 11. The stylesheet grew by 2.7 KB, 0.4 KB gzipped. A page view makes no more Worker requests: the counts come with the front page's payload, and a vote is one request, once.
- **Who's reading now**, the header's presence chip ([ARCHITECTURE.md](ARCHITECTURE.md#presence)), added 43.4 KB to the module and 11.0 KB at brotli 11 (1,780,647 → 1,824,020 bytes, and 494,439 → 505,482 at brotli 11, release builds on 21 October 2026): the beats' scheduling and what a browser keeps between its tabs, the chip and its popover. Its popover's parts are drawn anew when they change, not as a keyed list and shown or hidden parts, which kept it 3.4 KB smaller at brotli 11 than it first was. A page view makes no more Worker requests: the beat goes after the page's own call, and a browser beats every 4 to 15 minutes while in use ([BUDGET.md](BUDGET.md#14-presence)). The static copy doesn't have it.

### 5. Infinite scroll

- Lists show 25 more cards at a time, drawn a screenful per task as above, so scrolling never waits on a long task that builds 25 cards.
- A sentinel 1,200 px below the last card shows the next 25 before the reader reaches it.
- On the front page, once the loaded pool runs out, the next trending page (`/api/v1/home?offset=100`, … up to 900) is fetched at the same point, so the next cards are usually ready before they are needed.
- The end is checked again after cards or a server page arrive. Short and empty filtered pages keep filling without requiring the sentinel to leave and re-enter the viewport; failures still wait for retry backoff, and loading stops at offset 900.

### Code loaded when used

Parts of the app that most visits don't use are cut out of the module when it's built, and fetched the first time they're used.

- **How.** A function marked `#[wasm_split(<part>, …)]` is the way into a part (allthing-web's `split.rs`). Before wasm-bindgen, `cargo xtask` runs wasm-split ([`wasm_split_cli_support`](https://github.com/WorldSEnder/wasm-split-prototype), what `cargo leptos --split` runs): the code only that function reaches goes into `<part>.wasm`, which shares the module's memory and function table, and `allthing-split.js` fetches it the first time the function is called. Calling it again is immediate. wasm-split finds what each function reaches from the relocations the linker keeps (`--emit-relocs` in `.cargo/config.toml`), and the parts reach the table through its export, which wasm-bindgen keeps (`--keep-lld-exports`). The splitting itself costs the module about 0.6 KB at brotli 11.
- **Files.** The module, its JavaScript, the loader and the parts work only with one another, so one fingerprint names them all: `assets/allthing-<hash>.wasm`, which the Worker serves at `/wasm/` as before, and the folder `assets/allthing-<hash>/` with `allthing.js`, `allthing-split.js` and the parts. `index.html` preloads both scripts. A part is a static asset: fetching one costs no Worker request, and the edge compresses it.
- **After a deploy.** A page opened before a deploy runs the previous version, whose files are gone. When a part can't be fetched, the page loads again, once per version and tab (`split::reload_once`), as the new version; if the part still can't come, the page says so and offers to try again.
- **The DAU page's dashboard** is the first part (`split_dau.wasm`, 39 KB, 16.9 KB at brotli 11): its tiles, chart, tables and readouts. /dau fetches it while it fetches the counts. The module went from 2,145,244 to 2,119,212 bytes, and from 579,624 to 572,909 bytes at brotli 11: 6.6 KB (1.2%) less to download before anything renders, about 34 ms on a 1.6 Mbps "slow 4G" first visit (release builds on 20 October 2026). Until the dashboard's code has come, the page says it is loading the counts, as before. With SnapMeter's meter, the 48-hour, 7- and 30-day ranges and its definitions (21 October 2026), the part is 70.5 KB (26.4 KB at brotli 11), 31.2 KB with the code it shares, from 24.5 KB: 6.7 KB more, about 35 ms on a 1.6 Mbps "slow 4G" first visit to /dau, fetched alongside the counts. A first cut was 11.8 KB more: drawing each range once, one reactive readout, a signal type the app already has, and no standard-library sort for the few kinds of messages saved 5.0 KB of it. The module every visit downloads grew by 0.5 KB at brotli 11, for the payload's hourly counts, and the stylesheet by 0.4 KB.
- **The composer** is the next (`split_composer.wasm`, 131 KB, 48.7 KB at brotli 11), with **the emoji and sticker picker** it shares with React (24.7 KB at brotli 11): the GIF, emoji and sticker pickers, scheduling, the bar and posting. The dialog's frame, its title and the text box stay in the module, so the composer opens at once and takes typing the first time too, while the bar shows a quiet "Loading…" in the post button's place. Pointing at or focusing Cast, Reply or Quote, or pressing one on a touch screen, starts fetching its code, so it usually comes before the click does; and for a reader signed in, a page with Reply (a post's) fetches it, with the GIF key, once a visit when the browser is first idle (`compose::warm_when_idle`; in Safari, which can't say, 1.5 s on), so the first Reply opens with its bar, as every one after it does. React's picker is a part of its own (0.3 KB) with the shared picker, fetched as React is pointed at, focused or pressed. A part of a page that can't be fetched says so in its place, with Reload, and doesn't reload the page by itself, so a draft isn't lost unasked. The module went from 2,119,212 to 1,958,089 bytes, and from 572,909 to 537,600 bytes at brotli 11: 34.5 KB (6.2%) less to download before anything renders, about 177 ms on a 1.6 Mbps "slow 4G" first visit (release builds on 20 October 2026).
- **Signing in and /scheduled** came next. Signing in, from the relay's code to the Worker's check of the approval (`split_signin.wasm`, 9.1 KB at brotli 11), is fetched as Sign in is pointed at, focused or pressed (`signin::warm`), or when the dialog opens; the dialog itself, and the account's picture and menu, stay in the module. If it can't be fetched, the dialog says so, and Get a new code tries again. /scheduled's list (`split_scheduled.wasm`, 15.6 KB) comes with the page, which says it is loading the casts meanwhile; it shares 3.2 KB with the composer. The module went from 1,958,089 to 1,899,433 bytes, and from 537,600 to 522,250 bytes at brotli 11: 15.0 KB (2.9%) less, about 77 ms on a 1.6 Mbps "slow 4G" first visit (release builds on 20 October 2026).
- **Writing** is a part too (`split_writer.wasm`, 13.5 KB at brotli 11, with 7.6 KB it shares with the composer and the others). It holds signing a like, a recast, a follow, a delete or a cast and handing it to the node; approving a key and reading the one this browser keeps; and looking on the node for what was handed to the Farcaster app. A signed-in reader's browser fetches it as soon as the page knows who they are, so it's in before their first write, and before the first look on the node for what they handed to the Farcaster app. A signed-out reader's browser never fetches it, since their writes go to the Farcaster app. If it can't be fetched, a signed-in reader's writes go to the Farcaster app too, as where Allthing doesn't write for them. The module went from 1,899,433 to 1,856,726 bytes, and from 522,250 to 508,003 bytes at brotli 11: 14.2 KB (2.7%) less, about 71 ms on a 1.6 Mbps "slow 4G" first visit (release builds on 20 October 2026). Throttled as the speed report's phone, alternating with the build before it, median of 9: the WebAssembly was in 74 ms sooner (3,407 → 3,333 ms) and the first cast came 85 ms sooner (3,755 → 3,670 ms).
- **Pages of their own.** A post's page, a profile's, a channel's, search's and the channels' are parts too: `split_post.wasm` (34.4 KB at brotli 11), `split_user.wasm` (26.1 KB), `split_channel.wasm` (15.6 KB), `split_search.wasm` (10.3 KB) and `split_channels.wasm` (8.3 KB), with the code they share in small chunks. Their code comes as it's likely wanted (`split::preload_pages`), so a page opens as it did when its code came with the app:
  - a post's a few seconds after the app starts, once the browser is idle (opening a post is how most visits go on from a feed; not on a slow or metered connection), and as a card is pointed at or touched, with its data;
  - a profile's, a channel's, search's and the channels' as a link to one is pointed at, touched or focused, anywhere on the page, and search's as the search box is. The pages' parts are 94.8 KB together at brotli 11, compressed apart, so fetching them all on every visit would cost more than the module saves;
  - and with the app, when a visit starts on one. `index.html` lists each page's files (its `allthing-parts` meta tag, filled in by `cargo xtask`), and its `early-parts` script preloads them; the loader then uses what was preloaded.

  Until a page's code has come, two placeholder cards show. The module went from 1,857,056 to 1,622,512 bytes, and from 508,082 to 456,187 bytes at brotli 11: 51.9 KB (10.2%) less, about 259 ms on a 1.6 Mbps "slow 4G" first visit (release builds on 20 October 2026). Throttled as the speed report's phone, alternating with the build before it, median of 9: the WebAssembly was in 260 ms sooner (3,331 → 3,071 ms) and the first cast came 261 ms sooner (3,664 → 3,403 ms).
- **Followers and following.** `/u/<name>/followers` and `/following` are a part of their own, `split_follows.wasm` (33.2 KB at brotli 11), so the profile's part didn't grow; it comes as a profile's counts are pointed at, touched or focused, with the list's first page (`api::prefetch`), and with the app when a visit starts on a list. A list of 1,000 people keeps under 40 rows in the page (`pages/follows/window.rs`), and Back from a person's profile shows the list kept from the visit, where it was.
- **A feed card's emoji reactions** are a part too (`split_chips.wasm`, 10.3 KB, 4.4 KB at brotli 11), with code they share with a post's page (10.5 KB at brotli 11 in four small chunks, which the post's code needs anyway and the app fetches a few seconds in, but on a slow or metered connection). A card's chips call it the first time a card has some (`reactions::card_reactions`), right as the listing's first cards are drawn, so their code comes while the page's images do; it costs no Worker request. Until it comes, an empty row of the chips' height (28 px; a chip's `li` is a flex box, so the line's descent adds nothing) holds their place, so nothing below moves, and if it can't come, the cards go without them, with no message on each (`split::lazy_extra`). Inlined, the chips added 29.8 KB to the module and 9.5 KB at brotli 11 (1,642,062 → 1,671,826 bytes, and 459,799 → 469,288); as a part they add 8.5 KB and 1.8 KB (→ 1,650,538 and 461,590 bytes at brotli 11), the memo that says whether a card has any and the row that waits for them (release builds on 21 October 2026).
- **Images and videos in the composer** added 23.6 KB to the module and 4.4 KB at brotli 11 (1,726,484 → 1,750,034 bytes, and 483,170 → 487,527 at brotli 11, release builds on 21 October 2026): the draft's files and how their uploads go, a paste's or a drop's files handed to the composer's part, a scheduled draft's files, and the reader's own cast showing a video. The rest is in the composer's part, which grew from 133,752 to 212,651 bytes, and from 49,913 to 72,301 at brotli 11: the attach button and the tiles, uploading with progress, and taking a picture's metadata out (`allthing_core::metadata`). So a reader who doesn't write fetches none of it, and one who does fetches it as Cast, Reply or Quote is pointed at, about 110 ms more on a 1.6 Mbps "slow 4G" link; /scheduled's list grew 1.8 KB at brotli 11, for its files' thumbnails. Taking the metadata out reads the picture into the WebAssembly memory, which holds it twice (as read, and without its metadata) until it's handed to the upload: up to about 20 MB for a 10 MB photo, which the memory keeps for the visit. Videos and GIFs aren't read. A page view makes no more requests: the composer asks `/api/v1/media` as it opens, for a reader signed in, and keeps the answer 5 minutes.
- **Drafts that last** added 34.9 KB to the module and 8.8 KB at brotli 11 (1,801,432 → 1,836,328 bytes, and 499,834 → 508,630 at brotli 11, release builds on 21 October 2026): the Compose and Drafts tabs, the drafts the module holds as read (`KEPT`), and keeping a draft and taking one up as the composer opens. Their JSON is read and written in the composer's part, which grew from 213,966 to 266,461 bytes, and from 73,188 to 87,551 at brotli 11, with the Drafts list and Discard: first built in the module, the drafts cost it 16.5 KB at brotli 11, most of it the JSON's code for the drafts' shapes. A reader with drafts kept fetches the composer's part once the browser is idle, so a first open takes its draft up at once; anyone else's comes as Cast, Reply or Quote is about to be used, as before. The tap that opens the composer does 1.5 to 5 ms more work with the CPU slowed 2× (its main thread's time, measured alternating with the build before it): the tabs' five elements, and taking a draft up.
- **Messages** (`/messages`) is one part: `split_realtime.wasm`, the inbox (#general and encrypted conversations together, the thread, the composer and the person picker), 302.0 KB and 94.5 KB at brotli 11, plus, for a signed-in reader, the SpacetimeDB bridge (`assets/stdb-<hash>.js`, 120 KB, 28 KB at brotli 11). It comes with the page, which shows the box's frame meanwhile, or as the account menu's Messages is pointed at, focused or touched. Its styles are injected by the part, so `allthing.css` doesn't carry them. Dropping the Farcaster Direct Casts flow (22 October 2026, release builds) took the page's code from two parts, `split_messages.wasm` and `split_realtime.wasm` (246,872 + 265,760 = 512,632 bytes; 77,567 + 83,021 = 160,588 at brotli 11), to this one (301,998; 94,517 at brotli 11), 41% less, and the app's module from 1,990,372 to 1,981,622 bytes (546,361 → 544,471 at brotli 11). Of the Worker the page asks only for the attestation as it connects, and `/api/v1/search` when the reader looks for someone to write to.
- **Writes read back from the node** added 5.8 KB to the module and 2.7 KB at brotli 11 (1,780,642 → 1,786,571 bytes, and 494,306 → 497,063 at brotli 11, release builds on 21 October 2026; a small change moves brotli's result by about a kilobyte either way): a notice's Retry or Approve again, and a cast on its way that says it's posting until the node has it. The rest is in writing's part, which grew from 39,334 to 41,203 bytes (16,373 → 17,157 at brotli 11): handing a message to the node again while it's busy, reading each write back from the node's store, and taking back what doesn't land; and in the composer's, from 206,097 to 211,000 bytes (70,540 → 72,131 at brotli 11): keeping a cast's signed message for Retry. A page view makes no more requests. A write makes 1 to 6 more from the browser to Hypersnap, about a kilobyte each at most, none through the Worker ([BUDGET.md](BUDGET.md)).
- **A reply in its thread at once** added 5.5 KB to the module and 1.3 KB at brotli 11 (1,786,584 → 1,792,130 bytes, and 496,875 → 498,135 at brotli 11, release builds on 21 October 2026; a small change moves brotli's result by about a kilobyte either way): a reply on its way, counted, and the word for screen readers once it's posted. A post's part grew from 84,138 to 93,955 bytes (29,783 → 32,944 at brotli 11), for the reader's replies drawn as replies under what they answer, about 16 ms more on a 1.6 Mbps "slow 4G" link to a post's page; the composer's from 211,000 to 224,108 bytes (72,073 → 76,194 at brotli 11), for sending a reply after the composer has closed and opening it again with the draft, why, and Retry. Drawn as a keyed list, the reader's replies cost the post's part 3.9 KB more at brotli 11; there are one or two at most, so they're drawn again as one changes. A page view makes no more requests, and a reply no more than before.
- **Notifications** added 5.6 KB to the module at brotli 11 (1,780,640 → 1,804,219 bytes, and 494,256 → 499,893 at brotli 11, release builds on 21 October 2026), about 28 ms more on a 1.6 Mbps "slow 4G" first visit and 5 ms on 10 Mbps: the header's bell and its count, the reader's read state, the route, and reading each group's time (`notifications::Times`) rather than the whole payload. The page is a part (`split_notifications.wasm`, 67.3 KB, 24.9 KB at brotli 11; with the chunks it shares with the other pages, 13 files and 29.8 KB at brotli 11, compressed apart), fetched as the bell is pointed at, touched or focused, or with the app when a visit starts on it. Until its code has come, two placeholder cards show, as for the other pages. A signed-in reader's tab makes one more Worker request when the app starts, the bell's, and one every 3 minutes while it is in view ([BUDGET.md](BUDGET.md#1-what-a-page-view-costs)).
- **One QR flow** took 4.7 KB out of the module and 1.2 KB at brotli 11 (1,788,389 → 1,783,670 bytes, and 496,364 → 495,152 at brotli 11, release builds on 21 October 2026): writing's own approval dialog went, with its asking once on a page load, and the sign-in dialog shows the approval in its approve form. Signing in's part grew 0.2 KB at brotli 11 (8,257 → 8,491 bytes), and writing's shrank a little (16,402 → 16,360).

## Measurements

<!-- Keep these up to date when the numbers move. -->

**Opening a post from the home page.** "Before" is 0.2.0 in production; "after" is 0.3.0 with a hovered (prefetched) card, measured as click → post text.

| | 0.2.0 | 0.3.0 |
|---|---|---|
| Edge cache cold for that post | 840–1,100 ms | 20–28 ms |
| Edge cache warm | ≈ 335 ms | 20–28 ms |

Both columns were measured on https://allthing.social, from the same cloud sandbox, on 2026-10-18.

In 0.2.0 a post waited for its API call. In 0.3.0 the post renders from the feed's data, so the click no longer depends on the network at all, and replies arrive in the background.

**Other metrics, 0.3.0.** Local runs used `wrangler dev`; production was measured from a cloud sandbox.

| Metric | Local | Production |
|---|---|---|
| Home LCP, cold (median of 3) | ≈ 120–150 ms | 736 ms (samples 655, 736, 1,872 ms) |
| Slowest interaction (sort tabs, popovers) | 56–72 ms | 56 ms |
| Layout shift (CLS), home | — | 0.000 |

The production LCP samples vary with the sandbox's proxy and with how warm each data center's edge cache is. The 1,872 ms outlier was a cold load through the proxy. The budget applies to the median.

Interaction latency was 312–1,088 ms before sorting became reactive, because each sort change rebuilt the page.

**Layout shift as images come, 21 October 2026.** CLS as Chrome counts it (a buffered `layout-shift` observer, shifts that answer input left out), from a cold load or from the action until 3 to 4 seconds after it. "Before" is allthing.social at 87beef5, read only; "after" is this change under `wrangler dev`, with live data through the local Worker, the same day. One run each; the footer's own move as a page's content takes its placeholder's place is the page's height while loading, not an image's, and is in brackets where it moved.

| | 1280 px before | 1280 px after | 390 px before | 390 px after |
|---|---|---|---|---|
| A post loaded cold (`/p/0xd8a8…c5b85deb2a9`, one photo, 33 replies) | 0.251 (0.321) | 0 (0.069) | 0.301 (0.386) | 0 |
| The front page, Past hour | 0.495 | 0.000 | 0.502 | 0.006 |
| The front page, Past week | 0.313 | 0 | 0.327 | 0.000 |
| The front page, new | 0.125 | 0 | 0 | 0 |
| The front page, 14 scrolls down | 0.189 | 0.001 | 0.041 | 0 |

`e2e/image-boxes.fixture.spec.ts` holds a post loaded cold, two time ranges and the new tab under 0.05 at both widths, with the images taking 1.5 s to come.

### Speed report

`e2e/speed-report.spec.ts` loads the front page cold, seven times per profile, under fixed network and CPU profiles, and prints the medians. It reports and asserts nothing about the budget, which `e2e/perf.spec.ts` checks:

| Profile | Viewport | Network | CPU |
|---|---|---|---|
| Desktop | 1280 × 900 | 10 Mbps down, 40 ms RTT | as is |
| Phone ("slow 4G") | 390 × 844, touch | 1.6 Mbps down, 150 ms RTT | slowed 4× |

It measures time to first byte, when the WebAssembly and the front page's data are in (and how many KB the WebAssembly took on the wire), the first cast card on screen (drawn early, or by the app), the app's own first card (once the early ones have given way), LCP (the later of the LCP entry and the first card, as the budget counts it), and the slowest of two sort-tab switches (Event Timing) in the app. It records the deployed commit before and after its runs, and fails if a deploy landed in between.

CI runs it on a manual run of the workflow (the `speed report` job, against https://allthing.social); locally:

```sh
E2E_SPEED=1 E2E_BASE_URL=https://allthing.social npx playwright test e2e/speed-report.spec.ts
```

A cloud sandbox's network and CPU stall often enough to make these numbers meaningless there; a CI runner's samples agree within about 20 ms.

**WebAssembly with type-erased views (#140), 19 October 2026.** Production, before (29c7ade) and after (040d095) the change, median of 7 on a CI runner:

| Desktop, 1280 px | Before | After |
|---|---|---|
| WebAssembly on the wire | 431.4 KB | 384.5 KB |
| In by | 571 ms | 510 ms |
| First cast | 649 ms | 563 ms |
| LCP | 1,196 ms | 992 ms |
| Slowest interaction | 56 ms | 40 ms |

The phone run before the change overlapped its deploy, so the phone's before and after were measured side by side instead: main built with and without `--cfg erase_components`, each served from the runner as the Worker serves it (the brotli WebAssembly at `/wasm/`, the API passed through to production), three alternating rounds of three:

| | Desktop before | Desktop after | Phone before | Phone after |
|---|---|---|---|---|
| WebAssembly on the wire | 431.4 KB | 384.9 KB | 431.4 KB | 384.9 KB |
| In by | 573 ms | 530 ms | 3,295 ms | 3,062 ms |
| First cast | 628 ms | 586 ms | 3,540 ms | 3,306 ms |
| LCP | 1,120 ms | 1,076 ms | 3,540 ms | 3,306 ms |
| Slowest interaction | 40 ms | 32 ms | 112 ms | 112 ms |

On the phone profile the front page shows its first cast about 0.23 s sooner, the time 47 KB takes at 1.6 Mbps. Production's phone numbers after the change match: first cast 3,052 ms (samples 3,040–3,070), slowest interaction 104 ms.

**The front page's first cards before the app (`web/paint.js`), 21 October 2026.** Main (9f19445) and this change were each served locally over HTTP/2 with TLS, as the Worker serves them (the WebAssembly from its brotli-11 copy, the rest brotli'd). Both got the same front-page payload (production's, 88 casts) and the images it asks for, recorded from their hosts, so the throttled browser timed both builds on the same bytes. Nine alternating rounds under the speed report's profiles; medians:

| | Desktop, main | Desktop, early cards | Phone, main | Phone, early cards |
|---|---|---|---|---|
| Data in by | 191 ms | 200 ms | 934 ms | 994 ms |
| WebAssembly in by | 560 ms | 571 ms | 3,086 ms | 3,169 ms |
| First cast on screen | 642 ms | 260 ms | 3,444 ms | 1,215 ms (1,206–1,226) |
| The app's own first card | 642 ms | 643 ms | 3,444 ms | 3,510 ms |
| LCP, the first screen's images in | 1,156 ms | 940 ms | 6,736 ms | 5,640 ms |
| Layout shift (CLS) | 0.000 (0 to 0.236) | 0.000 (all 9) | 0.155 (0 to 0.336) | 0.000 (all 9) |
| Slowest sort switch | 32 ms | 24 ms | 56 ms | 56 ms |

- The first cast shows 2.2 s sooner on the phone and 0.38 s sooner on a computer. The early cards can't be used, and the app's own come as before on a computer and 66 ms later on the phone: the script's 10 KB share the line with the WebAssembly.
- LCP counts the first screen's images here, and they come sooner: they're asked for once the WebAssembly is in, not once the app has drawn its cards.
- On main, a single image grows its card as it loads and pushes the cards below it down. The early cards' tiles have their boxes from the start, and the app's take the same boxes.

**The early cards drawn again, 22 October 2026.** On production they had stopped drawing: the front page's first card was often a quote, which `paint.js` didn't draw, and a card it doesn't draw ends the early ones, so there were none, and the first cast came with the app's (production, f596b2b, median of 5: 2,768 ms on the desktop profile, 4,645 ms on the phone, the WebAssembly in by 3,993 ms and the data by 2,244 ms). Now quotes are drawn as the app's quote cards, and on a phone the early cards stop at the fourth, before the app's poll, so nothing moves when the app takes over. Locally (`wrangler dev`, live data, a 737.6 KB WebAssembly uncompressed), median of 7:

| | Desktop, before | Desktop, after | Phone, before | Phone, after |
|---|---|---|---|---|
| Data in by | 262 ms | 277 ms | 1,298 ms | 1,270 ms |
| WebAssembly in by | 928 ms | 911 ms | 4,778 ms | 4,786 ms |
| First cast on screen | 1,193 ms | 450 ms | 5,458 ms | 1,578 ms (1,446–1,603) |
| The app's own first card | 1,193 ms | 1,066 ms | 5,458 ms | 5,537 ms |

**Since then, 20 October 2026 (3cc5cd0).** Production, median of 7 on a CI runner (run 39483304275), against 040d095 the day before (run 39251478185), each run's own profile:

| Desktop, 1280 px | 19 Oct (040d095) | 20 Oct (3cc5cd0) |
|---|---|---|
| Time to first byte | 66 ms | 127 ms |
| WebAssembly on the wire | 384.5 KB | 565.9 KB |
| In by | 510 ms | 764 ms |
| Data in by | 227 ms | 278 ms |
| First cast | 563 ms | 846 ms |
| LCP | 992 ms | 1,820 ms |
| Slowest interaction | 40 ms | 56 ms |

| Phone, 390 px | 19 Oct (040d095) | 20 Oct (3cc5cd0) |
|---|---|---|
| Time to first byte | 50 ms | 97 ms |
| WebAssembly on the wire | 384.5 KB | 565.9 KB |
| In by | 2,823 ms | 3,798 ms |
| Data in by | 1,031 ms | 1,045 ms |
| First cast | 3,052 ms | 4,160 ms (samples 4,148–4,179) |
| LCP | 3,052 ms | 4,160 ms |
| Slowest interaction | 104 ms | 176 ms |

- **The phone shows its first cast 1.1 s later, and the WebAssembly is nearly all of it.** It is 181 KB (47%) bigger on the wire, which is 0.9 s at 1.6 Mbps; the data came in when it did. The module grew with what came after 040d095: writing from Allthing (liking and recasting alone added 29.5 KB) and the hand-off to the Farcaster app, emoji and stickers, scheduled casts, favorites and show-less, the DAU page. Most are listed above with their sizes.
- **The slowest sort switch on the phone, 176 ms with the CPU slowed 4×, is over the 100 ms that `e2e/perf.spec.ts` allows**, which it checks unthrottled (56 ms there, as at 1280).
- **Time to first byte doubled on both profiles**, for `index.html`, a static asset the Worker doesn't run for: the runner's network that day, not the app. The desktop's first-cast samples were noisy too (809 to 3,648 ms, median 846).
- **The budget still holds unthrottled.** The same run's `e2e/perf.spec.ts` measured home LCP at 456 ms (median of 2,012, 456 and 424), a post opening in 46 ms, and the slowest interaction at 56 ms.
