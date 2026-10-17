<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Roadmap

Milestones, not dates. Allthing stays read-only throughout; participation would need its own scope decision.

## Done

- **v0: seed.** Rust workspace and product direction.
- **v1: Next.js prototype.** Live data from Hypersnap, threads, profiles and keyboard navigation, on GitHub Pages. It has since been replaced.
- **v2: Allthing in Rust, on Cloudflare Workers** ([allthing.social](https://allthing.social)):
  - [x] Leptos WebAssembly app and a workers-rs API Worker; static assets free on the Workers Free plan
  - [x] Original light design with an "Allthing" wordmark, subtle motion and reduced-motion support
  - [x] Honest engagement (likes, recasts, replies; no downvotes) and engagement-based hot, new and top ranking
  - [x] Inline images, hover preview and an accessible media viewer
  - [x] Inline HLS video (native on Safari, a Rust Media Source Extensions player elsewhere) and click-to-load YouTube, Vimeo, Loom and Streamable players
  - [x] Profile pictures and Neynar scores beside names; profiles with registration date, counts and bio
  - [x] Spam labels from the Farcaster app's public dataset, shown and never used to hide
  - [x] Wallet activity by chain for verified EVM addresses
  - [x] Provenance per cast (signing app and backend) and the protocol design in [PROVENANCE.md](PROVENANCE.md)
  - [x] Edge caching with stale-while-revalidate, coalescing and upstream guards ([BUDGET.md](BUDGET.md))
  - [x] AGPL-3.0-or-later, with a dependency license audit in CI

## Next

- **A second backend.** Read the same casts from a Snapchain node (or a second Hypersnap operator). Show which backends returned a cast, and let readers pick the source. Reconcile by message hash, not by text.
- **Smaller app.** The WebAssembly module is about 410 KB compressed, mostly the Leptos runtime and `serde_json`. Candidates: a leaner JSON decoder for payloads, splitting rarely used views, and trimming web-sys features.
- **Solana activity.** Find a keyless source of Solana address history that allows browser or Worker requests, or document a key-based option that keeps the key out of the repository.
- **Reading controls kept in the browser.** Local mutes and filters (clearly separate from network labels), saved posts, and remembered collapse state.
- **Search** across casts and users, as far as public endpoints allow.
- **Ingress provenance.** Prototype phase 2 of [PROVENANCE.md](PROVENANCE.md), signed ingress receipts, against a Hypersnap fork, and show receipts when nodes publish them.
- **Pagination beyond the pool.** Cursor-based "more" for long channels and profiles, within the subrequest budget.

## Not planned

Sign-in, posting, reactions, wallets that sign, and anything that hides content based on a third party's label.
