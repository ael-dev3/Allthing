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
- **The front page's first cards are drawn from that data before the app has come** (`web/paint.js`). On a slow line the data comes seconds before the WebAssembly (on the speed report's phone, at about 1.0 s against 3.1 s), so a small script draws the cards the app draws first: in the app's order (hot or top, with the reader's favorites and those they see less of), with the app's markup and the usage bar above them, under the boot screen's header. They only show: they're inert and hidden from screen readers. Once mounted, the app is drawn hidden under them, and when its own cards are drawn, in the same boxes, they take their place (`boot.rs`), without their entrance animation again. The [speed report](#speed-report) times both: the first cast on screen, and the app's own first.
  - Only the front page, for hot and top over the past hour, day or week, and only from a fresh answer (not one the Worker marks stale). At most the first 6 cards, and never past one the app draws otherwise: a quote, a video, a sticker, or a single image it can't size (one not on Cloudflare Images, such as a Tenor or KLIPY GIF).
  - A single image's tile has its final box before the image comes. Cloudflare Images describes the variant a feed shows (`…/fit=scale-down,w=720,format=json`, about 110 bytes, sent with `access-control-allow-origin: *`), the tile takes that size, and so does the app's tile for the same image (`media::known_size`). A card with one is drawn once its description has come, a round trip after the data. Nothing on the first screen moves as its images come, there or after the app takes over; without this, a single image grew its card when it loaded, pushing the cards below it down.
  - Its images (avatars, emoji, link thumbnails and media) wait until the WebAssembly is in, and the app's own cards then find them on their way. Asked for at once, in a trial on the speed report's phone, they shared the slow line with it and the app's first card came 1.4 s later.
  - If the app can't load, `boot.js`'s panel replaces the cards. Leaving the front page, or its data failing in the app, hands over at once, and the app's cards replace them at most 4 s after the app has mounted, whatever else.
  - It is 38 KB (11 KB at brotli), and adds no request to the Worker. The static copy doesn't load it.

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
- **Then the next run, ahead of the reader.** Once the runs in view have their batches, the run after the last card in view brings its own when its first card is less than a screen below the viewport, so a reader reading down finds its popovers already loaded. It waits until all six of its cards are on the page: a run the next page is still to fill would be asked for twice. A run the reader scrolls past before its turn isn't loaded; its popovers load when they open.
- **Batches follow the list.** A card brings its run of six cards, in list order (cards 1–6, 7–12, …), as one `/api/v1/engagement` request, with a list only for counts above zero. Readers of the same feed ask for the same batches, so the edge cache answers most of them, and upstream reads don't grow with readers.
- **Filed as single answers.** Each list goes into the client cache under its own `/api/v1/reactions?…&limit=10` path, with the batch's arrival time and age, so it expires, and reports its age, as if it had been asked for alone. An answer already kept that was read upstream later stays. A popover that opens while its batch is on its way waits for it instead of asking again, and a list the batch lacks (its read failed, or the Worker kept Hypersnap's bucket for page builds) loads on hover, as before.
- **No "Loading…" flash.** A preview reads the client cache synchronously, so its people are there in the frame the popover opens. A resource would resolve a tick later, even from the cache.
- **Replies** come from the post payload that hovering the card prefetched, whose thread holds the direct replies, so they need no request of their own once it is in.
- **The people in a preview.** As a likes, recasts or replies preview's list opens, the vote and wallet activity of everyone it names load (`api::prefetch_row_facts`), so the card a name opens is complete. A list opens on a tap, or once the pointer has rested on its count for 280 ms, so a count the pointer only crosses loads nothing. They go ahead of feed authors' in the card queue, in the list's order, two in flight with the other prefetches. The name the reader points at or touches goes first (`api::prefetch_facts_first`), about 100 ms before its card opens, and outside the 5-minute count, as the card would read its facts then anyway; one opened before its turn reads its own at once. Anyone whose facts are kept or on their way is skipped, and at most 30 people (three lists) are queued per 5 minutes (60 requests, both endpoints edge-cached for an hour or more). The full Liked by and Recasted by lists, up to 100 people, aren't preloaded: their cards read their facts when they open.
- **Limits**, per page view (a new URL, or a list rendered anew, as after Back):
  - one batch on its way at a time, and batches start at least 2 s apart;
  - at most 8 batches, 48 cards, about the front page's first two pages; later cards load their previews when they open;
  - batches share the prefetch cap (2 in flight);
  - nothing is preloaded with `Save-Data` or on `2g`/`slow-2g` connections;
  - nor in the static copy, whose cards aren't observed.
