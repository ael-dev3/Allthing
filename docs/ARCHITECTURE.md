<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Architecture

Allthing is a read-only Farcaster client. It is one Cloudflare Worker that serves two things: a static WebAssembly app, and a small JSON API that the app calls once per page. All of it is Rust, except generated glue, a short boot script and a one-line inline script that starts the front page's data request.

```text
 Browser                                   Cloudflare (one Worker, Free plan)            Upstreams
 ───────                                   ──────────────────────────────────            ─────────
 index.html, boot.js        ── static ──▶  Workers Static Assets (free, no Worker run)
 /wasm/allthing-*.wasm      ── Worker ──▶  the module's brotli-11 copy from ASSETS (boot.js falls back to /assets/)
 allthing-*.wasm/.js/.css                  dist/: hashed, cached for a year

 one call per page          ── /api/* ──▶  allthing-worker (Rust, workers-rs)
 GET /api/v1/home …                          rate limit (per IP) → edge cache (Cache API)
                                             → coalesced build → guarded upstream calls  ──▶  Hypersnap /v2 + /v1
                                                                                         ──▶  Base RPC (NS vote)
                                                                                         ──▶  EVM RPCs (wallet activity)
                                                                                         ──▶  farcaster.xyz (votes, channel records)

 images, video, posters     ── direct ──▶  imagedelivery.net, Cloudflare Stream, …  (never proxied)

                                           cron, every minute: the indexer
                                             one page of casts             ──────────────────▶  Hypersnap /v2
                                             → D1 allthing-index (casts, rankings); the front page
                                               reads one ranking row for Past month, Past year and All time
```

## Crates

| Crate | Target | Role |
|---|---|---|
| `allthing-core` | native + wasm32 | Everything that can be pure: the data model, normalizing Hypersnap's JSON, ranking, cast text tokenizing, embed classification and image sizes, HLS playlist parsing, provenance decoding, wallet shares, the content spam filter, profile cards, upstream guards (token bucket, breaker, budget), the API payload types and cache policies. Only `serde` and `serde_json`; no I/O. Most unit tests live here and run against real Hypersnap responses in `tests/fixtures`. |
| `allthing-data` | native + wasm32 | The page builders that assemble each payload, and the Hypersnap and RPC clients behind guards. The runtime is abstracted as a `Host` (clock, HTTP with a timeout, background work), so the same code runs in the Worker and in the browser. |
| `allthing-worker` | wasm32 (Worker) | The `/api/v1` handler: routing, the per-IP rate limit, the edge cache with coalescing, and a `Host` over Workers `fetch`, `AbortController`, `waitUntil` and D1. Its cron handler runs the indexer. |
| `allthing-web` | wasm32 (browser) | The Leptos app (client-side rendering): routes, pages, components, the media viewer, the HLS player, prefetching and scroll restoration. With the `static-host` feature it builds payloads in-process through a browser `Host`. |
| `xtask` | native | `cargo xtask dist` builds `dist/`; `cargo xtask pages` builds the static copy in `dist-pages/`. |

`scripts/build.sh` runs `worker-build` for the Worker and `cargo xtask dist` for the app. The xtask runs `cargo build`, `wasm-bindgen --target web` and `wasm-opt -Oz`, then writes fingerprinted files (`allthing-<hash>.wasm/.js/.css`, `boot-<hash>.js`) and fills them into `web/index.html`. Both WebAssembly builds use `opt-level = "z"`, LTO, one codegen unit and `panic = "abort"`.

## The API

All endpoints are `GET` (`HEAD` also works). Any other method gets 405, and the API sends no CORS headers.

