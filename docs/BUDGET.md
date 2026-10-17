<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Budget: running on the Cloudflare Workers Free plan

Allthing runs as one Worker on the Workers **Free** plan. This page explains how it fits within the plan's limits, what each page view costs, how much load reaches Farcaster's servers, and what happens at each limit.

The relevant Free plan limits, as of October 2026:

| Limit | Value | Allthing's design |
|---|---|---|
| Worker requests | 100,000/day (resets 00:00 UTC) | Only `/api/*` runs the Worker; everything else is a static asset |
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
| Profile `/u/<name>` | 2 (`/api/v1/user`, `/api/v1/wallets`) | 1 label shard and `meta.json`, about 32 KB raw |
| Channel directory | 1 (`/api/v1/channels`) | none |
| Changing sort or page | **0** | none: the payload holds the whole pool and the browser re-ranks it locally |
| Back/forward within 60 s | **0** | none: in-memory client cache |
| Revisit within `max-age` | **0** | none: browser HTTP cache (30 s to 1 h per endpoint) |

Images and videos never touch the Worker. They load straight from `imagedelivery.net` and Cloudflare Stream; HLS segments are fetched by the browser, never proxied.

**Capacity:** about **80,000 page views a day** with a typical mix (about 1.2 Worker requests per view), before counting views that cost nothing (sorting, paging, back/forward, revisits).

## 2. Edge cache: one upstream fetch serves everyone nearby

Every API payload is cached at the edge with these policies (`crates/allthing-core/src/api.rs`):

| Endpoint | Fresh | Stale-while-revalidate until | Kept for stale-if-error | Browser `max-age` |
|---|---|---|---|---|
| home | 60 s | 10 min | 24 h | 30 s |
| channel | 2 min | 15 min | 24 h | 60 s |
| cast | 2 min | 30 min | 24 h | 60 s |
| user | 5 min | 30 min | 24 h | 2 min |
| wallets | 6 h | 24 h | 48 h | 1 h |
| channels | 1 h | 24 h | 48 h | 10 min |
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
| home | cache read 1 + trending casts 1 + trending channels ≤1 (memoized 10 min per isolate) + Neynar scores ≤2 + origins ≤36 (12 authors × at most 3) + app names ≤1 + cache write 1 |
| channel | as home, plus channel lookup 1 and a possible feed refetch 1 |
| cast | cache read 1 + conversation 1 + channel ≤1 + parent ≤1 + quotes ≤3 + scores ≤2 + origins (budget-limited) + apps ≤1 + cache write 1 |
| user | cache read 1 + lookup 1–2 + casts 1 + signers 1 + cast signers 1 + removed-key history ≤1 + scores ≤2 + apps ≤1 + cache write 1 (≤ 12) |
| wallets | cache read 1 + user 1 + 9 EVM chains, one batched JSON-RPC call each (+ up to 5 fallback endpoints) + cache write 1 (≤ 17) |
| channels | 3 |

