<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Performance

Navigation should feel instant: a post opens as soon as it is clicked, the feed keeps scrolling without waiting, and nothing a reader does takes long enough to notice. This page sets the budget, explains how the app meets it within the Workers Free plan ([BUDGET.md](BUDGET.md)), and records measurements.

## Budget

| Metric | Target | Measured by |
|---|---|---|
| Opening a post from the feed (click → post text on screen) | **< 100 ms** | `e2e/perf.spec.ts` |
| Largest Contentful Paint, home, cold load | **< 1.5 s** (median of 3) | `e2e/perf.spec.ts` (the later of the LCP entry and the first cast card) |
| Interaction to Next Paint (slowest interaction) | **< 100 ms** | `e2e/perf.spec.ts` (Event Timing entries with an `interactionId`) |
| API response from the edge cache | < 50 ms server time | `x-allthing-cache: fresh` responses |

The CI `perf` job runs `e2e/perf.spec.ts` against https://allthing.social after every push to `main` (and on demand through `workflow_dispatch`), so a regression in production shows up as a red check.

## How the app meets it

### 1. Reuse what the page already has

- Every cast that a list renders, and every cast it quotes, goes into a client-side store (`api::remember_casts`, at most 2,000 entries).
- `/p/<hash>` reads that store first (`api::seen_cast`). If the cast is there, the post renders at once with its text, media, author and counts, the quoted casts the feed carried, and reply skeletons. The full payload (thread, parent, quotes) loads behind it and replaces the skeletons.
- A direct visit with nothing in the store falls back to the normal one-request load.

- **The front page's data is requested by a one-line inline script** at the top of `index.html`, so it starts while the HTML is still being parsed. `api.rs` (`early_response`) takes that pending response instead of fetching again.
  - The script is allowed by its hash in the CSP (`web/_headers`), and `scripts/check.sh` fails if the two disagree.
  - Starting the request from `boot.js` was tried first, but a module script has to download before it runs, so the request still began about when the app's own JS arrived (700–990 ms on production).
  - The static copy, served under `/Allthing/`, skips it.

### 2. Prefetch, under a strict budget

- **Hover or touch** on a card prefetches `/api/v1/cast?hash=…` (`pointerenter`, `touchstart`).
- **Viewport**: a shared `IntersectionObserver` notices cards that are at least 60% visible for 600 ms. It then prefetches them in idle time (`requestIdleCallback`), at most 6 per minute.
- **Speculation Rules** (`web/index.html`) let supporting browsers prefetch `/p/*` documents with `moderate` eagerness (on hover). The document is the static app shell, so this costs no Worker request. The CSP allows it with `'inline-speculation-rules'`.
- **Limits**, in `crates/allthing-web/src/api.rs`:
  - at most 2 prefetches in flight;
  - duplicates are skipped, both in flight and when already cached;
  - a page request that matches an in-flight prefetch waits for it instead of fetching twice;
  - nothing is prefetched with `Save-Data` or on `2g`/`slow-2g` connections;
  - the static GitHub Pages copy, which has no shared cache, never prefetches from the viewport.
- **Cache**: answers stay in a client cache for 5 minutes, up to 200 entries, so back, forward and reopened posts cost nothing.

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

### 4. Edge

- **Stale-while-revalidate** on every API payload, with request coalescing per isolate and stale-if-error. Policies are in `allthing_core::api` and listed in [BUDGET.md](BUDGET.md#2-edge-cache-one-upstream-fetch-serves-everyone-nearby).
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
| Edge cache cold for that post | 840–1,350 ms | 20–28 ms |
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