| Endpoint | Payload | Built from |
|---|---|---|
| `/api/v1/home` | `HomePayload`: a pool of about 100 trending casts, annotations (with the casts they quote) | `feed/trending`, `casts` (quoted casts) |
| `/api/v1/channel?id=<id or parent URL>` | `ChannelPayload`: the channel (with its banner, picture, description, follower and member counts, lead, moderators, pinned cast and link where the Farcaster app has them) and a pool of its casts | `channel`, `feed/parent_urls` (every spelling of the channel's parent URL) and farcaster.xyz `v1/channel` in parallel, then `user/bulk` (lead, moderators, mentions) and `cast` (the pinned cast) |
| `/api/v1/cast?hash=<hash>` | `CastPayload`: the thread (5 levels), the parent, quoted casts, the channel | `cast/conversation`, `cast` |
| `/api/v1/user?name=<username or fid:N>` | `UserPayload`: the profile and a pool of recent casts, with the casts they quote | `user/by_username` (it knows current fnames only; any other name goes through the hub's `userNameProofByName`, then `user/bulk` for the FID it names), `user/bulk`, `feed/user/casts`, and for an account registered before the ID registry moved to OP Mainnet (Nov 2023), the fname server's `transfers?fid=` for when it got its first username, `casts` (quoted casts) |
| `/api/v1/wallets?fid=<fid>` | `WalletActivity`: shares by chain, counts, sources | `user/bulk`, then one batched `eth_getTransactionCount` per chain |
| `/api/v1/channels?q=<query>` | `ChannelsPayload` | `channel/trending` or `channel/search` |
| `/api/v1/home?offset=<100…900>` | the next page of the trending pool, for infinite scroll | `feed/trending` with an offset cursor |
| `/api/v1/home?window=1h\|7d` | the trending pool over the past hour or week, for the front page's time filter (`?t=` in the page URL; 24 hours without the parameter) | `feed/trending` with `time_window` |
| `/api/v1/home?window=30d\|365d\|all` | the top casts of the past month, the past year or all time, ranked from [Allthing's index](#allthings-index-past-month-past-year-all-time), with `index` saying how many casts it was ranked from and how far back they go. Until the index can rank the range, the past week's trending pool, without `index` | the span's ranking row in D1, then `casts?casts=…` for the page's 100 casts, fresh |
| `/api/v1/latest` | `LatestPayload`: the newest ~100 top-level casts, for the front page's "new", with each shard's event cursor | `/v1/events` on shards 1 and 2 (only the events since the previous build's cursors; the newest 1,000 per shard when the previous copy is over 10 minutes old), then `casts?casts=…` once for the new casts and the listed ones' counts, and once more for the casts they quote |
| `/api/v1/search?q=<query>` | `SearchPayload`: casts (with the casts they quote), people and channels | `cast/search` (then `casts?casts=…` to hydrate embeds and counts, and once more for quoted casts), `user/search`, `channel/search` |
| `/api/v1/reactions?hash=<hash>&type=likes\|recasts&limit=10\|100` | `ReactionsPayload`: who liked or recasted, newest first | `reactions/cast` |
| `/api/v1/engagement?likes=<hashes>&recasts=<hashes>` | `EngagementPayload`: who liked and recasted a batch of feed cards, loaded before anyone hovers a count. The hashes are comma-separated, 12 lists in all at most, and are normalized, deduplicated and sorted, so the same lists share one cache entry. Each of its `previews` is exactly what `/reactions?…&limit=10` answers for that cast and kind; a list that couldn't be read is left out, and its popover loads it on hover | `reactions/cast`, once per list, six at a time |
| `/api/v1/replies?hash=<hash>` | `RepliesPayload`: the ten most engaged direct replies | `cast/conversation` with `reply_depth=1` |
| `/api/v1/vote?fid=<fid>` | `VotePayload`: the account's NS vote ([VOTE.md](VOTE.md)) | Base `eth_call` to the opt-in NFT, farcaster.xyz `user-by-fid` |
| `/api/v1/status` | isolate and upstream-guard state, how fresh the cached "new" and trending lists are, and the index's rankings and progress | two cache reads and the indexer's state in D1; no upstream calls |
| `/version` (and `/api/v1/version`) | `{"name","version","commit"}` | nothing |

Paths are relative to `https://haatz.quilibrium.com/v2/farcaster/` (Hypersnap's Neynar-compatible API). Payloads carry `Meta` (backend, fetch time, staleness) and `Annotations`, shared by every cast on the page:

- **origins**: for each cast, its signer key and the fid of the app that registered that key. The app comes from the signer's `SignedKeyRequest` metadata: `requestFid` from Hypersnap's `/v1/signersByFid`, or decoded from the key's onchain event (`/v1/onChainEventsByFid`) when the key has since been removed. Signers are matched to casts with `/v1/castById` (one cast) or `/v1/castsByFid` (several).
- **apps**: the names and pictures of those app fids.
- **pending**: how many casts are still unattributed. Origins resolve progressively, up to 12 new authors per build, and each payload carries the previous one's origins forward.
- **quotes**: in feeds (home, channels, profiles, search), the casts that the page's casts quote, by hash, so each card shows its quote at once as a small card instead of a link. A build reads the ones it lacks in one bulk `casts?casts=…` request (up to 100), within the build's budget, and carries the previous payload's quotes forward while still quoted. A quoted cast that is spam or a mini app keeps only its author and time, and its card says it is hidden. Quotes that a failed read or the budget leaves out show as links.
- **users**: by fid, what the profile cards of the accounts the page names show beyond their picture and names (`allthing_core::profile::UserCard`): the start of the bio (160 characters, cut between words), follower and following counts, and the registration Hypersnap reports. They come from the user objects Hypersnap sends with each cast (and, on channel pages, with the hosts' `user/bulk` read), so they cost no request, and a build carries the previous payload's cards forward while their accounts are still shown (the author of a carried quote). A profile's own card is its `user`; a person found by search is a whole profile already.

## Edge cache, coalescing and guards

Payloads are cached with the Cache API under `https://<host>/__cache/api/v1/...` keys. Each endpoint has its own policy (`allthing_core::api`):

- **fresh**: served as is;
- **stale**: served at once while one background rebuild runs under `waitUntil`;
- **expired**: rebuilt while the request waits; if that fails, the last good copy is served (stale-if-error).

Unknown users and casts are cached as 404s for 5 minutes.

**Coalescing**: one build per key per isolate. The request that misses starts the build, and the build also runs under `waitUntil`, so it is cached even if that client disconnects. Requests for the same key join it. They wait on their **own** timers and read the result from a shared slot, instead of awaiting the first request's I/O: the runtime would treat that as a hang, and it could outlive the first request. A build older than 30 s is presumed dead and no longer blocks its key.

**Guards** (`allthing_core::guard`, used in `upstream.rs`):

- per-isolate token buckets for each upstream; preloaded popover previews (`/engagement`) never take Hypersnap's below 16 of its 40 tokens, so page builds keep a share;
- a circuit breaker per upstream: three consecutive failures open it for 5 s, doubling up to 5 min, and `Retry-After` is honored;
- 8 s and 5 s timeouts through `AbortController`;
- a budget of 45 subrequests per invocation (the limit is 50), which counts cache reads and writes too. Work beyond the budget is skipped, not failed.

Numbers and the request budget are in [BUDGET.md](BUDGET.md).

## Data sources

| Data | Source | Notes |
|---|---|---|
| Casts, threads, channels, users, search, reactions | Hypersnap (`haatz.quilibrium.com`): Neynar-shaped `/v2/farcaster/*` plus hub `/v1/*` | A read node mirroring Snapchain mainnet. |
| The NS vote | Network School's opt-in NFT on Base (stay), farcaster.xyz's public `user-by-fid` (leave) | [VOTE.md](VOTE.md) |
| Channel records: display name, description, picture, banner, follower and member counts, lead, moderators, pinned cast, link | The Farcaster app's public channel API (`api.farcaster.xyz/v1/channel`), one read per channel build | Best-effort: without it a channel page shows Hypersnap's data. It sends no CORS headers, so the static copy never has it. Hypersnap's `member_count` counts the accounts that have cast in a feed, so it is shown as "casters", never as members. |
| Signing app | Signer events and `SignedKeyRequest` metadata through Hypersnap `/v1` | "via Farcaster" and the like (the Farcaster app's account, fid 9152, is still named Warpcast; `allthing_core::clients::client_name` calls it Farcaster), and the front page's bar of the apps posting today's trending casts |
| When an account joined | Its onchain registration, as Hypersnap reports it; for accounts registered before the ID registry moved to OP Mainnet (7 Nov 2023), which all report that day, the fname server's public transfer log (`fnames.farcaster.xyz/transfers?fid=`) | Those show when they got their first username, which the old sign-up registered along with the account. It allows cross-origin reads, so the static copy shows it too. |
| Wallet activity | Account nonces of verified EVM addresses (up to 5) on Base, Ethereum, Optimism, Arbitrum, Polygon, Zora, Celo, Unichain and Monad, from keyless public RPCs; on Solana, the signatures of verified addresses (up to 2, 1,000 each) from its public RPC | A nonce counts transactions *sent* from an address; a Solana signature, any transaction involving it, shown as "1,000+" at the cap. Shares use largest-remainder rounding. Solana's RPC refuses browsers and, so far, Cloudflare Workers, so profiles usually say that Solana isn't counted. |
| Video | Cloudflare Stream behind `stream.farcaster.xyz` (and `stream.warpcast.com`), fetched by the browser | Posters are Stream thumbnails next to the redirected manifest. |

**Mini apps and frames are filtered out server-side.** Allthing doesn't run them, so a cast whose embeds or text link to a mini app or frame is dropped:
- **Detection by URL.** `normalize.rs` sets `Cast.mini_app` from URLs: `farcaster.xyz/miniapps/…`, `farcaster.xyz/~/mini-apps…`, `warpcast.com/~/frames…`, `*.neynar.app`. It also uses `fc:frame` or `fc:miniapp` metadata when an API provides it.
- **Detection by page.** Hypersnap returns no embed metadata, so mini apps on their own domains are found by reading the linked page's meta tags (`fc:miniapp`, `fc:frame`, `fc:frame:*`, `of:*`). The Worker reads a few unknown links per build, remembers each verdict per URL, and carries the verdicts in the payload (`Annotations.links`); `thread::mark_mini_apps` then flags the casts. Ordinary casts and quoted casts share the same six-link quota per invocation, one parallel batch. Quoted links are checked before stripping hidden quote content, and locally deferred checks remain eligible on the next build. See [BUDGET.md](BUDGET.md#3-subrequests-per-invocation).
- **Spam.** `spam.rs` flags casts by the owner's content rules: links or embeds on `*.vercel.app`, anything mentioning or linking gig-bot, DEGEN reward or airdrop bait ("Just unlocked my DEGEN rewards", `?degen=1` links), and DEGEN staking promotions (yeetr.fun links or names, "stake your DEGEN"). `normalize.rs` sets `Cast.spam`, and spam goes wherever mini apps go. The rules read only the cast's own text, links and mentions, never anything about the account that posted it.
- **Lists.** The builders drop hidden casts (`Cast::hidden`: mini apps and spam) from home, channels, profiles, search and reply previews (`thread::without_hidden`; a replies preview made from a post payload goes through the same `thread::reply_preview`). A quoted cast that is hidden, in a listing or on a post page, comes without its content (`quotes::as_quote`) and shows as "Quoted cast hidden" (spam: no label is shown) or "Quoted cast hidden: mini app". Like and recast previews, single or batched, list people, not casts.
- **Threads.** `thread::prune_hidden` drops a hidden reply. If it has replies of its own, it keeps the node with its content stripped, shown as "Mini app post hidden" or, for spam, "Post hidden". Allthing never labels a cast or an account as spam on screen; the content rules in `allthing-core/src/spam.rs` only decide what is left out.

Provider quirks stay in `hypersnap.rs` and `normalize.rs`. Examples: channel feeds return twice the requested limit, a channel's `member_count` is its number of distinct casters (`Channel.casters`), user lookups by ENS-style names need a search fallback, errors come back as HTTP 400, and reaction lists come shard by shard, each shard oldest first (`normalize::reactors` orders them by reaction time), one kind per call (`types=likes,recasts` answers with the likes alone). The Farcaster app's channel record is merged in by `normalize::enrich_channel`. The rest of the code sees the `allthing_core::model` types only.

## Allthing's index (Past month, Past year, All time)

Hypersnap ranks trending casts over 7 days at most, and its search ignores dates, so the longer ranges come from Allthing's own index of notable casts, in the D1 database `allthing-index` (the `INDEX` binding; schema in `migrations/`). The logic is in `allthing_core::index`, the run in `allthing_data::indexer`, and the SQL in `crates/allthing-worker/src/index.rs`.

- **The indexer** is the Worker's cron handler, run every minute. Each run reads one page of 100 casts: every fourth run, a page of the past week's trending casts (the ten pages in turn, so each is refreshed about every 40 minutes); the others, the next page back in time of one feed, in turn. Feeds start with seven big channels (`SEED_CHANNELS`), and every author of a cast with 250 points or more joins the queue (at most 120 feeds; feeds read to their start are remembered and not read again). It keeps the top-level casts that are not spam or mini apps and whose points could rank: at least 20, and once a ranking is full, at least its last cast's points. Only those are normalized, which keeps a run's CPU to about one page's parse.
- **Storage.** `casts` holds each kept cast's hash, time and points; storing it again updates its points, and writes nothing if they haven't changed. `rankings` holds each span's top 1,000 casts as one JSON row, recomputed inside D1 by one statement, so the Worker never reads more than a page of casts. A ranking is due again a minute per 50 indexed casts after it was computed, between 5 minutes and 6 hours. `state` holds where each feed is, and the day's D1 usage.
- **Limits.** The indexer estimates its D1 rows read and written and stops reading for the day at 50,000 rows written or 2,000,000 read, half of the free plan's 100,000 and 5,000,000 or less, leaving the rest to the front page. See [BUDGET.md](BUDGET.md#9-the-index-cron-and-d1).
- **Reading.** `/api/v1/home?window=30d|365d|all` reads the span's ranking row, then the page's 100 casts in one bulk `casts?casts=…` call, so counts, text and media are fresh; deleted casts drop out, and spam and mini apps are filtered again. The payload's `index` (`IndexCoverage`) says how many casts the span was ranked from and the oldest one's date, and the page shows it under the time filter. Until a span has a ranking (a new index, or D1 not answering), the payload is the past week's trending pool without `index`, and the page says so. The static copy has no index: its menu leaves these ranges out.
- **Coverage** grows as the feeds are read back: Past month and Past year fill within hours, and All time keeps deepening for days as the queued authors' histories are read. The ranking reflects what has been indexed, which the note under the filter states.

## The browser app

- **Boot.** `index.html` shows a static header and a soft glow while `boot.js` imports the wasm-bindgen glue and instantiates the module. The app then mounts into `#app` and removes the placeholder. `www.allthing.social` is sent to the apex before anything loads.
- **Routing.** Routes: `/`, `/c/<id>` (or `/c?id=`), `/p/<hash>` (or `/p?hash=`), `/u/<name>` (or `/u?name=`), `/channels`, `/search?q=`. The query forms keep links from the old static site working. Any other path renders "not found".
- **One request per page.** Each page reads one payload (`api.rs`, with a 5-minute in-memory cache). Sorting (hot, new, top) re-ranks the pool locally (`allthing_core::ranking`). Lists show 25 cards at a time and grow as the reader scrolls. Profiles also read `/wallets` and `/vote`, and so does a profile card when it opens.
- **Speed** ([PERFORMANCE.md](PERFORMANCE.md)):
  - posts open instantly from the feed's own data;
  - cards prefetch on hover and in idle time, within caps, and the cards in view have who liked and recasted them preloaded in batches;
  - `nav.rs` restores scroll on back and forward, and makes the whole card open the post; a quoted cast's card inside it opens the quoted cast.
- **Channel pages.** `pages/channel.rs` heads the feed with the channel's banner (a fixed 3:1 box, cover-cropped), its picture over the banner's edge, its name and `/id`, description, follower and member counts, link, lead and moderators (six, then "+N"), and puts the pinned cast above the feed. Each part shows only when the payload has it, so a channel known only to Hypersnap shows its name and how many accounts have cast there.
- **Popovers.** `popover.rs` is one accessible popover (hover or focus after 280 ms, tap on touch, Escape, outside click). `who.rs` uses it for who liked, recasted and replied. It is also behind the profile's "Wallets" button, which lists every verified address with copy-to-clipboard.
  - **Preloaded previews.** `preload.rs` loads who liked and recasted the feed cards in view before anyone hovers a count: the cards' shared prefetch observer reports them, and each card in view brings its run of six cards, in list order, as one `/api/v1/engagement` batch. `api::preload_engagement` files each list under its own `/reactions?…&limit=10` path in the client cache, dated as the batch. A preview reads that cache synchronously, so its people are there in the frame the popover opens, with no "Loading…" (a resource would resolve a tick later). Limits: [PERFORMANCE.md](PERFORMANCE.md#popover-previews).
  - **Replies previews** come from the post's own payload when hovering the card prefetched it (its thread holds the direct replies, ranked as `/replies` ranks them), and from `/api/v1/replies` otherwise. Putting replies in the batch would have cost a conversation read per cast (10 to 140 KB each, parsed and normalized as full casts) and the replies' link checks for mini apps: for six casts, up to 18 more subrequests and most of a build's 10 ms of CPU, for a popover the hover has usually paid for already.
- **Profile cards.** `profile_card.rs` puts the popover on a profile's own links (`links`): user chips in feeds, quotes and replies, the post's author, the full list of likes and recasts, people found by search and a channel's hosts.
  - Hovering a picture or name for 100 ms, or keyboard focus resting on it, opens the account's card. It stays open while the pointer is over it, closes on leaving, Escape or a click elsewhere, and Tab moves through its links and on, without trapping focus.
  - A tap on a touch screen (`pointerType` "touch") opens the card instead of the profile, and its "View profile" link opens the profile. A mouse click or the keyboard follows the link as before.
  - The bio, counts and join date come from the payload (`notes.users`), or from what this visit already showed of the account (`api::seen_card`; a profile visited knows when the account got its first username). The vote and wallet activity of feed cards' authors are prefetched as the cards come into view (`api::prefetch_card_facts`) and read synchronously (`api::ready`), so the card opens complete; otherwise they load when it opens, through the components the profile uses (`facts.rs`). A card never reads `/api/v1/user`.
  - Clicks in a card are its own (`nav::card_click` skips popovers), and text selected in it keeps it open. Cards in a dialog's list or a channel's header, which clip what overflows them, are placed against the viewport and close when anything scrolls.
- **Media.** `media.rs` turns embeds into a gallery with generated alt text that names the author and quotes the cast. The lightbox is a modal dialog with a focus trap, Escape, arrow keys and pointer swipes, and it returns focus to the tile that opened it.
- **Video.** `player.rs` and `video.rs`: an IntersectionObserver loads the master playlist 600 px before a video scrolls into view. Safari plays the playlist natively. Videos autoplay when at least 60% visible and pause when scrolled away. They autoplay muted unless the reader turned sound on: playing a video or unmuting one turns it on, and muting one turns it off. The choice is kept in localStorage (`allthing.sound`). Each player notes the muted state it set, so a `volumechange` it didn't cause is the reader's. Before the reader has interacted with the page, browsers refuse sound; the video then plays muted, and its sound comes on at the first tap or key press. Only one plays at a time, and none autoplay with reduced motion, Save-Data or a 2g connection. Elsewhere the Rust player parses master and media playlists (`allthing_core::hls`), picks a rendition by height (720p inline, 1080p in the viewer), and feeds fMP4 segments into separate video and audio `SourceBuffer`s. It keeps 24 s buffered ahead, follows seeks, steps over gaps and ends the stream. Plain `.mp4` and `.webm` files play natively. YouTube, Vimeo, Loom and Streamable links render as facades that load the provider's iframe only on click; YouTube uses `youtube-nocookie.com`.
- **Threads.** `CommentTree` renders replies recursively. Collapsing animates the row height and marks the hidden subtree `inert`.
- **Design.** `style/allthing.css` is a light theme: warm paper, an indigo accent, cyan and gold for the "portal" details, and Instrument Serif for display type. It adds a faint lattice and glows behind the page and a slowly drifting line under the header. Cards rise in and lift on hover, threads expand, and skeletons shimmer. `prefers-reduced-motion` turns the motion off.

## The static copy (GitHub Pages)

A static host can't run the Worker, so `cargo xtask pages` builds a second variant of the app:

- compiled with the `static-host` feature, `api.rs` answers `/api/v1/...` paths in-process: `allthing_data::Endpoint::from_path` and `build` run with a browser `Host` (`window.fetch`, `AbortController`, `setTimeout`) and a budget of 30 upstream calls per page;
- it preloads nothing from the viewport, popover previews included: they load when they open, as they did before the batches. `/api/v1/engagement` still builds in-process if asked for (its 12 reads fit the budget of 30), and a replies preview still comes from the post that hovering its card built;
- compiled with `ALLTHING_BASE_PATH=/Allthing`, every link, asset and the router live under that prefix;
- `404.html` is a copy of `index.html`, which is how GitHub Pages serves deep links to a single-page app.

Annotations memoized in the page (origins, apps) carry over between pages during a visit. There is no shared cache, so each visitor's browser makes its own upstream reads, and the module is about 30% larger because it carries the builders. The Farcaster app's API sends no CORS headers, so the browser can't read it: channel pages show Hypersnap's data alone, and votes recorded only by the app are unknown. The Cloudflare deployment is the primary one. This copy keeps the project usable from any static host.

## Security and privacy

- **Read-only**: there are no accounts, cookies, keys, signers or write paths. The Worker accepts only `GET` and `HEAD`.
- **No secrets**: every upstream is public and keyless. The Workers Builds token lives in Cloudflare, never in the repository.
- **Content Security Policy** (`web/_headers`): scripts are self-hosted, with `'wasm-unsafe-eval'` for WebAssembly only. The one exception is `static.cloudflareinsights.com`, for Cloudflare Web Analytics, which Cloudflare injects when it is switched on for the zone. It is cookieless, and turning it off in the dashboard removes the script. Its POST of page timings to `/cdn-cgi/rum`, handled by Cloudflare rather than the Worker, is the only non-GET request a page makes. Frames are limited to the four video providers. `connect-src` allows any https origin, because HLS playlists are fetched from wherever a video is hosted. There is no `unsafe-eval` and no framing of the site, and the only inline scripts are the early data request and the speculation rules in `web/index.html`, each allowed by its hash.
- **Links** in casts get `rel="noopener noreferrer nofollow ugc"`. Cast text is tokenized and rendered as text nodes, never as HTML.
- **Abuse**: a per-IP rate limit (`API_LIMITER`, 120 a minute), the caching above, and guards that refuse to send rather than queue. See [BUDGET.md](BUDGET.md).

## Testing

- **Unit tests**: `cargo test` covers the core against recorded Hypersnap responses, link shortening in the app, and the xtask's CSS minifier and fingerprinting.
- **Lints**: `scripts/check.sh` runs rustfmt and Clippy with `-D warnings` for the native and wasm32 targets, and the `cargo deny` license audit.
- **Tooling**: `npm run test:tools` checks the static test server with temporary fixture directories (malformed paths, root boundaries, redirects and deep-link fallback), and the production deployment wait against a local HTTP server (revision matching, transient failures and bounded timeouts). CI runs it before the browser fixtures.
- **Browser regressions**: `npx playwright test '\.fixture\.spec\.ts$'` runs deterministic browser tests against the local build, with API and media responses supplied by fixtures. CI runs these on pull requests and pushes to `main`; new deterministic suites should use the `.fixture.spec.ts` suffix. They exercise the code under review without depending on live data. Failure reports are uploaded for 7 days.
- **Static payloads**: CI also builds the `/Allthing` copy and runs `E2E_PAGES=1 npx playwright test e2e/static-body-limit.fixture.spec.ts e2e/static-metadata.fixture.spec.ts`. These fixtures exercise the browser Host's capped HTML streams, cleanup after read failures, complete uncapped JSON responses, and linked-page metadata filtering with inert fallback text and actual frame declarations.
- **Performance**: `e2e/perf.spec.ts` checks the budget in [PERFORMANCE.md](PERFORMANCE.md); CI runs it against production after each push to `main`.
- **End to end**: `e2e/allthing.spec.ts` runs Playwright against `wrangler dev`, or against production with `E2E_BASE_URL`, using live data. Video playback runs against a small VP9/Opus HLS fixture (`e2e/fixtures/hls`), because Playwright's Chromium cannot decode H.264.
