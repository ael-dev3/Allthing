<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Roadmap

Milestones, not dates. Releases use SemVer ([RELEASING.md](RELEASING.md)); details per release are in the [changelog](../CHANGELOG.md). Allthing reads; the owner has since scoped a second phase, one feature at a time: Sign In With Farcaster by QR code first, then a post and reply composer.

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
- **Since 0.3.0, unreleased** ([changelog](../CHANGELOG.md#unreleased)):
  - [x] a Reddit-style time filter on the front page;
  - [x] each cast's whole text in feeds, and media at its own shape;
  - [x] video sound that stays on once the reader turns it on;
  - [x] spam, and mini apps on their own domains, left out;
  - [x] a plainer interface, without Neynar scores, the Channels sidebar or "Updated N min ago";
  - [x] browser regression tests on every pull request.

## Next

- **Phase 2: participating.** Sign In With Farcaster by QR code through the Farcaster app ([ARCHITECTURE.md](ARCHITECTURE.md#sign-in)); next, a post and reply composer with a GIF picker (search and trending, from a free GIF API). Posting needs a signer the account approves in the Farcaster app, and that scope comes with the composer.

- **A second backend.** Read the same casts from a Snapchain node (or a second Hypersnap operator). Show which backends returned a cast, and let readers pick the source. Reconcile by message hash, not by text.
- **Smaller app.** The WebAssembly module is about 410 KB compressed, mostly the Leptos runtime and `serde_json`. Candidates: a leaner JSON decoder for payloads, splitting rarely used views, and trimming web-sys features.
- **Solana activity.** Find a keyless source of Solana address history that allows browser or Worker requests, or document a key-based option that keeps the key out of the repository.
- **Reading controls kept in the browser.** Local mutes and filters, saved posts, and remembered collapse state.
- **Ingress provenance.** Prototype phase 2 of [PROVENANCE.md](PROVENANCE.md), signed ingress receipts, against a Hypersnap fork, and show receipts when nodes publish them.
- **Pagination beyond the pool** for channels and profiles: cursor-based loading as the reader scrolls, as the front page already does, within the subrequest budget.
- **Vote badges in feeds**: one Multicall3 batch per page ([VOTE.md](VOTE.md#alternatives-considered)).

## Not planned

Ways to sign in other than Farcaster's QR code, wallets that sign, and anything that hides content based on who posted it.
