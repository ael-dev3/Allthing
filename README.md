<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<p align="center">
  <a href="https://allthing.social">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="docs/wordmark-dark.svg">
      <img src="docs/wordmark.svg" alt="Allthing" width="320">
    </picture>
  </a>
</p>

<p align="center"><strong>A quiet window onto Farcaster, made for reading.</strong></p>

<p align="center">
  <a href="https://github.com/ael-dev3/Allthing/actions/workflows/ci.yml"><img src="https://github.com/ael-dev3/Allthing/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-AGPL--3.0--or--later-blue" alt="License: AGPL-3.0-or-later"></a>
  <a href="https://github.com/ael-dev3/Allthing/releases"><img src="https://img.shields.io/github/v/release/ael-dev3/Allthing?sort=semver" alt="Latest release"></a>
</p>

Allthing is an independent Farcaster client, written in Rust, that shows trending casts, channels, threads and profiles with honest engagement counts, media that plays in place, and the app each cast was posted from. No account is needed to read. A signed-in reader can like, recast, post with images and video, and schedule casts, with a key they approve once in the Farcaster app, which stays in their browser.

**Live at [allthing.social](https://allthing.social).**

![The Allthing front page: trending Farcaster casts with their likes, recasts and replies, and the app each one was posted from](docs/screenshots/home.png)

## Features

- **Reading without an account.** A trending front page that keeps loading as you scroll, channel feeds and a channel directory, posts with their parent, quoted casts and threaded replies that collapse and expand, profiles, and one search box for casts, people and channels. A click anywhere on a card opens the post.
- **Engagement as it is.** Farcaster has likes, recasts and replies and no downvotes, so those three counts are all Allthing shows. Feeds sort by **hot** (likes + 2 × recasts + replies, decaying with age), **new** or **top**, and the front page adds a Reddit-style time filter.
- **Who engaged.** Hovering a likes or recasts count or its icon, or tapping the count, lists the newest people; "See everyone", or a click on the count, opens the full list. The replies count previews the most engaged replies. All three come loaded with the cards around them, so they open at once.
- **Profile cards.** Hovering a name or profile picture, or tapping it on a phone, shows a small card: the start of the bio, followers and following, when the account joined, its NS vote and wallet activity by chain.
- **Media in place.** Feeds show each cast's whole text. A single image or video spans the card at its own shape, a GIF shows at about the size Discord gives one (at most 300 px each way), and images open in an accessible viewer with keyboard and swipe navigation, alt text, and focus that returns where it was. Farcaster's HLS videos play inline: natively in Safari, and elsewhere through a small HLS player written in Rust over Media Source Extensions. The topmost video in view autoplays, muted until you turn sound on, and nothing autoplays with reduced motion or Save-Data. YouTube, Vimeo, Loom and Streamable players load only when clicked. Images and video come straight from their hosts, never through Allthing.
- **Profiles with context.** The bio, followers and following, the registration date, and:
  - wallet activity by chain for the account's verified EVM addresses (Base, Ethereum, Optimism, Arbitrum, Polygon, Zora, Celo, Unichain and Monad), with a Wallets popover listing every verified address;
  - how the account voted when farcaster.xyz asked everyone to stay or leave ([docs/VOTE.md](docs/VOTE.md)).
- **Favorites, and seeing less.** Favorite on a profile makes the account one of yours: its casts rise well up your hot and top feeds, with a small star, and new keeps them in time order. Show less, beside it, is as strong the other way: their casts drop well down hot and top, and lower in new, with no mark, and nothing is hidden. Neither needs an account, and both stay in your browser.
- **Provenance.** Each cast names the app that registered its signing key. [docs/PROVENANCE.md](docs/PROVENANCE.md) studies what Snapchain and Hypersnap record, and proposes how a node, and then the protocol, could record the client and the entry point verifiably.
- **Less noise, judged by content.** Casts that embed or link to mini apps or frames, which Allthing doesn't run, and spam caught by a few fixed content rules are left out by the Worker. Nothing is hidden because of who posted it.
- **Fast and frugal.** Each page makes one API call (profiles, and profile cards as they open, also look up wallets and the vote). Posts open instantly from the feed's own data, cards are prefetched within a capped budget, and Back returns to the same place. The Worker caches every payload at the edge, so the whole site runs on the Cloudflare Workers Free plan. CI measures smoothness at phone and computer widths against budgets (a tap answered within 100 ms, no blank frame, Back to the same card) and records the ones not met yet ([docs/PERFORMANCE.md](docs/PERFORMANCE.md#smoothness)).
- **Sign in, and write, with one approval.** Approve Allthing once in the Farcaster app, by a QR code on a computer or a button on a phone: that approval signs you in and lets you write from Allthing. Your picture appears in the header; a click on it opens your profile, and its menu (on hover, or a tap on a phone) has your scheduled casts and Sign out.
- **Handed to the Farcaster app when you're signed out.** Signed out, or on a copy of Allthing that can't endorse keys, the heart and recast icons under each cast, Follow on a profile, and Delete under your own post or reply open the cast or the profile in the Farcaster app, to do it there: on a computer as a QR code for your phone (or on farcaster.xyz), on a phone straight in the app. No sign-in needed.
- **Emoji reactions.** React, under a post or a reply, picks a Pepe that goes out as a reply of that one emoji, since Farcaster's own reactions are only likes and recasts: the Farcaster app opens that reply, ready to post. A cast's emoji replies gather under it as chips with who reacted; a chip reacts with its emoji too, and your own takes your reaction back.
- **Write, with images, video, a GIF or a Pepe.** Cast (at the top of each feed, or a round button on a phone), and Reply and Quote under a post, open a composer: near the top of the window on a computer, a sheet from the bottom on a phone. Attach up to two images or videos (PNG, JPG, GIF or WebP; MP4, MOV or WebM) with the attach button, by pasting a screenshot or by dropping files; a photo's location and camera data are taken out in the browser before it's uploaded to Allthing's storage, and the cast embeds their links. It has a Pepe emoji picker (searchable, recently used first), Pepe stickers and a GIF picker (trending and search, from [KLIPY](https://klipy.com)). Signed out, Post in Farcaster hands the draft to the Farcaster app, ready to post.
- **Writing from Allthing itself.** Signed in, like, recast, cast, reply, quote, follow, delete and [schedule casts](docs/ARCHITECTURE.md#scheduled-casts) (listed, to edit or cancel, at /scheduled), shown "via Allthing". Each is a Farcaster message signed in your browser by the key you approved, which never leaves it, and sent from there to Hypersnap, a Snapchain node; the server never sees the key. Allthing asks for keys as its own account, @allthingapp (`ALLTHING_FID`), whose custody key, set in Cloudflare only (`ALLTHING_CUSTODY_KEY`, [docs/DEPLOY.md](docs/DEPLOY.md#secrets)), endorses each request.
- **Direct Casts.** Signed in, [Messages](docs/ARCHITECTURE.md#direct-casts), in the account menu, shows your Farcaster Direct Casts with a Farcaster API key you make in the Farcaster app and paste once: only your browser keeps it, and it goes only to Farcaster's API. That API lets only Farcaster's own sites read Direct Casts today, so for now Messages sends a new message from your browser and links to your inbox in the Farcaster app ([docs/DIRECT_CASTS.md](docs/DIRECT_CASTS.md)).
- **Pepe emoji.** `:pepe_name:` in a cast shows one of 219 Pepe emoji, most of them animated, served by Allthing itself; a cast of a few emoji and nothing else shows them large, and with reduced motion they stand still. Other Farcaster apps show the shortcode as written.
- **Pepe stickers.** 32 larger animated Pepes, from the composer's sticker button, recently used first. A sticker goes out as an image of its GIF, so every Farcaster app shows it.
- **Daily active accounts.** [allthing.social/dau](https://allthing.social/dau) counts Farcaster's active accounts as [SnapMeter](https://github.com/ael-dev3/SnapMeter) does, exactly, from the network's event stream, laid out like SnapMeter's dashboard: the past 24 hours against the 24 before, today so far, yesterday, the past 30 days and the 7-day average; the past 48 hours by the hour, or 7 or 30 days by UTC day, each with a table; what accounts did; which apps they used; and SnapMeter's definitions, term by term. An app's card in the front page's client bar shows its own count and how it's counted, and opens the page.
- **Accessible.** It works with the keyboard throughout, gives images alt text, keeps focus inside dialogs, respects reduced motion and fits a phone screen.

## Architecture

Allthing is one Cloudflare Worker that serves a static WebAssembly app and a small JSON API. All of it is Rust, apart from generated JavaScript glue and a few lines of boot script.

```text
          ┌── static files ──▶ Workers Static Assets: the app, styles, fonts (free)
Browser ──┼── /api/v1 ───────▶ allthing-worker ──▶ edge cache ──▶ Hypersnap, public RPCs
          ├── /m/ ───────────▶ allthing-worker ──▶ R2: the attachments readers uploaded
          ├── writes ────────▶ Hypersnap's submitMessage, signed in the browser
          └── media ─────────▶ straight from its host, never proxied
```

The Worker builds each payload from [Hypersnap](https://github.com/farcasterorg/hypersnap), a read node on Farcaster's Snapchain network, and from public RPCs. It caches payloads at the edge with stale-while-revalidate, merges concurrent requests, guards upstream calls with token buckets, circuit breakers and timeouts, and keeps each request within 45 subrequests and the 10 ms CPU limit, so the number of readers barely changes the load upstream. The same app also builds as a static copy (the `static-host` feature) that runs the payload builders in the browser.

| Crate | Role |
|---|---|
| [`allthing-core`](crates/allthing-core) | Pure Rust: the data model, Hypersnap normalization, ranking, text, embeds, HLS playlists, provenance, wallets, spam rules and cache policy. No I/O; most unit tests live here. |
| [`allthing-data`](crates/allthing-data) | Page payload builders and guarded upstream clients, behind a small `Host` trait so they run in the Worker and in the browser. |
| [`allthing-worker`](crates/allthing-worker) | The `/api/v1` Worker (workers-rs): routing, the per-IP rate limit, the edge cache and request coalescing. |
| [`allthing-web`](crates/allthing-web) | The browser app: Leptos, rendered client-side, compiled to WebAssembly. |
| [`xtask`](xtask) | Build tooling: fingerprinted static assets and the static copy. |

More in [docs/](docs):

- [Architecture](docs/ARCHITECTURE.md): crates, API, data sources, caching and the browser app
- [Budget](docs/BUDGET.md): fitting the Workers Free plan, and the load on upstreams
- [Performance](docs/PERFORMANCE.md): targets, instant navigation, prefetching and measurements
- [Provenance](docs/PROVENANCE.md): which client and which backend, with research and a protocol design
- [The NS vote](docs/VOTE.md): where votes are recorded and how Allthing reads them
- [Direct casts](docs/DIRECT_CASTS.md): how a client other than the Farcaster app can read and send them today, and what that needs
- [Product principles](docs/PRODUCT.md), [deploying](docs/DEPLOY.md), [releasing](docs/RELEASING.md) and the [roadmap](docs/ROADMAP.md)

## Quick start

You need Node.js 22.12 or later (for `wrangler` and Playwright), `bash`, `curl` and `git`. The build installs [rustup](https://rustup.rs) if it is missing, then the toolchain pinned in `rust-toolchain.toml`: Rust 1.99.1 with the `wasm32-unknown-unknown` target.

```sh
git clone https://github.com/ael-dev3/Allthing.git
cd Allthing
npm ci                                # wrangler and Playwright
bash scripts/build.sh                 # the pinned toolchain, the Worker and the app
npx wrangler dev                      # http://127.0.0.1:8787
```

The first build also installs `wasm-bindgen` and `worker-build` into `target/tools`, so it takes a few minutes. Rerun it after changing Rust code.

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

Bug reports, fixes and focused improvements are welcome, within Allthing's scope: it reads, its one sign-in is Farcaster's, and it writes only what a reader asks for, with a key they approved. Read [CONTRIBUTING.md](CONTRIBUTING.md) for the ground rules, the workflow and the checks, and follow the [code of conduct](CODE_OF_CONDUCT.md). Commits and pull request titles are [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/), each change a reader or operator would notice gets a line in the [changelog](CHANGELOG.md), and versions follow [Semantic Versioning](https://semver.org) ([docs/RELEASING.md](docs/RELEASING.md)).

## Security

Please report vulnerabilities privately, as [SECURITY.md](SECURITY.md) describes, not in public issues.

## License

Copyright © Allthing contributors. Allthing is free software under the [GNU Affero General Public License, version 3 or any later version](LICENSE) (`AGPL-3.0-or-later`), and its source files carry that SPDX identifier.

The AGPL is a strong copyleft license: if you distribute a modified version, **or run one as a network service for others**, you must offer its users the source code under the same license (AGPL section 13). The site's footer links to this repository for that reason. Keep that link, or an equivalent, in your own deployment.

<details>
<summary>Dependencies and third-party material</summary>

- **Rust crates.** Everything compiled into the app, the Worker or the build tooling uses a license compatible with AGPL-3.0-or-later: MIT, Apache-2.0, BSD-3-Clause, BSL-1.0, CC0-1.0, Unicode-3.0, Unlicense or Zlib, and many crates are dual-licensed. `cargo deny check licenses` enforces the allowlist in [deny.toml](deny.toml), and CI runs it.
- **Instrument Serif**, the display face, also outlined in the wordmark above: © The Instrument Serif Project Authors, under the [SIL Open Font License 1.1](web/fonts/OFL-InstrumentSerif.txt). It ships as separate font files with its license, as the OFL allows.
- **Pepe emoji and stickers** (`web/emoji/v1/`, `web/stickers/v1/`): public emotes from 7TV, FrankerFaceZ and emoji.gg, each credited with its source and maker in [web/emoji/CREDITS.md](web/emoji/CREDITS.md) and [web/stickers/CREDITS.md](web/stickers/CREDITS.md). They belong to their makers and are not covered by the AGPL. Pepe the Frog was created by Matt Furie.
- **Generated JavaScript glue** from wasm-bindgen and worker-build, under MIT or Apache-2.0.
- **Development tools** (wrangler, workerd, Playwright and their npm dependencies) are used to build and test but are never shipped to readers. They are under MIT, Apache-2.0, ISC, 0BSD or CC0-1.0, apart from `sharp`'s libvips binaries, which are LGPL-3.0-or-later and also compatible.
- **Data.** Farcaster content belongs to its authors and is shown as published.
- **Snapchain and Hypersnap** (GPL-3.0) are referenced, not included; see the [licensing note in docs/PROVENANCE.md](docs/PROVENANCE.md#8-licensing-note).

</details>

Allthing is independent. It is not affiliated with or endorsed by Farcaster, Neynar, the operators of Hypersnap, or Cloudflare. Its name comes from the Allthing, the Hegemony's open forum in Dan Simmons' *Hyperion Cantos*; no text or artwork from the books is included.
