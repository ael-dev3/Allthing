<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Contributing

Thanks for helping. Small, focused pull requests are easiest to review. For larger changes, open an issue first describing the reader's problem and your approach.

## Ground rules

- **Read-only.** Don't add sign-in, signing, wallet connections or anything that writes to Farcaster unless that scope has been agreed in an issue first.
- **Never hide content** on the basis of a third-party label. Show labels where they belong, with their source.
- **Stay within the Free plan.** One API call per page; new upstream calls go through `allthing-data/src/upstream.rs` (guards, timeouts, budget) and the edge cache. Update [docs/BUDGET.md](docs/BUDGET.md) if the numbers change.
- **Keep provider quirks in `allthing-data/src/hypersnap.rs` and `allthing-core/src/normalize.rs`**, and keep pure logic in `allthing-core`, with tests. The builders in `allthing-data` also run in the browser for the static build, so they reach the network only through the `Host` trait.

## Workflow

1. Install [rustup](https://rustup.rs) and Node.js 22.12+, then `npm ci`. `rust-toolchain.toml` pins Rust and the wasm32 target.
2. Branch from `main` with a short-lived branch named for the change: `feat/…`, `fix/…`, `perf/…`, `docs/…` or `chore/…`.
3. Build with `SKIP_LABELS=1 bash scripts/build.sh` and run with `npx wrangler dev`.
4. Before pushing, run `bash scripts/check.sh` (version consistency, rustfmt, Clippy for native and wasm32, unit tests, `cargo deny` license audit). For UI or API changes, also run `npx playwright test`, and for anything on the reading path, `npx playwright test e2e/perf.spec.ts`.
5. Write commits as [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/): `feat`, `fix`, `perf`, `refactor`, `docs`, `test` or `chore`, with an optional scope (`fix(player): …`).
6. Add a line to `CHANGELOG.md` under `[Unreleased]` for anything a reader or operator would notice.
7. Open a pull request whose title is a Conventional Commit. It is squash-merged once CI is green. `main` must always be deployable: Cloudflare deploys every push to it.
8. Explain what changed for readers, and how you checked it.

Versions follow SemVer from the workspace `Cargo.toml`. How releases are cut, tagged and published is in [docs/RELEASING.md](docs/RELEASING.md).

## Licensing

Allthing is licensed under **AGPL-3.0-or-later**. By contributing, you agree that your contribution is licensed under the same terms.

- Start new source files with an SPDX header: `// SPDX-License-Identifier: AGPL-3.0-or-later` (or `#` or `<!-- -->` comments, as the file type requires).
- New dependencies must be compatible with AGPL-3.0-or-later. `cargo deny check licenses` enforces the allowlist in `deny.toml`. Add a license there only if it is GPL-compatible, and say so in the pull request.
- Don't include code, fonts, images or data you aren't allowed to redistribute, and keep upstream license notices with anything third-party.

## Never commit

API tokens, account secrets, `.env` or `.dev.vars` files, private keys, session URLs, personal data exports, or build output (`dist/`, `target/`, `crates/allthing-worker/build/`). `.gitignore` covers the usual cases. Deployment credentials live in Cloudflare Workers Builds, not in this repository.
