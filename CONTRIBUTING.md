<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Contributing to Allthing

Thanks for helping. Small, focused changes are the easiest to review, so send **one pull request per change**. For anything larger, open an issue first that describes the reader's problem and the approach you have in mind.

Everyone taking part is expected to follow the [code of conduct](CODE_OF_CONDUCT.md). Report security problems privately, as [SECURITY.md](SECURITY.md) describes, never in a public issue.

## Ground rules

- **Reads only.** Don't add other ways to sign in, signing, wallet connections or anything else that writes to Farcaster unless that scope has been agreed in an issue first. The one sign-in is Sign In With Farcaster by QR code ([docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#sign-in)).
- **Never hide content** because of who posted it: Allthing shows no account labels. Casts with mini apps or frames, and spam by the fixed rules in `allthing-core/src/spam.rs`, are left out server-side (`Cast::hidden`, `thread::without_hidden`, `thread::prune_hidden`), and new list endpoints must do the same.
- **Stay within the Free plan.** Each page view makes one API call. The Worker has a 10 ms CPU limit and at most 45 subrequests per invocation, cache calls included. New upstream calls go through `allthing-data/src/upstream.rs` (token buckets, breaker, timeouts) and payloads through `cache::serve`. Update [docs/BUDGET.md](docs/BUDGET.md) when the costs change.
- **Put code where it belongs.** Pure logic and data types go in `allthing-core`, with unit tests. Provider quirks stay in `allthing-data/src/hypersnap.rs` and `allthing-core/src/normalize.rs`. The page builders in `allthing-data` also run in the browser for the static copy, so they reach the network only through the `Host` trait. UI goes in `allthing-web`, and its links are built with the helpers in `allthing-web/src/util.rs`, because the static copy lives under `/Allthing`.
- **Keep it fast.** Stay within the budget in [docs/PERFORMANCE.md](docs/PERFORMANCE.md), and send prefetches through `api::prefetch`.

## Workflow

1. Install [rustup](https://rustup.rs) and Node.js 22.12 or later, then run `npm ci`. `rust-toolchain.toml` pins Rust and the wasm32 target.
2. Branch from `main` with a short-lived branch named for the change: `feat/…`, `fix/…`, `perf/…`, `docs/…` or `chore/…`.
3. Build with `bash scripts/build.sh` and run with `npx wrangler dev` (http://127.0.0.1:8787). Rebuild after changing Rust code.
4. Test the change as described below.
5. Write commits as [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/): `feat`, `fix`, `perf`, `refactor`, `docs`, `test` or `chore`, with an optional scope, as in `fix(player): …`. [docs/RELEASING.md](docs/RELEASING.md) says which types bump the version.
6. Add a line to [CHANGELOG.md](CHANGELOG.md) under `[Unreleased]`, in the section it belongs to (Added, Changed, Removed or Fixed), for anything a reader or operator would notice.
7. Open a pull request whose title is a Conventional Commit, and fill in the template: what changed for readers, why, and how you checked it. It is squash-merged once CI is green. `main` must always be deployable: Cloudflare deploys every push to it.

## Tests

- **Before every push**, run `bash scripts/check.sh`. It checks that `Cargo.toml` and `package.json` agree on the version, that `web/_headers` holds the CSP hash of every inline script in `web/index.html`, and that each release in the changelog links to its tag. Then it runs rustfmt, Clippy (native and wasm32, with and without `static-host`), the unit tests and the `cargo deny` license audit.
- **For UI or API changes**, run the end-to-end suite and check the change at desktop (1280×900) and phone (390×844) widths. Run `npx playwright install chromium` once, then:
  - `npx playwright test '\.fixture\.spec\.ts$'`: the deterministic browser regressions that CI runs, with API and media responses from fixtures. Name new deterministic tests `*.fixture.spec.ts`.
  - `npx playwright test`: the whole suite, including `e2e/allthing.spec.ts` against live Farcaster data. Playwright starts `wrangler dev` on port 8787. Tests run at 1280×900; the phone-width tests and some fixture tests run at 390×844.
  - `npx playwright test e2e/perf.spec.ts`, for anything on the reading path.
  - `bash scripts/build-pages.sh && E2E_PAGES=1 npx playwright test`, if you touched the static copy.
- `npm run test:tools`, if you changed the static test server (`e2e/pages-server.mjs`).
- For UI changes, attach before-and-after screenshots at both widths to the pull request.

The live-data suite saves screenshots in `docs/screenshots/` (set `E2E_SCREENSHOT_DIR` to save them elsewhere). Only `home.png`, which the README shows, is tracked; commit a new one only when the README should change.

Versions follow SemVer from the workspace `Cargo.toml`. How releases are cut, tagged and published is in [docs/RELEASING.md](docs/RELEASING.md).

## Licensing

Allthing is licensed under **AGPL-3.0-or-later**. By contributing, you agree that your contribution is licensed under the same terms.

- Start new files with an SPDX header: `// SPDX-License-Identifier: AGPL-3.0-or-later` (or a `#` or `<!-- -->` comment, as the file type requires).
- New dependencies must be compatible with AGPL-3.0-or-later. `cargo deny check licenses` enforces the allowlist in `deny.toml`. Add a license there only if it is GPL-compatible, and say so in the pull request.
- Don't include code, fonts, images or data you aren't allowed to redistribute, and keep upstream license notices with anything third-party.

## Never commit

API tokens, account secrets, `.env` or `.dev.vars` files, private keys, session URLs, personal data exports, or build output (`dist/`, `dist-pages/`, `target/`, `crates/allthing-worker/build/`, `.wrangler/`). `.gitignore` covers the usual cases. Deployment credentials live in Cloudflare Workers Builds, not in this repository.
