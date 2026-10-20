<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Notes for coding agents

Allthing is a Farcaster client for reading, written in Rust: a Leptos WebAssembly app plus a workers-rs API Worker, deployed to Cloudflare Workers (Free plan) at https://allthing.social. Read `docs/ARCHITECTURE.md` first.

- **Sign in by QR code; write with a key the reader approves once.** The one sign-in is Sign In With Farcaster by QR code (`allthing-worker/src/auth.rs`, `allthing-core/src/siwf.rs`, `allthing-web/src/components/signin.rs`; see `docs/ARCHITECTURE.md#sign-in`). After it, a reader approves Allthing once in the Farcaster app, and casts, replies, quotes, likes, recasts, follows and deletes go from Allthing: the browser makes and keeps the key and signs (`allthing-core/src/app_key.rs` and `message.rs`, `allthing-web/src/keys.rs` and `components/writer.rs`; see `docs/ARCHITECTURE.md#liking-and-recasting`), and the Worker keeps scheduled casts the browser signed until their time (`allthing-worker/src/scheduled_casts.rs`, `allthing-data/src/scheduled.rs`). Allthing asks for those keys as its owner's own account, @0xael.eth (`ALLTHING_FID` in `wrangler.toml`), and shows the casts "via Allthing" (`allthing-core/src/clients.rs`). By the owner's decision there is no other Farcaster account, no seed phrase import and no wallet connection: never add one. The Worker endorses readers' keys with that account's custody key, the `ALLTHING_CUSTODY_KEY` secret, set in Cloudflare only (`allthing-worker/src/app_key.rs`; see `docs/DEPLOY.md#secrets`), and uses it for nothing else; it never holds a reader's key or signs as a reader. Until the secret is set (`writes.ready` in `/api/v1/status`), and for a reader signed out, a write opens the cast, the profile or a draft ready to post in the Farcaster app instead (`allthing-web/src/components/handoff.rs`, `allthing-core/src/compose.rs`; see `docs/ARCHITECTURE.md#handing-writes-to-the-farcaster-app`). Never add other login methods, wallet connections or write paths unless asked to.
- **Never hide content** because of who posted it. Allthing shows no account labels; spam is judged by a cast's content (see Mini apps and spam below).
- **Where code goes.**
  - Pure logic and data types, with unit tests, go in `crates/allthing-core`.
  - Provider quirks go in `crates/allthing-data/src/hypersnap.rs` and `crates/allthing-core/src/normalize.rs`.
  - Page payload assembly goes in `crates/allthing-data/src/pages.rs`. It runs in the Worker and, for the static GitHub Pages build (`static-host` feature), in the browser, so it must only reach the network through the `Host` trait.
  - UI goes in `crates/allthing-web` (pages in `src/pages`, components in `src/components`).
- **Budget.** Each page view makes exactly one API call. The Worker has a 10 ms CPU limit and at most 45 subrequests per invocation, cache calls included.
  - Upstream calls go through `allthing-data/src/upstream.rs` (token buckets, breaker, timeouts).
  - Payloads go through `cache::serve`.
  - Never await another request's I/O; see the coalescing notes in `cache.rs`.
  - Update `docs/BUDGET.md` when costs change.
- **Links.** The app's routes are `/`, `/c/<id>`, `/p/<hash>`, `/u/<name>`, `/channels`, `/search?q=`, `/dau` and `/scheduled`. Build them with the helpers in `crates/allthing-web/src/util.rs` (`href`, `channel_href`, …), never as literal strings: the GitHub Pages build lives under `/Allthing`.
- **Licensing.** Licensing is AGPL-3.0-or-later. Start new files with an SPDX header. New dependencies must pass `cargo deny check licenses`.
- **Secrets.** Never commit secrets, tokens, `.dev.vars` or `.env` files, or session URLs. The Worker's secrets (`SESSION_SECRET`, `SOLANA_RPC_URL`) are set in Cloudflare; see `docs/DEPLOY.md#secrets`.
- **Checks.**
  - Run `bash scripts/check.sh` (fmt, Clippy native and wasm32, tests, license audit).
  - End to end: `bash scripts/build.sh && npx playwright test`. Playwright starts `wrangler dev` on port 8787.
  - Against production: `E2E_BASE_URL=https://allthing.social npx playwright test`.
  - The static copy: `bash scripts/build-pages.sh && E2E_PAGES=1 npx playwright test`.
- **Versions and commits.** Follow `docs/RELEASING.md`:
  - SemVer from the workspace `Cargo.toml` (keep `package.json` in step), not v1/v2/v3 names;
  - Conventional Commits;
  - a `CHANGELOG.md` entry under `[Unreleased]`;
  - short-lived `feat/*` or `fix/*` branches, squash-merged by PR with green CI;
  - releases tagged `vX.Y.Z` with a GitHub release.
- **Speed.** Keep within `docs/PERFORMANCE.md`. Prefetches go through `api::prefetch`, which caps and dedupes them.
- **Mini apps and spam.** Casts with mini app or frame links, and spam by the rules in `allthing-core/src/spam.rs`, are filtered out server-side (`Cast::hidden`, `thread::without_hidden`, `thread::prune_hidden`). Keep new list endpoints doing the same.
- **Deploying.** Cloudflare Workers Builds deploys `main` with `bash scripts/build.sh` and `npx wrangler deploy`; see `docs/DEPLOY.md`.
