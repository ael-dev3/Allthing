<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<p align="center">
  <a href="https://allthing.social">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="docs/wordmark-dark.svg">
      <img src="docs/wordmark.svg" alt="Allthing" width="320">
    </picture>
  </a>
</p>

<p align="center"><strong>A quiet, read-only window onto Farcaster.</strong></p>

<p align="center">
  <a href="https://github.com/ael-dev3/Allthing/actions/workflows/ci.yml"><img src="https://github.com/ael-dev3/Allthing/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-AGPL--3.0--or--later-blue" alt="License: AGPL-3.0-or-later"></a>
  <a href="https://github.com/ael-dev3/Allthing/releases"><img src="https://img.shields.io/github/v/release/ael-dev3/Allthing?sort=semver" alt="Latest release"></a>
</p>

Allthing is an independent Farcaster client, written in Rust, that shows trending casts, channels, threads and profiles with honest engagement counts, media that plays in place, and the app each cast was posted from. Nobody signs in and nothing is ever posted: it only reads the public network.

**Live at [allthing.social](https://allthing.social).**

![The Allthing front page: trending Farcaster casts with their likes, recasts and replies, and the app each one was posted from](docs/screenshots/home.png)

## Features

- **Reading without an account.** A trending front page that keeps loading as you scroll, channel feeds and a channel directory, posts with their parent, quoted casts and threaded replies that collapse and expand, profiles, and one search box for casts, people and channels. A click anywhere on a card opens the post.
- **Engagement as it is.** Farcaster has likes, recasts and replies and no downvotes, so those three counts are all Allthing shows. Feeds sort by **hot** (likes + 2 × recasts + replies, decaying with age), **new** or **top**, and the front page adds a Reddit-style time filter.
- **Who engaged.** Hovering or tapping a likes or recasts count lists the newest people, and a click opens the full list. The replies count previews the most engaged replies.
- **Profile cards.** Hovering a name or profile picture, or tapping it on a phone, shows a small card: the start of the bio, followers and following, when the account joined, its Farcaster vote, the Farcaster app's label and wallet activity by chain.
- **Media in place.** Feeds show each cast's whole text. A single image or video spans the card at its own shape, and images open in an accessible viewer with keyboard and swipe navigation, alt text, and focus that returns where it was. Farcaster's HLS videos play inline: natively in Safari, and elsewhere through a small HLS player written in Rust over Media Source Extensions. The topmost video in view autoplays, muted until you turn sound on, and nothing autoplays with reduced motion or Save-Data. YouTube, Vimeo, Loom and Streamable players load only when clicked. Images and video come straight from their hosts, never through Allthing.
- **Profiles with context.** The bio, followers and following, the registration date, and:
  - the Farcaster app's public spam label, attributed to it and never used to hide anyone's casts;
  - wallet activity by chain for the account's verified EVM addresses (Base, Ethereum, Optimism, Arbitrum, Polygon, Zora, Celo, Unichain and Monad), with a Wallets popover listing every verified address;
  - how the account voted when farcaster.xyz asked everyone to stay or leave ([docs/VOTE.md](docs/VOTE.md)).
- **Provenance.** Each cast names the app that registered its signing key. [docs/PROVENANCE.md](docs/PROVENANCE.md) studies what Snapchain and Hypersnap record, and proposes how a node, and then the protocol, could record the client and the entry point verifiably.
- **Less noise, nothing hidden by labels.** Casts that embed or link to mini apps or frames, which Allthing doesn't run, and spam caught by a few fixed content rules are left out by the Worker. Labels from third parties are shown with their source and never used to filter.
- **Fast and frugal.** Each page makes one API call (profiles, and profile cards as they open, also look up wallets and the vote). Posts open instantly from the feed's own data, cards are prefetched within a capped budget, and Back returns to the same place. The Worker caches every payload at the edge, so the whole site runs on the Cloudflare Workers Free plan.
- **Accessible.** It works with the keyboard throughout, gives images alt text, keeps focus inside dialogs, respects reduced motion and fits a phone screen.

## Architecture

Allthing is one Cloudflare Worker that serves a static WebAssembly app and a small JSON API. All of it is Rust, apart from generated JavaScript glue and a few lines of boot script.

```text
          ┌── static files ──▶ Workers Static Assets: the app, styles, fonts, label shards (free)
Browser ──┼── /api/v1 ───────▶ allthing-worker ──▶ edge cache ──▶ Hypersnap, public RPCs
          └── media ─────────▶ straight from its host, never proxied
```

The Worker builds each payload from [Hypersnap](https://github.com/farcasterorg/hypersnap), a read node on Farcaster's Snapchain network, and from public RPCs. It caches payloads at the edge with stale-while-revalidate, merges concurrent requests, guards upstream calls with token buckets, circuit breakers and timeouts, and keeps each request within 45 subrequests and the 10 ms CPU limit, so the number of readers barely changes the load upstream. The same app also builds as a static copy (the `static-host` feature) that runs the payload builders in the browser.

| Crate | Role |
|---|---|
| [`allthing-core`](crates/allthing-core) | Pure Rust: the data model, Hypersnap normalization, ranking, text, embeds, HLS playlists, provenance, wallets, labels, spam rules and cache policy. No I/O; most unit tests live here. |
| [`allthing-data`](crates/allthing-data) | Page payload builders and guarded upstream clients, behind a small `Host` trait so they run in the Worker and in the browser. |
| [`allthing-worker`](crates/allthing-worker) | The `/api/v1` Worker (workers-rs): routing, the per-IP rate limit, the edge cache and request coalescing. |
| [`allthing-web`](crates/allthing-web) | The browser app: Leptos, rendered client-side, compiled to WebAssembly. |
| [`xtask`](xtask) | Build tooling: fingerprinted static assets, the static copy and the spam-label shards. |

More in [docs/](docs):

- [Architecture](docs/ARCHITECTURE.md): crates, API, data sources, caching and the browser app
- [Budget](docs/BUDGET.md): fitting the Workers Free plan, and the load on upstreams
- [Performance](docs/PERFORMANCE.md): targets, instant navigation, prefetching and measurements
- [Provenance](docs/PROVENANCE.md): which client and which backend, with research and a protocol design
- [The Farcaster vote](docs/VOTE.md): where votes are recorded and how Allthing reads them
- [Product principles](docs/PRODUCT.md), [deploying](docs/DEPLOY.md), [releasing](docs/RELEASING.md) and the [roadmap](docs/ROADMAP.md)

## Quick start

You need Node.js 22.12 or later (for `wrangler` and Playwright), `bash`, `curl` and `git`. The build installs [rustup](https://rustup.rs) if it is missing, then the toolchain pinned in `rust-toolchain.toml`: Rust 1.99.1 with the `wasm32-unknown-unknown` target.

```sh
git clone https://github.com/ael-dev3/Allthing.git
cd Allthing
npm ci                                # wrangler and Playwright
SKIP_LABELS=1 bash scripts/build.sh   # the pinned toolchain, the Worker and the app
npx wrangler dev                      # http://127.0.0.1:8787
```

The first build also installs `wasm-bindgen` and `worker-build` into `target/tools`, so it takes a few minutes. Rerun it after changing Rust code. Without `SKIP_LABELS=1` it also builds the spam-label shards: it reuses the live site's when the dataset hasn't changed, and otherwise downloads the dataset (about 125 MB).

Checks and tests:

```sh
bash scripts/check.sh                       # version, CSP hashes, changelog links, rustfmt, Clippy, unit tests, cargo-deny
npx playwright install chromium             # once
npx playwright test '\.fixture\.spec\.ts$'  # deterministic browser regressions, as CI runs them
npx playwright test                         # everything, including live Farcaster data
E2E_BASE_URL=https://allthing.social npx playwright test         # against production
bash scripts/build-pages.sh && E2E_PAGES=1 npx playwright test   # the static copy
```

Playwright starts `wrangler dev` on port 8787 by itself. CI runs `check.sh` and the fixture regressions on pull requests and pushes to `main`, and checks the [performance budget](docs/PERFORMANCE.md) against production after each push to `main`. Cloudflare Workers Builds deploys `main` ([docs/DEPLOY.md](docs/DEPLOY.md)); the repository holds no secrets.

## Contributing

Bug reports, fixes and focused improvements are welcome, as long as Allthing stays read-only. Read [CONTRIBUTING.md](CONTRIBUTING.md) for the ground rules, the workflow and the checks, and follow the [code of conduct](CODE_OF_CONDUCT.md). Commits and pull request titles are [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/), each change a reader or operator would notice gets a line in the [changelog](CHANGELOG.md), and versions follow [Semantic Versioning](https://semver.org) ([docs/RELEASING.md](docs/RELEASING.md)).

## Security

Please report vulnerabilities privately, as [SECURITY.md](SECURITY.md) describes, not in public issues.

## License

Copyright © Allthing contributors. Allthing is free software under the [GNU Affero General Public License, version 3 or any later version](LICENSE) (`AGPL-3.0-or-later`), and its source files carry that SPDX identifier.

The AGPL is a strong copyleft license: if you distribute a modified version, **or run one as a network service for others**, you must offer its users the source code under the same license (AGPL section 13). The site's footer links to this repository for that reason. Keep that link, or an equivalent, in your own deployment.

<details>
<summary>Dependencies and third-party material</summary>

- **Rust crates.** Everything compiled into the app, the Worker or the build tooling uses a license compatible with AGPL-3.0-or-later: MIT, Apache-2.0, BSD-3-Clause, BSL-1.0, CC0-1.0, Unicode-3.0, Unlicense or Zlib, and many crates are dual-licensed. `cargo deny check licenses` enforces the allowlist in [deny.toml](deny.toml), and CI runs it.
- **Instrument Serif**, the display face, also outlined in the wordmark above: © The Instrument Serif Project Authors, under the [SIL Open Font License 1.1](web/fonts/OFL-InstrumentSerif.txt). It ships as separate font files with its license, as the OFL allows.
- **Generated JavaScript glue** from wasm-bindgen and worker-build, under MIT or Apache-2.0.
- **Development tools** (wrangler, workerd, Playwright and their npm dependencies) are used to build and test but are never shipped to readers. They are under MIT, Apache-2.0, ISC, 0BSD or CC0-1.0, apart from `sharp`'s libvips binaries, which are LGPL-3.0-or-later and also compatible.
- **Data.** The spam labels come from [warpcast/labels](https://github.com/warpcast/labels), which publishes no license, so the dataset isn't covered by Allthing's license. Its shards are built at deploy time and never committed, and the site credits the source. Farcaster content belongs to its authors and is shown as published.
- **Snapchain and Hypersnap** (GPL-3.0) are referenced, not included; see the [licensing note in docs/PROVENANCE.md](docs/PROVENANCE.md#8-licensing-note).

</details>

Allthing is independent. It is not affiliated with or endorsed by Farcaster, Neynar, the operators of Hypersnap, or Cloudflare. Its name comes from the Allthing, the Hegemony's open forum in Dan Simmons' *Hyperion Cantos*; no text or artwork from the books is included.
