<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Budget: running on the Cloudflare Workers Free plan

Allthing runs as one Worker on the Workers **Free** plan. This page explains how it fits within the plan's limits, what each page view costs, how much load reaches Farcaster's servers, and what happens at each limit.

The relevant Free plan limits, as of October 2026:

| Limit | Value | Allthing's design |
|---|---|---|
| Worker requests | 100,000/day (resets 00:00 UTC) | Only `/api/*` and `/version` run the Worker; everything else is a static asset |
| Static asset requests | free and unlimited | The app, styles, fonts and label shards are all static |
| CPU time | 10 ms per invocation | Payloads are parsed once per refresh and cached; cache hits copy bytes |
| Subrequests | 50 per invocation (`fetch` **and** Cache API calls) | Hard budget of 45 per invocation, enforced in code |
| Cache API | per data center; no-op on `*.workers.dev` | Effective on `allthing.social` only |

## 1. What a page view costs

The browser app is static. A page view runs the Worker for its one API call and nothing else:

| Page | Worker requests | Free static requests |
|---|---|---|
| Front page `/` | 1 (`/api/v1/home`) | index.html, boot.js, app JS + WASM, CSS, font (first visit only: hashed, immutable) |
| Channel `/c/<id>` | 1 (`/api/v1/channel`) | none after the first visit |
| Cast `/p/<hash>` | 1 (`/api/v1/cast`) | none |
| Profile `/u/<name>` | 3 (`/api/v1/user`, `/api/v1/wallets`, `/api/v1/vote`) | 1 label shard and `meta.json`, about 32 KB raw |
| Channel directory | 1 (`/api/v1/channels`) | none |
| Search `/search?q=` | 1 (`/api/v1/search`) | none |
| Hovering likes, recasts or replies | 1 per popover (`/api/v1/reactions`, `/api/v1/replies`), after a 280 ms delay, then cached | none |
| Typing in the search box | 1 per distinct query, after typing pauses for 250 ms (`/api/v1/search`, the same payload the results page uses, so pressing Enter then costs nothing) | none |
| Opening a post already prefetched | **0** | none: client cache |
| Changing sort | **0** between hot and top: the payload holds the whole pool and the browser re-ranks it locally; **1** for "new" (`/api/v1/latest`, reused for 30 s) | none |
| Loading more front-page casts | 1 per upstream batch of up to 100 (`/api/v1/home?offset=`) | none |
| Changing the time range | 1 per trending window not loaded yet (`/api/v1/home?window=1h` or `7d`); the past month, year and all time share the week's payload | none |
| Back/forward within 5 min | **0** | none: in-memory client cache |
| Revisit within `max-age` | **0** | none: browser HTTP cache (30 s to 1 h per endpoint) |

Images and videos never touch the Worker. They load straight from `imagedelivery.net` and Cloudflare Stream; HLS segments are fetched by the browser, never proxied.

The front page fills the space near the end of the list even when filtering leaves a batch short or empty. It can request offsets 100 through 900, at most nine additional payloads (1,000 source casts including the first page). Each payload remains a separate cached, guarded invocation with the same 45-subrequest budget. Failed pages keep their 2, 4 and 8 second retry delays. Deep scrolling or short filtered batches can therefore cost up to ten home requests per visit; the typical capacity estimate below does not describe that worst case.

**Prefetching** ([PERFORMANCE.md](PERFORMANCE.md)) spends Worker requests ahead of clicks, within caps:
- hover or touch on a card: one `/api/v1/cast` request, deduplicated;
- cards that stay in view: at most 6 a minute;
- never more than 2 in flight, and none on Save-Data or 2g.

A reader who opens posts they hovered spends nothing extra; idle viewport prefetches cost at most 6 requests a minute per active reader.

**Capacity:** about **60,000 page views a day** with a typical mix, including prefetches and popovers (about 1.6 Worker requests per view). That is before counting views that cost nothing (sorting, back/forward, revisits).

## 2. Edge cache: one upstream fetch serves everyone nearby

Every API payload is cached at the edge with these policies (`crates/allthing-core/src/api.rs`):

| Endpoint | Fresh | Stale-while-revalidate until | Kept for stale-if-error | Browser `max-age` |
|---|---|---|---|---|
| home | 60 s | 10 min | 24 h | 30 s |
| latest ("new") | 30 s | 60 s | 24 h | 15 s |
| channel | 2 min | 15 min | 24 h | 60 s |
| cast | 2 min | 30 min | 24 h | 60 s |
| user | 5 min | 30 min | 24 h | 2 min |
| user, when the first-username read failed | 30 s | 5 min | 24 h | 15 s |
| wallets | 6 h | 24 h | 48 h | 1 h |
| channels | 1 h | 24 h | 48 h | 10 min |
| search | 5 min | 30 min | 24 h | 2 min |
| reactions (who liked, recasted) | 5 min | 1 h | 24 h | 5 min |
| replies (popover preview) | 2 min | 30 min | 24 h | 60 s |
| vote | 1 h | 24 h | 48 h | 10 min |
| not found (404) | 5 min | — | 5 min | 2 min |

