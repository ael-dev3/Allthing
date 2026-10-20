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

The CI `perf` job runs `e2e/perf.spec.ts` against https://allthing.social after every push to `main` (and on demand through `workflow_dispatch`). Before measuring, it waits for `/version` to report the checked-out commit (the triggering commit for a push, or current `main` for a manual run), or a later commit on `main` that includes it. Workers Builds skips a commit that a newer push supersedes: of five merges made within 33 seconds on 19 October 2026 it built the first and the last, so a commit can be live only as part of a later one, which the job tells from the repository's history. It polls every 10 seconds for at most 30 minutes, aborts an individual version read after five seconds, and fails without benchmarking if that release never arrives. Workers Builds takes 6 to 16 minutes from a push until the commit is live, and about 25 when a second merge queues behind the first: the five-minute wait it replaced gave up 21 seconds before #144 went live on 19 October 2026, and from its introduction (#133) the job failed on every push to `main` it ran for, without measuring anything. This keeps a pending deployment from passing the budget with the previous release's results.

## How the app meets it

### 1. Reuse what the page already has

- Every cast that a list renders, and every cast it quotes, goes into a client-side store (`api::remember_casts`, at most 2,000 entries).
- `/p/<hash>` reads that store first (`api::seen_cast`). If the cast is there, the post renders at once with its text, media, author and counts, the quoted casts the feed carried, and reply skeletons. The full payload (thread, parent, quotes) loads behind it and replaces the skeletons.
- A direct visit with nothing in the store falls back to the normal one-request load.
- `/u/<name>` does the same for profiles. The store also keeps each cast's author by username (`api::seen_person`), so a profile opened from a feed shows its header at once: the picture, names and fid, and, from the author's profile card (`api::seen_card`), the start of the bio, the counts and the join date (`allthing_core::profile::ProfilePreview`). Its casts load behind it. Without a card, the bio and counts are placeholders; a join date that the profile would correct (an account from before November 2023, whose card can't date it) is left out until the payload arrives.
- **Profile cards** open with what the page's payload already says about the account (`notes.users`: the start of its bio, its counts, when it joined), or what an earlier page of the visit showed (`api::seen_card`), so they show at once. The vote and wallet activity are preloaded for the accounts a reader is likely to open (the authors of feed cards in view, the people of a [preview](#popover-previews) the reader comes into) and read synchronously, so the card shows them in its first frame. On a touch screen the same holds for a card tapped open: its authors' facts load for cards the reader stops at (still in view after 600 ms), 6 authors per 5 minutes at most, against 12 with a mouse. A card whose vote or wallets are still on their way waits for them out of sight (`settling`), for half a second at most; a read slower than that shows its placeholders, which are the size of what replaces them, so the card never grows when they come (it grew by 27 px for two chains, and by 115 px for five chains and a note, before 19 October 2026). Either answer is reused for 5 minutes.

- **A page's data is requested by a one-line inline script** at the top of `index.html`, so it starts while the HTML is still being parsed, alongside the WebAssembly download. `api.rs` (`early_response`) takes that pending response instead of fetching again.
  - It covers the pages people arrive on: the front page (its sort and time range), `/p/<hash>`, `/u/<name>` and `/c/<id>`. It normalizes the hash, name or id as the app does (lowercase; an `0x` or `@` prefix), so the paths match; anything it doesn't recognize, such as a name with other characters, is left to the app.
  - On production the post, profile and channel requests used to start only once the app had booted, about 450 ms in (the front page's always started at once). The response is still the page's one API call.
  - The script is allowed by its hash in the CSP (`web/_headers`), and `scripts/check.sh` fails if the two disagree.
  - Starting the request from `boot.js` was tried first, but a module script has to download before it runs, so the request still began about when the app's own JS arrived (700–990 ms on production).
  - The static copy, served under `/Allthing/`, skips it.

### 2. Prefetch, under a strict budget

- **Hover or touch** on a card prefetches `/api/v1/cast?hash=…` (`pointerenter`, `touchstart`).
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

Who liked and who recasted a feed card show the moment their popover opens: the feed preloads them (`crates/allthing-web/src/preload.rs`).

- **Cards in view.** The shared observer above reports cards coming into view and leaving it. Once the cards in view have stayed put for 200 ms, the topmost one whose batch this page hasn't asked for brings it.
- **Batches follow the list.** A card brings its run of six cards, in list order (cards 1–6, 7–12, …), as one `/api/v1/engagement` request, with a list only for counts above zero. Readers of the same feed ask for the same batches, so the edge cache answers most of them, and upstream reads don't grow with readers.
- **Filed as single answers.** Each list goes into the client cache under its own `/api/v1/reactions?…&limit=10` path, with the batch's arrival time and age, so it expires, and reports its age, as if it had been asked for alone. An answer already kept that was read upstream later stays. A popover that opens while its batch is on its way waits for it instead of asking again, and a list the batch lacks (its read failed, or the Worker kept Hypersnap's bucket for page builds) loads on hover, as before.
- **No "Loading…" flash.** A preview reads the client cache synchronously, so its people are there in the frame the popover opens. A resource would resolve a tick later, even from the cache.
- **Replies** come from the post payload that hovering the card prefetched, whose thread holds the direct replies, so they need no request of their own once it is in.
- **The people in a preview.** Once the reader's pointer or keyboard focus comes into a likes, recasts or replies preview's list, the vote and wallet activity of everyone it names load (`api::prefetch_row_facts`), so the card a name opens is complete. A list the pointer passes without coming into loads nothing; on a touch screen, where a tap opened the list, they load at once. They go ahead of feed authors' in the card queue, in the list's order, two in flight with the other prefetches; a name opened before its turn reads its own at once. Anyone whose facts are kept or on their way is skipped, and at most 20 people are queued per 5 minutes (40 requests, both endpoints edge-cached for an hour or more). The full Liked by and Recasted by lists, up to 100 people, aren't preloaded: their cards read their facts when they open.
- **Limits**, per page view (a new URL, or a list rendered anew, as after Back):
  - one batch on its way at a time, and batches start at least 2 s apart;
  - at most 4 batches, 24 cards; later cards load their previews on hover;
  - batches share the prefetch cap (2 in flight);
  - nothing is preloaded with `Save-Data` or on `2g`/`slow-2g` connections;
  - nor in the static copy, whose cards aren't observed.
- **Why these numbers.**
  - A batch is at most 12 Hypersnap reads (six cards, likes and recasts): 14 subrequests with the cache read and write, against a budget of 45, and about 2 ms of parsing when every list is full ([BUDGET.md](BUDGET.md#4-cpu-time)).
  - An isolate's Hypersnap bucket holds 40 tokens and refills 10 a second. One reader's batches, at most 12 reads every 2 s, ask for 6 a second at most, so even if every batch missed the edge cache and reached the same isolate, a scrolling reader couldn't drain it. Four batches cap a page view at 48 reads.
  - The Worker adds its own floor: previews never take the bucket below 16 tokens, so page builds keep a share when many readers' batches meet in one isolate. A batch with less room reads what fits (likes first), is kept for 15 s, and is rebuilt.
- **Cost**: one Worker request per batch, so a feed costs one to four more requests (usually one or two), with a mouse or on a touch screen, while hovering or tapping a preloaded count costs none. See [BUDGET.md](BUDGET.md).

### 3. Client-side navigation

- Links are handled by the Leptos router, so there are no full reloads.
- Clicking anywhere on a card navigates (`nav::card_click`), unless the click lands on a link or button, or text is being selected. A click on the card of a cast it quotes opens the quoted cast instead. Ctrl-, ⌘- and shift-click open a new tab as with any link.
- Opening a post from a card goes straight to the router, like a link, and the post rises in with a short CSS animation (off under `prefers-reduced-motion`). There are no view transitions: snapshotting a long feed before each navigation cost about 100 ms.
- **Scroll**: `history.scrollRestoration` is `manual`, and `nav.rs` (`ScrollMemory`) restores each page's place on back and forward.
  - **What it saves**: the card under the line just below the header and its offset in the viewport, plus the pixel position for pages without cards. It saves on scroll and, in the capture phase, on every click, so that the scrolls a navigation causes don't overwrite it.
  - **What comes back**: lists show as many cards as before (`CastList` remembers per page), and the front page keeps the later trending pages it had loaded for 5 minutes.
  - **How long it holds**: after Back, the card is put back and held there for 6 s against late changes (images loading above it, lists growing back, a late scroll-to-top). The reader's own wheel, touch, key or mouse input ends the hold.
  - **Images keep their size**: a single image's natural size is remembered once it has loaded, and the image is drawn at that size when the list is rendered again. Back therefore doesn't make the page grow while images arrive, including after the hold ends.
  - Sort tabs and reply toggles use `data-noscroll`, so they never jump to the top.
- **Sorting** reorders the existing list (a keyed `For` over a derived signal) instead of rebuilding the page.
- **Drawing a screenful at a time.** A list rendered anew (a sort tab, another page) draws 6 cards, about a screenful, in the task that asked for it, and 6 more in a task after each paint until it has drawn what it shows (`CastList`, `DRAW_STEP`). A re-sort that changes the top of the list starts again from there. The tap's frame paints at once: building all 25 cards in the click took 13–39 ms of WebAssembly on a fast desktop CPU, which put the slowest interaction over 100 ms on CI runners. Back and forward draw the whole list at once while they restore the reader's place (`nav::restoring`).

### Animations that never stop

- An animation that runs all the time moves by `transform` or `opacity`, which the compositor animates without the main thread. The header's line of light slides a strip twice its gradient's width by one gradient (`.portal-line::before`).
- When it animated `background-position` instead, every frame repainted the header and redid its backdrop blur, and the frame after each tap or keystroke waited behind that work: interactions took 40–64 ms in a headless browser without a GPU, and 16–32 ms after the change.
- The wordmark's sheen still animates its background, but it holds still for most of its 14 s cycle.

### 4. Edge

- **Stale-while-revalidate** on every API payload, with request coalescing per isolate and stale-if-error. Policies are in `allthing_core::api` and listed in [BUDGET.md](BUDGET.md#2-edge-cache-one-upstream-fetch-serves-everyone-nearby).
- **Server time**: every API answer says how long the Worker spent on it in a `Server-Timing` header, named after how it was served (`fresh;dur=3`, or `miss;dur=1840` for a build the request waited for). Browsers show it in their network panel and in `PerformanceResourceTiming.serverTiming`. Clocks in a Worker only advance across I/O, so it measures waiting on the cache and upstream, not CPU time.
- **Compact JSON**: payloads skip empty and default fields (`skip_serializing_if`). Cloudflare compresses responses with Brotli for clients that accept it.
- **Immutable assets**: the app's `.wasm`, `.js` and `.css` are fingerprinted and served with `Cache-Control: public, max-age=31536000, immutable` (`web/_headers`).
- **The WebAssembly at brotli's highest level**: the edge compresses static assets on the fly at a fast level, which left the 1.5 MB module at about 550 KB. The build also writes a copy compressed at quality 11 (`<name>.wasm.br`, about 434 KB), and the page loads the module from `/wasm/<name>`, which the Worker answers with that copy (`Content-Encoding: br`, passed through as it is) to browsers that accept brotli, and with the module itself to the rest. That is a fifth less to download before anything renders. If the Worker can't answer, `boot.js` loads `/assets/<name>`, the static copy.
- **Type-erased views**: the browser app is built with Leptos's `--cfg erase_components` (`.cargo/config.toml`), so each component's view and each reactive closure in it is boxed instead of monomorphizing the view machinery for it. The module went from 1,524,962 to 1,395,957 bytes, and from 440,631 to 393,099 bytes at brotli 11 (47.5 KB, a tenth, less to download before anything renders: about 240 ms on a 1.6 Mbps "slow 4G" link, 40 ms on 9 Mbps). The cost is a dynamic call per closure. Measured on 19 October 2026 on a CI runner, the two builds served locally with the same live data, alternating, median of 9: the first cards drew 3–6 ms later unthrottled (69 → 72 ms at 1280 px, 62 → 68 ms at 390 px, from instantiation) and 15–21 ms later with the CPU slowed 4×; switching sort tabs took the same 56 ms at 1280 px and 32 → 48 ms at 390 px, within the 100 ms budget. A release `opt-level = "s"` was tried too, and was larger (1,695 KB).
- **Sign-in costs browsers little**: its cryptography and QR encoder run in the Worker, and the browser draws the code from the SVG path the Worker sends. The header button, dialog and account menu added 15.2 KB to the module at brotli 11 (395,279 → 410,508 bytes, measured on 19 October 2026 from release builds), about 76 ms more on a 1.6 Mbps "slow 4G" first visit and 12 ms on 10 Mbps. A signed-in page view still makes one API call: the header reads who is signed in from a cookie.
- **Liking and recasting** cost every reader 29.5 KB of the module at brotli 11 (434,181 → 463,702 bytes, release builds on 19 October 2026): the approval dialog, the key's storage and signing, the message encoding and its BLAKE3 hash. That is about 150 ms more on a 1.6 Mbps "slow 4G" first visit and 24 ms on 10 Mbps. The browser's own WebCrypto does the Ed25519 signing, and answers from the Farcaster app's API and the node are read as `serde_json::Value`, one JSON reader instead of a deserializer per answer, which saved 3.5 KB of the 33 it first took. A signed-in reader's page view makes no more API calls: `/api/v1/signer` is asked once an hour, and what they liked is read from Hypersnap by the browser.
- **Casting, replying and quoting** with the reader's key added 8.1 KB more (463,702 → 471,779 bytes at brotli 11): the cast's encoding, mentions and the composer's post button. Mentions are looked up only when a cast is posted.
- **Following and deleting** added 7.1 KB more (471,779 → 478,838 bytes at brotli 11): Follow and its lookup, Delete and its question, and a deleted cast shown as deleted in feeds, posts and threads. A profile view costs a reader who can follow one Hypersnap read from the browser, and no Worker request.
- **Emoji** added 2.9 KB to the module at brotli 11 (483,806 → 486,790 bytes, release builds on 20 October 2026): the pack's table, the shortcodes and their view. The images cost nothing until a cast shows one. Each is a static file under `/assets/emoji/v1/`, cached for a year and loaded lazily: a median of 17 KB and 133 KB at most, or with reduced motion a still of about 2 KB. A page view makes no more Worker requests.
- **The emoji picker** added 8.6 KB (486,790 → 495,615 bytes at brotli 11): its grid, search and recently used, and the composer's caret handling. Two choices kept it small:
  - its tiles share six listeners on the grid, instead of six each;
  - its results are drawn anew for each search instead of as a keyed list, which saved 3 KB.

  Opening it the first time loads the stills in view, about 110 KB at 1280 px (all 179 are 402 KB), from the year-long cache after that. It makes no Worker requests.
- **Stickers** added 4.7 KB more (495,615 → 500,468 bytes at brotli 11): the sticker button and the picker's Stickers tab with its recently used, a sticker as the cast's attachment, and stickers in casts. A cast's sticker loads its GIF from the site itself, cached for a year: a median of 74 KB (12 to 269 KB), or with reduced motion a still of about 4 KB. The picker's tab shows the stills until a sticker is pointed at. No Worker requests.
- **Handing writes to the Farcaster app** added 5.8 KB (551,831 → 557,797 bytes at brotli 11, release builds on 20 October 2026): its dialog, the links to casts, profiles and drafts, and the like, recast, follow, delete and composer buttons shown to every reader. The QR code is drawn by the Worker, as sign-in's is, so the browser ships no encoder; a phone asks for none.
- **A bigger Pepe pack**, 179 emoji where there were 103, added 1.1 KB (558,162 → 559,284 bytes at brotli 11, release builds on 20 October 2026): the table's 76 new rows, and each emoji's group. The new images, a median of 16 KB and 100 KB at most, cost nothing until a cast or the picker shows them.
- **The emoji picker's groups** added 2.8 KB (559,284 → 562,128 bytes at brotli 11): headings, the bar of groups with its faces, and following the grid's scroll to mark the group in view. The bar's faces are 11 of the stills the grid shows anyway.
- **Preconnects** to `imagedelivery.net` and `stream.farcaster.xyz`, where nearly all media is hosted.
- **The sound badge and looping** added 1.7 KB to the module (2,080,264 → 2,081,974 bytes) and nothing measurable at brotli 11 (562,328 → 562,142 bytes, release builds on 20 October 2026; on the main before the groups' bar the same change measured +1.0 KB, as a small change reorders the module and moves brotli's result by about a kilobyte either way): the badge's speaker, and the player's going on segment by segment and ending a stream once its last segment is in. A playlist that drifts from its media now costs one request per segment, where the player had fetched its last two segments in turn without end.

### 5. Infinite scroll

- Lists show 25 more cards at a time, drawn a screenful per task as above, so scrolling never waits on a long task that builds 25 cards.
- A sentinel 1,200 px below the last card shows the next 25 before the reader reaches it.
- On the front page, once the loaded pool runs out, the next trending page (`/api/v1/home?offset=100`, … up to 900) is fetched at the same point, so the next cards are usually ready before they are needed.
- The end is checked again after cards or a server page arrive. Short and empty filtered pages keep filling without requiring the sentinel to leave and re-enter the viewport; failures still wait for retry backoff, and loading stops at offset 900.

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

### Speed report

`e2e/speed-report.spec.ts` loads the front page cold, seven times per profile, under fixed network and CPU profiles, and prints the medians. It reports and asserts nothing about the budget, which `e2e/perf.spec.ts` checks:

| Profile | Viewport | Network | CPU |
|---|---|---|---|
| Desktop | 1280 × 900 | 10 Mbps down, 40 ms RTT | as is |
| Phone ("slow 4G") | 390 × 844, touch | 1.6 Mbps down, 150 ms RTT | slowed 4× |

It measures time to first byte, when the WebAssembly and the front page's data are in (and how many KB the WebAssembly took on the wire), the first cast card, LCP (the later of the LCP entry and the first card, as the budget counts it), and the slowest of two sort-tab switches (Event Timing). It records the deployed commit before and after its runs, and fails if a deploy landed in between.

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
