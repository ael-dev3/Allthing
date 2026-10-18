<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Notes for coding agents

Allthing is a read-only Farcaster client written in Rust: a Leptos WebAssembly app plus a workers-rs API Worker, deployed to Cloudflare Workers (Free plan) at https://allthing.social. Read `docs/ARCHITECTURE.md` first.

- **Read-only.** Never add login, signing, wallet connections or write paths unless asked to.
- **Never hide content** based on labels. Show labels where they belong, with their source.
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
- **Links.** The app's routes are `/`, `/c/<id>`, `/p/<hash>`, `/u/<name>`, `/channels` and `/search?q=`. Build them with the helpers in `crates/allthing-web/src/util.rs` (`href`, `channel_href`, …), never as literal strings: the GitHub Pages build lives under `/Allthing`.
- **Licensing.** Licensing is AGPL-3.0-or-later. Start new files with an SPDX header. New dependencies must pass `cargo deny check licenses`.
- **Secrets.** Never commit secrets, tokens, `.dev.vars` or `.env` files, or session URLs.
- **Checks.**
  - Run `bash scripts/check.sh` (fmt, Clippy native and wasm32, tests, license audit).
  - End to end: `SKIP_LABELS=1 bash scripts/build.sh && npx playwright test`. Playwright starts `wrangler dev` on port 8787.
  - Against production: `E2E_BASE_URL=https://allthing.social npx playwright test`.
  - The static copy: `bash scripts/build-pages.sh && E2E_PAGES=1 npx playwright test`.
- **Versions and commits.** Follow `docs/RELEASING.md`:
  - SemVer from the workspace `Cargo.toml` (keep `package.json` in step), not v1/v2/v3 names;
  - Conventional Commits;
  - a `CHANGELOG.md` entry under `[Unreleased]`;
  - short-lived `feat/*` or `fix/*` branches, squash-merged by PR with green CI;
  - releases tagged `vX.Y.Z` with a GitHub release.
- **Speed.** Keep within `docs/PERFORMANCE.md`. Prefetches go through `api::prefetch`, which caps and dedupes them.
- **Mini apps.** Casts with mini app or frame links are filtered out server-side (`Cast.mini_app`, `thread::without_mini_apps`). Keep new list endpoints doing the same.
- **Deploying.** Cloudflare Workers Builds deploys `main` with `bash scripts/build.sh` and `npx wrangler deploy`; see `docs/DEPLOY.md`.