- **Fresh**: served from cache as is (1 subrequest, the cache read).
- **Stale**: served immediately, and **one** background refresh starts through `waitUntil`. Each isolate refreshes a key at most once at a time.
- **Expired**: rebuilt while the request waits. If the rebuild fails, the last good copy is served with `x-allthing-stale: 1` and the page says so.
- **Concurrent misses** for the same key in an isolate share one build (request coalescing).
- Bad links (unknown user, missing cast) are cached as 404s for 5 minutes, so they don't reach upstream repeatedly.

## 3. Subrequests per invocation

Each invocation gets a budget of 45 subrequests, under the cap of 50. Every `fetch` and every Cache API call takes from it, and work that would exceed it is skipped rather than failing. In practice that means origins are left "pending" for a later refresh. Upper bounds per build:

| Build | Calls |
|---|---|
| Any cache hit | 1 (cache read) |
| home | cache read 1 + trending casts 1 + link pages ≤12 + quoted casts ≤1 + origins ≤36 (12 authors × at most 3) + app names ≤1 + cache write 1 |
| latest | cache read 1 + events 2 (one page per shard) + casts bulk 1 + link pages ≤12 + quoted casts ≤1 + origins ≤36 + app names ≤1 + cache write 1 |
| status | cache reads 2 (the latest and home payloads, for their freshness); no upstream calls |
| channel | cache read 1 + channel lookup, feed and farcaster.xyz channel record 3 (in parallel) + feed refetch ≤1, lead and moderators (`user/bulk`) ≤1 and pinned cast ≤1 (in parallel) + quoted casts ≤1 + link pages ≤12 + origins + app names ≤1 + cache write 1 (21 before origins, which take what is left of the 45) |
| cast | cache read 1 + conversation 1 + link pages ≤12 + channel ≤1 + parent ≤1 + quotes ≤3 + origins (budget-limited) + apps ≤1 + cache write 1 |
| user | cache read 1 + lookup 1–3 (by fname; else the name's username proof, then the account) + casts 1 + signers 1 + cast signers 1 + removed-key history ≤1 + first username ≤1 (accounts registered before Nov 2023, alongside the casts) + link pages ≤12 + quoted casts ≤1 + apps ≤1 + cache write 1 (≤ 25) |
| wallets | cache read 1 + user 1 + 9 EVM chains, one batched JSON-RPC call each (+ up to 5 fallback endpoints) + cache write 1 (≤ 17) |
| channels | 3 |
| search | cache read 1 + casts, users, channels 3 (in parallel) + bulk cast hydrate 1 + link pages ≤12 + quoted casts ≤1 + cache write 1 (≤ 19) |
| reactions | cache read 1 + reactions 1 + cache write 1 (≤ 3) |
| replies | cache read 1 + conversation (depth 1) 1 + link pages ≤12 + cache write 1 (≤ 15) |
| vote | cache read 1 + one batched Base `eth_call` pair 1 (+ 1 fallback RPC) + farcaster.xyz `user-by-fid` 1 + cache write 1 (≤ 5) |
| `/version` | 0 |

**Origins** (which app posted a cast) cost two upstream calls per author: the author's signers, and the cast's signer (or one page of the author's recent casts). They are resolved progressively. Each build resolves up to 12 new authors, carries already-resolved origins forward from the previous payload, and keeps a bounded per-isolate memory of 20,000 resolved casts, which never change. A busy front page is fully attributed within a few refreshes.

**Link pages** are read to find mini apps hosted on their own domains, which Hypersnap gives no metadata for: the page's `<head>` is checked for `fc:miniapp`, `fc:frame`, `fc:frame:*` or `of:*` meta tags (`crates/allthing-data/src/miniapps.rs`). Each build reads at most 12 unknown links, in batches of six, and never takes the last 10 subrequests, so the rest of the build keeps its share. Pages are read with a 2.5 s timeout and at most 128 KB, as a `Range` request and a capped stream. Verdicts are kept per URL for 24 h in each isolate and carried in the payload, so a link is read once, not on every refresh. Links that couldn't be read are tried again an hour later. Sites that never serve mini apps (YouTube, X, GitHub, Farcaster's own hosts and a few others) and media links are not read. A cast whose link is still unknown stays listed until a later refresh reads it.

**Quoted casts** come in the feed payloads, so cards show each quote at once instead of a link. The home, channel, profile and search builds read the casts their casts quote in one bulk `casts?casts=…` request of up to 100 hashes (`quoted_casts` in `crates/allthing-data/src/pages.rs`), alongside origins, and never take the last 2 subrequests for it, so app names and the cache write keep theirs. Quotes already in the previous payload are carried forward while still quoted, so a refresh reads only newly quoted casts, and makes no request when there are none. A read that fails or doesn't fit the budget leaves those quotes as links.

## 4. CPU time

Parsing is the main CPU cost, and it happens only when a payload is (re)built. Measured natively at `opt-level = "z"`, the full parse, normalize and serialize step takes:

- trending feed, 100 casts (206 KB of JSON): **0.74 ms**
- a user's casts, 100 casts (178 KB): **0.68 ms**
- a channel feed, 200 casts (382 KB): **1.46 ms**. Channel feeds come back at twice the requested limit, so Allthing requests 50.

A feed build also parses its quoted casts: one bulk read, usually a handful of casts and at most 100, and only those not carried from the previous payload.

WebAssembly in V8 runs these about 1.5 to 2.5 times slower, so a full rebuild stays around **2 to 5 ms**, well under 10 ms. A cache hit only copies bytes. Upstream responses are parsed into structs that declare only the fields Allthing reads. Solana-style large responses are avoided entirely (see below).

## 5. Load on upstreams

Allthing's goal is that the number of viewers barely changes upstream volume.

- **Per data center, per payload**: at most one refresh per freshness window while traffic lasts. For the front page that is at most 1,440 builds a day per active data center. A steady-state rebuild costs about 2 to 7 Hypersnap calls once origins are memoized, one of them for newly quoted casts when there are any.
- **Per isolate token buckets** (`crates/allthing-data/src/upstream.rs`) cap bursts:

  | Upstream | Burst | Sustained |
  |---|---|---|
  | Hypersnap | 40 | 10 requests/s |
  | Base RPC (vote) | 10 | 2 requests/s |
  | Each chain RPC (wallets) | 4 | 1 request every 2 s |
  | farcaster.xyz public API (votes, channel records) | 6 | 1 request/s |
  | Link pages (mini app meta tags), all sites together | 12 | 2 requests/s |

  A request finding an empty bucket is not sent. The payload is served from cache, or the API answers 503 with `Retry-After`.
- **Circuit breakers** (one per upstream, per isolate, except link pages, which are many unrelated sites): three consecutive failures (timeouts, 5xx, 429) open the breaker for 5 s, doubling up to 5 minutes. An upstream's `Retry-After` is honored, up to the same cap. While open, nothing is sent and cached copies are served.
- **Timeouts**: 8 s for Hypersnap, 5 s for RPCs and 2.5 s for link pages, via `AbortController`.
- **Memoized per isolate**: signer maps (1 h), app accounts (24 h), trending channels for the directory (10 min), resolved origins (forever, bounded), link page verdicts (24 h, bounded).
- **Static, built at deploy time**: the Farcaster app's 125 MB spam-label dataset becomes 35 shards of 32 KB, one character per fid (about 1.1 MB in all). Profiles read one shard as a free static asset, with zero Worker or upstream requests. Deploys reuse the live shards unless the upstream dataset changed (`scripts/build.sh`).
- **Solana activity is not measured**. Its public RPC refuses requests from Cloudflare Workers and from browsers (HTTP 403), and the keyless alternatives don't serve address history. Allthing leaves it out of the activity chart instead of retrying.

## 6. Abuse and the daily request limit

- **Per-client limit**: the `API_LIMITER` rate-limiting binding allows 120 API calls a minute per IP per Cloudflare location, and answers 429 with `Retry-After: 60` beyond that. It is a backstop; normal browsing uses a few calls a minute.
- **Read-only**: anything but GET or HEAD on `/api/*` gets 405. The API sends no CORS headers, so other sites' scripts cannot read its answers, which leaves little reason to call it from elsewhere.
- **The browser app** never polls; it makes one API call per page, caches answers for 5 minutes, prefetches within the caps above, and shows a "busy, try again" panel on 429 or 503 instead of retrying in a loop.
- **When 100,000 requests/day is reached**: requests that would run the Worker (`/api/*`) get 429 until 00:00 UTC. Static assets keep being served free, so the site still loads and shows the error panel. Nothing is billed: the Free plan has no overage.
- **Optional, set up in the dashboard**: a WAF rate-limiting rule on `/api/` and Bot Fight Mode stop abusive traffic *before* it reaches the Worker, so it doesn't count against the daily allowance.

## 7. The static copy

The GitHub Pages copy (see [ARCHITECTURE.md](ARCHITECTURE.md#the-static-copy-github-pages)) uses no Worker and no Cloudflare quota. Each visitor's browser runs the builders with the same guards and a budget of 30 upstream calls per page. Without a shared cache, its load on Hypersnap grows with its readers, which is one reason `allthing.social` is the primary site. The Farcaster app's API sends no CORS headers, so the browser can't read it: there, channel pages show only what Hypersnap gives (the name, the casts and how many accounts have cast), and the failed read costs one call of the 30 until the breaker opens.

Linked HTML uses the same 128 KB body limit in the browser: chunks are copied only up to the limit, and the remaining stream is cancelled, including on a timeout. Ordinary JSON responses remain uncapped. This bounds the retained HTML bytes and avoids waiting for a large or stalled tail; bytes a server already delivered in a chunk can still exceed the retained limit.

## 8. Watching it

- `GET /api/v1/status` reports, for the isolate that answers, its request count and each upstream's requests, failures and breaker state. It makes no upstream calls and touches no cache.
- Workers Logs are enabled with 5% head sampling (`wrangler.toml`), to stay within the free log allowance.
- Every API response carries `x-allthing-cache` (`fresh`, `revalidating`, `miss` or `stale-if-error`) and `x-allthing-age`.
