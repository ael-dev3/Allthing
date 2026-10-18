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

The CI `perf` job runs `e2e/perf.spec.ts` against https://allthing.social after every push to `main` (and on demand through `workflow_dispatch`), so a regression in production shows up as a red check.

## How the app meets it

### 1. Reuse what the page already has

- Every cast that a list renders, and every cast it quotes, goes into a client-side store (`api::remember_casts`, at most 2,000 entries).
- `/p/<hash>` reads that store first (`api::seen_cast`). If the cast is there, the post renders at once with its text, media, author and counts, the quoted casts the feed carried, and reply skeletons. The full payload (thread, parent, quotes) loads behind it and replaces the skeletons.
- A direct visit with nothing in the store falls back to the normal one-request load.
- **Profile cards** open with what the page's payload already says about the account (`notes.users`: the start of its bio, its counts, when it joined), or what an earlier page of the visit showed (`api::seen_card`), so they show at once. Only the vote and wallet activity load once the card is open, reused for 5 minutes like any answer, and the label shard, read once per visit.

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
- **Limits**, per page view (a new URL, or a list rendered anew, as after Back):
  - one batch on its way at a time, and batches start at least 2 s apart;
  - at most 4 batches, 24 cards; later cards load their previews on hover;
  - batches share the prefetch cap (2 in flight);
  - nothing is preloaded with `Save-Data` or on `2g`/`slow-2g` connections;
  - nor where no pointer can hover: on a touch screen a tap opens the popover, and loads it then;
  - nor in the static copy, whose cards aren't observed.
- **Why these numbers.**
  - A batch is at most 12 Hypersnap reads (six cards, likes and recasts): 14 subrequests with the cache read and write, against a budget of 45, and about 2 ms of parsing when every list is full ([BUDGET.md](BUDGET.md#4-cpu-time)).
  - An isolate's Hypersnap bucket holds 40 tokens and refills 10 a second. One reader's batches, at most 12 reads every 2 s, ask for 6 a second at most, so even if every batch missed the edge cache and reached the same isolate, a scrolling reader couldn't drain it. Four batches cap a page view at 48 reads.
  - The Worker adds its own floor: previews never take the bucket below 16 tokens, so page builds keep a share when many readers' batches meet in one isolate. A batch with less room reads what fits (likes first), is kept for 15 s, and is rebuilt.
- **Cost**: one Worker request per batch, so a feed read with a mouse costs one to four more requests (usually one or two), while hovering a preloaded count costs none. See [BUDGET.md](BUDGET.md).

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

### Animations that never stop

- An animation that runs all the time moves by `transform` or `opacity`, which the compositor animates without the main thread. The header's line of light slides a strip twice its gradient's width by one gradient (`.portal-line::before`).
- When it animated `background-position` instead, every frame repainted the header and redid its backdrop blur, and the frame after each tap or keystroke waited behind that work: interactions took 40–64 ms in a headless browser without a GPU, and 16–32 ms after the change.
- The wordmark's sheen still animates its background, but it holds still for most of its 14 s cycle.

### 4. Edge

- **Stale-while-revalidate** on every API payload, with request coalescing per isolate and stale-if-error. Policies are in `allthing_core::api` and listed in [BUDGET.md](BUDGET.md#2-edge-cache-one-upstream-fetch-serves-everyone-nearby).
- **Server time**: every API answer says how long the Worker spent on it in a `Server-Timing` header, named after how it was served (`fresh;dur=3`, or `miss;dur=1840` for a build the request waited for). Browsers show it in their network panel and in `PerformanceResourceTiming.serverTiming`. Clocks in a Worker only advance across I/O, so it measures waiting on the cache and upstream, not CPU time.
- **Compact JSON**: payloads skip empty and default fields (`skip_serializing_if`). Cloudflare compresses responses with Brotli for clients that accept it.
- **Immutable assets**: the app's `.wasm`, `.js` and `.css` are fingerprinted and served with `Cache-Control: public, max-age=31536000, immutable` (`web/_headers`).
- **Preconnects** to `imagedelivery.net` and `stream.farcaster.xyz`, where nearly all media is hosted.

### 5. Infinite scroll

- Lists render 25 cards at a time.
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
