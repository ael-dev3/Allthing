<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Architecture

Allthing is a read-only Farcaster client. It is one Cloudflare Worker that serves two things: a static WebAssembly app, and a small JSON API that the app calls once per page. All of it is Rust, except generated glue and a short boot script.

```text
 Browser                                   Cloudflare (one Worker, Free plan)            Upstreams
 ───────                                   ──────────────────────────────────            ─────────
 index.html, boot.js        ── static ──▶  Workers Static Assets (free, no Worker run)
 allthing-*.wasm/.js/.css                  dist/: hashed, cached for a year
 /data/labels/spam/NNNN.txt                dist/data: spam-label shards, cached a day

 one call per page          ── /api/* ──▶  allthing-worker (Rust, workers-rs)
 GET /api/v1/home …                          rate limit (per IP) → edge cache (Cache API)
                                             → coalesced build → guarded upstream calls  ──▶  Hypersnap /v2 + /v1
                                                                                         ──▶  Base RPC (Neynar scores)
                                                                                         ──▶  EVM RPCs (wallet activity)

 images, video, posters     ── direct ──▶  imagedelivery.net, Cloudflare Stream, …  (never proxied)
```

## Crates

| Crate | Target | Role |
|---|---|---|
| `allthing-core` | native + wasm32 | Everything that can be pure: the data model, normalizing Hypersnap's JSON, ranking, cast text tokenizing, embed classification and image sizes, HLS playlist parsing, provenance decoding, wallet shares, spam-label shards, upstream guards (token bucket, breaker, budget), the API payload types and cache policies. Only `serde` and `serde_json`; no I/O. Most unit tests live here and run against real Hypersnap responses in `tests/fixtures`. |
| `allthing-data` | native + wasm32 | The page builders that assemble each payload, and the Hypersnap and RPC clients behind guards. The runtime is abstracted as a `Host` (clock, HTTP with a timeout, background work), so the same code runs in the Worker and in the browser. |
| `allthing-worker` | wasm32 (Worker) | The `/api/v1` handler: routing, the per-IP rate limit, the edge cache with coalescing, and a `Host` over Workers `fetch`, `AbortController` and `waitUntil`. |
| `allthing-web` | wasm32 (browser) | The Leptos app (client-side rendering): routes, pages, components, the media viewer, the HLS player, keyboard navigation. With the `static-host` feature it builds payloads in-process through a browser `Host`. |
| `xtask` | native | `cargo xtask dist` builds `dist/`; `cargo xtask pages` builds the static copy in `dist-pages/`; `cargo xtask labels` turns the spam-label dataset into shards. |

`scripts/build.sh` runs `worker-build` for the Worker and `cargo xtask dist` for the app. The xtask runs `cargo build`, `wasm-bindgen --target web` and `wasm-opt -Oz`, then writes fingerprinted files (`allthing-<hash>.wasm/.js/.css`, `boot-<hash>.js`) and fills them into `web/index.html`. Both WebAssembly builds use `opt-level = "z"`, LTO, one codegen unit and `panic = "abort"`.

## The API

All endpoints are `GET` (`HEAD` also works). Any other method gets 405, and the API sends no CORS headers.

