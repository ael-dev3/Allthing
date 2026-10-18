<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Changelog

All notable changes to Allthing are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (0.x while it is a prototype; see [docs/RELEASING.md](docs/RELEASING.md)).

## [Unreleased]

### Fixed

- Popovers (who liked, recasted, replies) open toward the side with more room, so the replies preview on the first card no longer slides under the sticky header. Their height fits the space available, and the card with an open popover sits above the next card instead of under it.

## [0.3.0] - 2026-10-18

### Added

- **Search**: one box in the header for casts, people and channels, with results grouped. Search hits are re-read in bulk so they carry their media and counts.
- **Who liked, recasted and replied**:
  - Hovering (or focusing) a likes or recasts count shows the ten newest people. Clicking opens the full list (up to 100, as many as Hypersnap returns).
  - Hovering a replies count shows a scrollable preview of the most engaged replies, with avatars, scores, text and image thumbnails, and a link to the full thread. On touch screens a tap opens it.
- **Wallets popover** on profiles: every verified EVM and Solana address, each with one-click copy and a "Copied" confirmation.
- **The Farcaster vote** on profiles: whether the account voted to stay or to leave when farcaster.xyz asked, or did not vote.
  - Stay votes are read onchain on Base, from Network School's opt-in NFT keyed by FID.
  - Leave votes come from farcaster.xyz's public API, since they are not recorded onchain.
  - See [docs/VOTE.md](docs/VOTE.md).
- **Instant navigation** ([docs/PERFORMANCE.md](docs/PERFORMANCE.md)):
  - A post opens from the cast data already in the feed and loads its replies behind it.
  - Hovering or touching a card prefetches its post, and cards that stay in view are prefetched in idle time, within a capped budget.
  - Infinite scroll loads the next trending page before the reader reaches the end.
  - Navigation uses View Transitions and Speculation Rules, with preconnects to media hosts.
  - Back and forward restore the scroll position.
  - CI checks a performance budget against the live site.
- **Videos autoplay muted when scrolled into view** and pause when scrolled away, one at a time. They never autoplay with reduced motion or Save-Data.
- The whole post card is clickable, while text stays selectable and links inside it still work.
- **Release process**: Semantic Versioning from the workspace `Cargo.toml`, Conventional Commits, this changelog, tagged GitHub releases, and short-lived branches merged by squash ([docs/RELEASING.md](docs/RELEASING.md)).
- The version and short commit appear in the footer, and the Worker answers `/version` (also `/api/v1/version`).
- A test of Neynar scores for known FIDs, and [docs/SCORE.md](docs/SCORE.md) on why the score is read onchain.

### Changed

- **Casts that embed mini apps or frames are filtered out server-side**: from the front page, channels, profiles, search and reply previews. They are detected by URL (`farcaster.xyz/miniapps/…`, `farcaster.xyz/~/mini-apps…`, `warpcast.com/~/frames…`, `*.neynar.app`, including links in the text), or by `fc:frame`/`fc:miniapp` metadata where an API provides it. In threads, a mini app reply is dropped, unless it has replies of its own; then it stays as "Mini app post hidden" so the thread holds together.
- Profiles keep only the wallet activity infographic in the side area.
- The header has Home, Channels and search; the footer is one line (license, source, version).
- Changing the sort reorders the list in place instead of rebuilding the page, and expanding replies keeps the scroll position.
- Neynar scores are cached for 12 hours instead of 6 (they change weekly at most).
- GitHub Pages now redirects to https://allthing.social, keeping paths and queries.

### Removed

- The "Hypersnap" backend badges on casts, the "Read from" and ingress lines of the post's provenance box, and "view on farcaster.xyz" links.
- The keyboard shortcuts box and the j/k/o/x bindings, with their selection highlight. The media viewer keeps its arrow keys and Escape.
- The About page, the intro box, "How ranking works", "Where data comes from", and the "Front page" heading.
- Hover enlargement of images.
- On profiles: the verified addresses list, the "Posts via" breakdown, the per-chain "no activity" notes, the Solana note and "How this is measured".

### Fixed

- Clicking play on a video that was autoplaying muted could leave it muted.
- Search queries with characters that Hypersnap's query parser treats as syntax (`:`, quotes, brackets) no longer fail.

## [0.2.0] - 2026-10-17

### Added

- Allthing rebuilt in Rust:
  - a Leptos WebAssembly app;
  - a workers-rs API Worker on Cloudflare (Free plan) at https://allthing.social, with edge caching, request coalescing and upstream guards;
  - an original light design.
- Engagement shown as it is (likes, recasts, replies; no downvotes) with engagement-based ranking.
- Inline media with an accessible media viewer, a Rust HLS player, and click-to-load YouTube, Vimeo, Loom and Streamable players.
- Neynar scores beside names; profiles with registration date, spam labels from the Farcaster app's dataset, and wallet activity by chain.
- The signing app shown on each cast, and [docs/PROVENANCE.md](docs/PROVENANCE.md).
- A static GitHub Pages build of the same app (`static-host`).
- AGPL-3.0-or-later licensing with a dependency license audit.

## [0.1.0] - 2026-10-17

### Added

- A read-only Next.js prototype with live Hypersnap data, on GitHub Pages.

[Unreleased]: https://github.com/ael-dev3/Allthing/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/ael-dev3/Allthing/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/ael-dev3/Allthing/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/ael-dev3/Allthing/releases/tag/v0.1.0
