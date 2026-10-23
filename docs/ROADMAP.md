<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Roadmap

Milestones, not dates. Releases use SemVer ([RELEASING.md](RELEASING.md)); details per release are in the [changelog](../CHANGELOG.md). Allthing supports reading, participating through a reader-approved Farcaster key, and its own Messages.

## Done

- **Seed.** Rust workspace and product direction.
- **0.1.0: Next.js prototype.** Live data from Hypersnap, threads, profiles and keyboard navigation, on GitHub Pages. It has since been replaced.
- **0.2.0: Allthing in Rust, on Cloudflare Workers** ([allthing.social](https://allthing.social)):
  - [x] Leptos WebAssembly app and a workers-rs API Worker; static assets free on the Workers Free plan
  - [x] Original light design with an "Allthing" wordmark, subtle motion and reduced-motion support
  - [x] Honest engagement (likes, recasts, replies; no downvotes) and engagement-based hot, new and top ranking
  - [x] Inline images, hover preview and an accessible media viewer
  - [x] Inline HLS video (native on Safari, a Rust Media Source Extensions player elsewhere) and click-to-load YouTube, Vimeo, Loom and Streamable players
  - [x] Profile pictures beside names; profiles with registration date, counts and bio
  - [x] Wallet activity by chain for verified EVM addresses
  - [x] Provenance per cast (signing app and backend) and the protocol design in [PROVENANCE.md](PROVENANCE.md)
  - [x] Edge caching with stale-while-revalidate, coalescing and upstream guards ([BUDGET.md](BUDGET.md))
  - [x] AGPL-3.0-or-later, with a dependency license audit in CI
- **0.3.0: polish and speed**:
  - [x] instant post opening, prefetching, infinite scroll and scroll restoration;
  - [x] search;
  - [x] who-liked and replies popovers;
  - [x] videos that autoplay in view;
  - [x] the wallets popover and the NS vote;
  - [x] mini app filtering;
  - [x] a simpler interface;
  - [x] a release process.
- **Reading improvements since 0.3.0** ([changelog](../CHANGELOG.md)):
  - [x] a Reddit-style time filter on the front page;
  - [x] each cast's whole text in feeds, and media at its own shape;
  - [x] video sound that stays on once the reader turns it on;
  - [x] spam, and mini apps on their own domains, left out;
  - [x] a plainer interface, without Neynar scores, the Channels sidebar or "Updated N min ago";
  - [x] browser regression tests on every pull request.
- **Participating and Messages**:
  - [x] one Farcaster approval to sign in and write, with the key kept in the browser;
  - [x] casts, replies, quotes, likes, recasts, follows and deletes;
  - [x] a composer with attachments, GIFs, stickers, emoji and scheduled casts;
  - [x] notifications and verified Farcaster accounts in Messages, with end-to-end encrypted DMs.
- **0.5.1 release candidate** ([changelog](../CHANGELOG.md#051---2026-10-23)):
  - [x] For you and Following tabs, each with hot, new, top and every time range;
  - [x] browser-local seen-post history with Show seen posts and reset controls;
  - [x] mini app controls in Settings and a responsive Home/online-count header;
  - [x] fresh short-range casts and a shared public Home snapshot for cold data centers.

## Next

- **A second backend.** Read the same casts from a Snapchain node (or a second Hypersnap operator). Show which backends returned a cast, and let readers pick the source. Reconcile by message hash, not by text.
- **Smaller app.** Rarely used views already load as separate WebAssembly parts. Further candidates include a leaner JSON decoder, more selective splitting and trimming web-sys features; measurements belong in [PERFORMANCE.md](PERFORMANCE.md).
- **Solana activity.** Find a keyless source of Solana address history that allows browser or Worker requests, or document a key-based option that keeps the key out of the repository.
- **Reading controls kept in the browser.** Local mutes and filters, saved posts, and remembered collapse state.
- **Ingress provenance.** Prototype phase 2 of [PROVENANCE.md](PROVENANCE.md), signed ingress receipts, against a Hypersnap fork, and show receipts when nodes publish them.
- **Pagination beyond the pool** for channels and profiles: cursor-based loading as the reader scrolls, as the front page already does, within the subrequest budget.
- **Vote badges in feeds**: one Multicall3 batch per page ([VOTE.md](VOTE.md#alternatives-considered)).

## Not planned

Ways to sign in other than Farcaster's QR code, wallets that sign, and anything that hides content based on who posted it.