| Endpoint | Payload | Built from |
|---|---|---|
| `/api/v1/home` | `HomePayload`: a pool of about 100 trending casts, trending channels, annotations | `feed/trending`, `channel/trending` |
| `/api/v1/channel?id=<id or parent URL>` | `ChannelPayload`: the channel and a pool of its casts | `channel`, `feed/parent_urls` (every spelling of the channel's parent URL) |
| `/api/v1/cast?hash=<hash>` | `CastPayload`: the thread (5 levels), the parent, quoted casts, the channel | `cast/conversation`, `cast` |
| `/api/v1/user?name=<username or fid:N>` | `UserPayload`: the profile and a pool of recent casts | `user/by_username` (with an exact-match search fallback for ENS-style names), `user/bulk`, `feed/user/casts` |
| `/api/v1/wallets?fid=<fid>` | `WalletActivity`: shares by chain, counts, sources | `user/bulk`, then one batched `eth_getTransactionCount` per chain |
| `/api/v1/channels?q=<query>` | `ChannelsPayload` | `channel/trending` or `channel/search` |
| `/api/v1/status` | isolate and upstream-guard state | nothing: no upstream calls, no cache |

Paths are relative to `https://haatz.quilibrium.com/v2/farcaster/` (Hypersnap's Neynar-compatible API). Payloads carry `Meta` (backend, fetch time, staleness) and `Annotations`, shared by every cast on the page:

- **scores**: Neynar user scores by fid, read onchain on Base. One `eth_call` to `getScores(uint256[])` on the scores contract covers up to 150 fids; scores are memoized for 6 h per isolate.
- **origins**: for each cast, its signer key and the fid of the app that registered that key. The app comes from the signer's `SignedKeyRequest` metadata: `requestFid` from Hypersnap's `/v1/signersByFid`, or decoded from the key's onchain event (`/v1/onChainEventsByFid`) when the key has since been removed. Signers are matched to casts with `/v1/castById` (one cast) or `/v1/castsByFid` (several).
- **apps**: the names and pictures of those app fids.
- **pending**: how many casts are still unattributed. Origins resolve progressively, up to 12 new authors per build, and each payload carries the previous one's origins forward.

## Edge cache, coalescing and guards

Payloads are cached with the Cache API under `https://<host>/__cache/api/v1/...` keys. Each endpoint has its own policy (`allthing_core::api`):

- **fresh**: served as is;
- **stale**: served at once while one background rebuild runs under `waitUntil`;
- **expired**: rebuilt while the request waits; if that fails, the last good copy is served (stale-if-error).

Unknown users and casts are cached as 404s for 5 minutes.

**Coalescing**: one build per key per isolate. The request that misses starts the build, and the build also runs under `waitUntil`, so it is cached even if that client disconnects. Requests for the same key join it. They wait on their **own** timers and read the result from a shared slot, instead of awaiting the first request's I/O: the runtime would treat that as a hang, and it could outlive the first request. A build older than 30 s is presumed dead and no longer blocks its key.

**Guards** (`allthing_core::guard`, used in `upstream.rs`):

- per-isolate token buckets for each upstream;
- a circuit breaker per upstream: three consecutive failures open it for 5 s, doubling up to 5 min, and `Retry-After` is honored;
- 8 s and 5 s timeouts through `AbortController`;
- a budget of 45 subrequests per invocation (the limit is 50), which counts cache reads and writes too. Work beyond the budget is skipped, not failed.

Numbers and the request budget are in [BUDGET.md](BUDGET.md).

## Data sources

| Data | Source | Notes |
|---|---|---|
| Casts, threads, channels, users | Hypersnap (`haatz.quilibrium.com`): Neynar-shaped `/v2/farcaster/*` plus hub `/v1/*` | A read node mirroring Snapchain mainnet. Shown as the "Hypersnap" backend on each cast. |
| Neynar user score | Neynar's score contract on Base, via public RPCs (`mainnet.base.org`, publicnode) | Shown beside names when above zero, with tiers at 0.5 and 0.9 |
| Signing app | Signer events and `SignedKeyRequest` metadata through Hypersnap `/v1` | "via Warpcast" and the like |
| Spam labels | [warpcast/labels](https://github.com/warpcast/labels) `spam.jsonl` (Git LFS) | Compacted at deploy time into 32 KB shards, one character per fid, read by the browser as static files. Shown on profiles; never used to hide anything. |
| Wallet activity | Account nonces of verified EVM addresses (up to 5) on Base, Ethereum, Optimism, Arbitrum, Polygon, Zora, Celo, Unichain and Monad, from keyless public RPCs | A nonce counts transactions *sent* from an address. Shares use largest-remainder rounding. Solana is not measured: its public RPC refuses Workers and browsers. |
| Video | Cloudflare Stream behind `stream.farcaster.xyz` (and `stream.warpcast.com`), fetched by the browser | Posters are Stream thumbnails next to the redirected manifest. |

Provider quirks stay in `hypersnap.rs` and `normalize.rs`. Examples: channel feeds return twice the requested limit, user lookups by ENS-style names need a search fallback, and errors come back as HTTP 400. The rest of the code sees the `allthing_core::model` types only.

## The browser app

- **Boot.** `index.html` shows a static header and a soft glow while `boot.js` imports the wasm-bindgen glue and instantiates the module. The app then mounts into `#app` and removes the placeholder. `www.allthing.social` is sent to the apex before anything loads.
- **Routing.** Routes: `/`, `/c/<id>` (or `/c?id=`), `/p/<hash>` (or `/p?hash=`), `/u/<name>` (or `/u?name=`), `/channels`, `/about`. The query forms keep links from the old static site working. Any other path renders "not found".
- **One request per page.** Each page reads one payload (`api.rs`, with a 60-second in-memory cache for back and forward). Sorting (hot, new, top) and paging (25 per page) re-rank the pool locally (`allthing_core::ranking`, `listing`). Profiles also read `/wallets` and one static label shard.
- **Media.** `media.rs` turns embeds into a gallery with generated alt text that names the author and quotes the cast. Hovering an image opens a preview after 140 ms, sized to fit. The lightbox is a modal dialog with a focus trap, Escape, arrow keys and pointer swipes, and it returns focus to the tile that opened it.
- **Video.** `player.rs` and `video.rs`: an IntersectionObserver loads the master playlist 600 px before a video scrolls into view. Safari plays the playlist natively. Elsewhere the Rust player parses master and media playlists (`allthing_core::hls`), picks a rendition by height (480p for hover previews, 720p inline, 1080p in the viewer), and feeds fMP4 segments into separate video and audio `SourceBuffer`s. It keeps 24 s buffered ahead, follows seeks, steps over gaps and ends the stream. Plain `.mp4` and `.webm` files play natively. YouTube, Vimeo, Loom and Streamable links render as facades that load the provider's iframe only on click; YouTube uses `youtube-nocookie.com`.
- **Threads.** `CommentTree` renders replies recursively. Collapsing animates the row height and marks the hidden subtree `inert`. `keys.rs` adds `j`/`k`/`o`/`x` over any element marked `data-nav`.
- **Design.** `style/allthing.css` is a light theme: warm paper, an indigo accent, cyan and gold for the "portal" details, and Instrument Serif for display type. It adds a faint lattice and glows behind the page and a slowly drifting line under the header. Cards rise in and lift on hover, threads expand, and skeletons shimmer. `prefers-reduced-motion` turns the motion off.

## The static copy (GitHub Pages)

A static host can't run the Worker, so `cargo xtask pages` builds a second variant of the app:

- compiled with the `static-host` feature, `api.rs` answers `/api/v1/...` paths in-process: `allthing_data::Endpoint::from_path` and `build` run with a browser `Host` (`window.fetch`, `AbortController`, `setTimeout`) and a budget of 30 upstream calls per page;
- compiled with `ALLTHING_BASE_PATH=/Allthing`, every link, asset, label shard and the router live under that prefix;
- `404.html` is a copy of `index.html`, which is how GitHub Pages serves deep links to a single-page app.

Annotations memoized in the page (scores, origins, apps) carry over between pages during a visit. There is no shared cache, so each visitor's browser makes its own upstream reads, and the module is about 30% larger because it carries the builders. The Cloudflare deployment is the primary one. This copy keeps the project usable from any static host.

## Security and privacy

- **Read-only**: there are no accounts, cookies, keys, signers or write paths. The Worker accepts only `GET` and `HEAD`.
- **No secrets**: every upstream is public and keyless. The Workers Builds token lives in Cloudflare, never in the repository.
- **Content Security Policy** (`web/_headers`): scripts are self-hosted, with `'wasm-unsafe-eval'` for WebAssembly only. Frames are limited to the four video providers. `connect-src` allows any https origin, because HLS playlists are fetched from wherever a video is hosted. There is no `unsafe-eval`, no inline scripts and no framing of the site.
- **Links** in casts get `rel="noopener noreferrer nofollow ugc"`. Cast text is tokenized and rendered as text nodes, never as HTML.
- **Abuse**: a per-IP rate limit (`API_LIMITER`, 120 a minute), the caching above, and guards that refuse to send rather than queue. See [BUDGET.md](BUDGET.md).

## Testing

- **Unit tests**: `cargo test` covers the core against recorded Hypersnap responses, the Worker's ABI decoding of onchain scores, link shortening in the app, and the xtask's CSS minifier and fingerprinting.
- **Lints**: `scripts/check.sh` runs rustfmt and Clippy with `-D warnings` for the native and wasm32 targets, and the `cargo deny` license audit.
- **End to end**: `e2e/allthing.spec.ts` runs Playwright against `wrangler dev`, or against production with `E2E_BASE_URL`, using live data. Video playback runs against a small VP9/Opus HLS fixture (`e2e/fixtures/hls`), because Playwright's Chromium cannot decode H.264.