**Origins** (which app posted a cast) cost two upstream calls per author: the author's signers, and the cast's signer (or one page of the author's recent casts). They are resolved progressively. Each build resolves up to 12 new authors, carries already-resolved origins forward from the previous payload, and keeps a bounded per-isolate memory of 20,000 resolved casts, which never change. A busy front page is fully attributed within a few refreshes.

## 4. CPU time

Parsing is the main CPU cost, and it happens only when a payload is (re)built. Measured natively at `opt-level = "z"`, the full parse, normalize and serialize step takes:

- trending feed, 100 casts (206 KB of JSON): **0.74 ms**
- a user's casts, 100 casts (178 KB): **0.68 ms**
- a channel feed, 200 casts (382 KB): **1.46 ms**. Channel feeds come back at twice the requested limit, so Allthing requests 50.

WebAssembly in V8 runs these about 1.5 to 2.5 times slower, so a full rebuild stays around **2 to 5 ms**, well under 10 ms. A cache hit only copies bytes. Upstream responses are parsed into structs that declare only the fields Allthing reads. Solana-style large responses are avoided entirely (see below).

## 5. Load on upstreams

Allthing's goal is that the number of viewers barely changes upstream volume.

- **Per data center, per payload**: at most one refresh per freshness window while traffic lasts. For the front page that is at most 1,440 builds a day per active data center. A steady-state rebuild costs about 2 to 6 Hypersnap calls once origins and scores are memoized.
- **Per isolate token buckets** (`crates/allthing-data/src/upstream.rs`) cap bursts:

  | Upstream | Burst | Sustained |
  |---|---|---|
  | Hypersnap | 40 | 10 requests/s |
  | Base RPC (scores) | 10 | 2 requests/s |
  | Each chain RPC (wallets) | 4 | 1 request every 2 s |

  A request finding an empty bucket is not sent. The payload is served from cache, or the API answers 503 with `Retry-After`.
- **Circuit breakers** (one per upstream, per isolate): three consecutive failures (timeouts, 5xx, 429) open the breaker for 5 s, doubling up to 5 minutes. An upstream's `Retry-After` is honored, up to the same cap. While open, nothing is sent and cached copies are served.
- **Timeouts**: 8 s for Hypersnap and 5 s for RPCs, via `AbortController`.
- **Memoized per isolate**: Neynar scores (6 h, batched up to 150 fids per `eth_call`), signer maps (1 h), app accounts (24 h), trending channels (10 min), resolved origins (forever, bounded).
- **Static, built at deploy time**: the Farcaster app's 125 MB spam-label dataset becomes 35 shards of 32 KB, one character per fid (about 1.1 MB in all). Profiles read one shard as a free static asset, with zero Worker or upstream requests. Deploys reuse the live shards unless the upstream dataset changed (`scripts/build.sh`).
- **Solana activity is not measured**. Its public RPC refuses requests from Cloudflare Workers and from browsers (HTTP 403), and the keyless alternatives don't serve address history. The UI says so instead of retrying.

## 6. Abuse and the daily request limit

- **Per-client limit**: the `API_LIMITER` rate-limiting binding allows 120 API calls a minute per IP per Cloudflare location, and answers 429 with `Retry-After: 60` beyond that. It is a backstop; normal browsing uses a few calls a minute.
- **Read-only**: anything but GET or HEAD on `/api/*` gets 405. The API sends no CORS headers, so other sites' scripts cannot read its answers, which leaves little reason to call it from elsewhere.
- **The browser app** never polls; it makes one API call per page, caches answers for 60 seconds, and shows a "busy, try again" panel on 429 or 503 instead of retrying in a loop.
- **When 100,000 requests/day is reached**: requests that would run the Worker (`/api/*`) get 429 until 00:00 UTC. Static assets keep being served free, so the site still loads and shows the error panel. Nothing is billed: the Free plan has no overage.
- **Optional, set up in the dashboard**: a WAF rate-limiting rule on `/api/` and Bot Fight Mode stop abusive traffic *before* it reaches the Worker, so it doesn't count against the daily allowance.

## 7. The static copy

The GitHub Pages copy (see [ARCHITECTURE.md](ARCHITECTURE.md#the-static-copy-github-pages)) uses no Worker and no Cloudflare quota. Each visitor's browser runs the builders with the same guards and a budget of 30 upstream calls per page. Without a shared cache, its load on Hypersnap grows with its readers, which is one reason `allthing.social` is the primary site.

## 8. Watching it

- `GET /api/v1/status` reports, for the isolate that answers, its request count and each upstream's requests, failures and breaker state. It makes no upstream calls and touches no cache.
- Workers Logs are enabled with 5% head sampling (`wrangler.toml`), to stay within the free log allowance.
- Every API response carries `x-allthing-cache` (`fresh`, `revalidating`, `miss` or `stale-if-error`) and `x-allthing-age`.