- **Why these numbers.**
  - A batch is at most 12 Hypersnap reads (six cards, likes and recasts): 14 subrequests with the cache read and write, against a budget of 45, and about 2 ms of parsing when every list is full ([BUDGET.md](BUDGET.md#4-cpu-time)).
  - An isolate's Hypersnap bucket holds 40 tokens and refills 10 a second. One reader's batches, at most 12 reads every 2 s, ask for 6 a second at most, so even if every batch missed the edge cache and reached the same isolate, a scrolling reader couldn't drain it. Eight batches cap a page view at 96 reads.
  - The Worker adds its own floor: previews never take the bucket below 16 tokens, so page builds keep a share when many readers' batches meet in one isolate. A batch with less room reads what fits (likes first), is kept for 15 s, and is rebuilt.
- **Cost**: one Worker request per batch, so a feed costs one to eight more requests (usually one to three), with a mouse or on a touch screen, while hovering or tapping a preloaded count costs none. See [BUDGET.md](BUDGET.md).

### 3. Client-side navigation

- Links are handled by the Leptos router, so there are no full reloads.
- Clicking anywhere on a card navigates (`nav::card_click`), unless the click lands on a link or button, or text is being selected. A click on the card of a cast it quotes opens the quoted cast instead. Ctrl-, ⌘- and shift-click open a new tab as with any link.
- Opening a post from a card goes straight to the router, like a link, and the post rises in with a short CSS animation (off under `prefers-reduced-motion`). There are no view transitions: snapshotting a long feed before each navigation cost about 100 ms.
- **Scroll**: `history.scrollRestoration` is `manual`, and `nav.rs` (`ScrollMemory`) restores each page's place on back and forward.
  - **What it saves**: the card under the line just below the header and its offset in the viewport, plus the pixel position for pages without cards. It saves on scroll and, in the capture phase, on every click, so that the scrolls a navigation causes don't overwrite it.
  - **What comes back**: lists show as many cards as before (`CastList` remembers per page), and the front page keeps the later trending pages it had loaded for 5 minutes.
  - **How long it holds**: after Back, the card is put back and held there for 6 s against late changes (images loading above it, lists growing back, a late scroll-to-top). The reader's own wheel, touch, key or mouse input ends the hold.
  - **Images keep their size**: a single image's natural size is remembered once it has loaded, and the image is drawn at that size when the list is rendered again. Back therefore doesn't make the page grow while images arrive, including after the hold ends. A single GIF, drawn at most 300 px each way (70% of the card's width on a phone), takes its smaller box at once the same way.
  - Sort tabs and reply toggles use `data-noscroll`, so they never jump to the top.
- **Sorting** reorders the existing list (a keyed `For` over a derived signal) instead of rebuilding the page.
- **Drawing a screenful at a time.** A list rendered anew (a sort tab, another page) draws 6 cards, about a screenful, in the task that asked for it, and 6 more in a task after each paint until it has drawn what it shows (`CastList`, `DRAW_STEP`). A re-sort that changes the top of the list starts again from there. The tap's frame paints at once: building all 25 cards in the click took 13–39 ms of WebAssembly on a fast desktop CPU, which put the slowest interaction over 100 ms on CI runners. Back and forward draw the whole list at once while they restore the reader's place (`nav::restoring`).
- **The tap first, the feed after.** A sort tab answers in the frame after the tap: the tab shows as chosen at once, and the feed follows in a task of its own once that frame is painted (`after_a_paint` in `pages/home.rs`), then draws a screenful at a time as above. Switching between hot and new swaps the whole feed, tearing down the old cards and building new ones. On a phone with the CPU slowed 4×, that work made the tap wait for its next frame. New also read its client filter inside its view. A tap that changed only the sort changes the query too, so new redrew its 25 cards just before they were torn down. The filter is now a memo of its own. Measured on 20 October 2026 as the speed report measures, with the two builds served locally on the same data, alternating, median of 9:
  - the slowest of the new and hot taps went from 144 ms to 48 ms (40–56) on the phone profile;
  - at 1280 px it went from 56 ms to 32 ms (median of 7);
  - the first cast came 7 ms later (4,003 → 4,010 ms), the time the module's extra 0.7 KB takes.

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
- **The DAU page's dashboard** is the first part (`split_dau.wasm`, 39 KB, 16.9 KB at brotli 11): its tiles, chart, tables and readouts. /dau fetches it while it fetches the counts. The module went from 2,145,244 to 2,119,212 bytes, and from 579,624 to 572,909 bytes at brotli 11: 6.6 KB (1.2%) less to download before anything renders, about 34 ms on a 1.6 Mbps "slow 4G" first visit (release builds on 20 October 2026). Until the dashboard's code has come, the page says it is loading the counts, as before.
- **The composer** is the next (`split_composer.wasm`, 131 KB, 48.7 KB at brotli 11), with **the emoji and sticker picker** it shares with React (24.7 KB at brotli 11): the GIF, emoji and sticker pickers, scheduling, the bar and posting. The dialog's frame, its title and the text box stay in the module, so the composer opens at once and takes typing the first time too, while the bar shows a quiet "Loading…" in the post button's place. Pointing at or focusing Cast, Reply or Quote, or pressing one on a touch screen, starts fetching its code, so it usually comes before the click does. React's picker is a part of its own (0.3 KB) with the shared picker, fetched as React is pointed at, focused or pressed. A part of a page that can't be fetched says so in its place, with Reload, and doesn't reload the page by itself, so a draft isn't lost unasked. The module went from 2,119,212 to 1,958,089 bytes, and from 572,909 to 537,600 bytes at brotli 11: 34.5 KB (6.2%) less to download before anything renders, about 177 ms on a 1.6 Mbps "slow 4G" first visit (release builds on 20 October 2026).
- **Signing in and /scheduled** came next. Signing in, from the relay's code to the Worker's check of the approval (`split_signin.wasm`, 9.1 KB at brotli 11), is fetched as Sign in is pointed at, focused or pressed (`signin::warm`), or when the dialog opens; the dialog itself, and the account's picture and menu, stay in the module. If it can't be fetched, the dialog says so, and Get a new code tries again. /scheduled's list (`split_scheduled.wasm`, 15.6 KB) comes with the page, which says it is loading the casts meanwhile; it shares 3.2 KB with the composer. The module went from 1,958,089 to 1,899,433 bytes, and from 537,600 to 522,250 bytes at brotli 11: 15.0 KB (2.9%) less, about 77 ms on a 1.6 Mbps "slow 4G" first visit (release builds on 20 October 2026).
- **Writing** is a part too (`split_writer.wasm`, 13.5 KB at brotli 11, with 7.6 KB it shares with the composer and the others). It holds signing a like, a recast, a follow, a delete or a cast and handing it to the node; approving a key and reading the one this browser keeps; and looking on the node for what was handed to the Farcaster app. A signed-in reader's browser fetches it as soon as the page knows who they are, so it's in before their first write, and before the first look on the node for what they handed to the Farcaster app. A signed-out reader's browser never fetches it, since their writes go to the Farcaster app. If it can't be fetched, a signed-in reader's writes go to the Farcaster app too, as where Allthing doesn't write for them. The module went from 1,899,433 to 1,856,726 bytes, and from 522,250 to 508,003 bytes at brotli 11: 14.2 KB (2.7%) less, about 71 ms on a 1.6 Mbps "slow 4G" first visit (release builds on 20 October 2026). Throttled as the speed report's phone, alternating with the build before it, median of 9: the WebAssembly was in 74 ms sooner (3,407 → 3,333 ms) and the first cast came 85 ms sooner (3,755 → 3,670 ms).
- **Pages of their own.** A post's page, a profile's, a channel's, search's and the channels' are parts too: `split_post.wasm` (34.4 KB at brotli 11), `split_user.wasm` (26.1 KB), `split_channel.wasm` (15.6 KB), `split_search.wasm` (10.3 KB) and `split_channels.wasm` (8.3 KB), with the code they share in small chunks. Their code comes as it's likely wanted (`split::preload_pages`), so a page opens as it did when its code came with the app:
  - a post's a few seconds after the app starts, once the browser is idle (opening a post is how most visits go on from a feed; not on a slow or metered connection), and as a card is pointed at or touched, with its data;
  - a profile's, a channel's, search's and the channels' as a link to one is pointed at, touched or focused, anywhere on the page, and search's as the search box is. The pages' parts are 94.8 KB together at brotli 11, compressed apart, so fetching them all on every visit would cost more than the module saves;
  - and with the app, when a visit starts on one. `index.html` lists each page's files (its `allthing-parts` meta tag, filled in by `cargo xtask`), and its `early-parts` script preloads them; the loader then uses what was preloaded.

  Until a page's code has come, two placeholder cards show. The module went from 1,857,056 to 1,622,512 bytes, and from 508,082 to 456,187 bytes at brotli 11: 51.9 KB (10.2%) less, about 259 ms on a 1.6 Mbps "slow 4G" first visit (release builds on 20 October 2026). Throttled as the speed report's phone, alternating with the build before it, median of 9: the WebAssembly was in 260 ms sooner (3,331 → 3,071 ms) and the first cast came 261 ms sooner (3,664 → 3,403 ms).
- **A feed card's emoji reactions** are a part too (`split_chips.wasm`, 10.3 KB, 4.4 KB at brotli 11), with code they share with a post's page (10.5 KB at brotli 11 in four small chunks, which the post's code needs anyway and the app fetches a few seconds in, but on a slow or metered connection). A card's chips call it the first time a card has some (`reactions::card_reactions`), right as the listing's first cards are drawn, so their code comes while the page's images do; it costs no Worker request. Until it comes, an empty row of the chips' height (28 px; a chip's `li` is a flex box, so the line's descent adds nothing) holds their place, so nothing below moves, and if it can't come, the cards go without them, with no message on each (`split::lazy_extra`). Inlined, the chips added 29.8 KB to the module and 9.5 KB at brotli 11 (1,642,062 → 1,671,826 bytes, and 459,799 → 469,288); as a part they add 8.5 KB and 1.8 KB (→ 1,650,538 and 461,590 bytes at brotli 11), the memo that says whether a card has any and the row that waits for them (release builds on 21 October 2026).
- **Images and videos in the composer** added 23.6 KB to the module and 4.4 KB at brotli 11 (1,726,484 → 1,750,034 bytes, and 483,170 → 487,527 at brotli 11, release builds on 21 October 2026): the draft's files and how their uploads go, a paste's or a drop's files handed to the composer's part, a scheduled draft's files, and the reader's own cast showing a video. The rest is in the composer's part, which grew from 133,752 to 212,651 bytes, and from 49,913 to 72,301 at brotli 11: the attach button and the tiles, uploading with progress, and taking a picture's metadata out (`allthing_core::metadata`). So a reader who doesn't write fetches none of it, and one who does fetches it as Cast, Reply or Quote is pointed at, about 110 ms more on a 1.6 Mbps "slow 4G" link; /scheduled's list grew 1.8 KB at brotli 11, for its files' thumbnails. Taking the metadata out reads the picture into the WebAssembly memory, which holds it twice (as read, and without its metadata) until it's handed to the upload: up to about 20 MB for a 10 MB photo, which the memory keeps for the visit. Videos and GIFs aren't read. A page view makes no more requests: the composer asks `/api/v1/media` as it opens, for a reader signed in, and keeps the answer 5 minutes.

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
