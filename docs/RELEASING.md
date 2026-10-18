<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Releasing

How Allthing is versioned, how changes reach `main`, and how a release is cut. `main` is always deployable: Cloudflare Workers Builds deploys every push to it ([DEPLOY.md](DEPLOY.md)).

## Versions

- **Semantic Versioning**, `MAJOR.MINOR.PATCH`. While Allthing is a prototype it stays on `0.x`:
  - **MINOR** (`0.3.0` → `0.4.0`) for new features or anything a reader would notice as a change;
  - **PATCH** (`0.3.0` → `0.3.1`) for fixes only.
  - `1.0.0` will mean the API paths and URLs are stable.
- **One source of truth**: `version` under `[workspace.package]` in the root `Cargo.toml`. Every crate inherits it (`version.workspace = true`). `package.json` repeats it for npm tooling, and `scripts/check.sh` fails if the two differ.
- **Where it shows**: the site footer (`v0.3.0 (abc1234)`), the Worker's `/version` endpoint (`{"name":"allthing","version":"0.3.0","commit":"abc1234"}`), and the Worker's upstream `User-Agent`. The commit is stamped at build time from `WORKERS_CI_COMMIT_SHA` (Workers Builds) or `git rev-parse` (`scripts/build.sh`).
- Releases are not named v1, v2, v3. A release is its version number, and its notes say what changed.

## Commits

[Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/): `type(scope): summary`, in the imperative, under about 72 characters.

| Type | For | Version bump |
|---|---|---|
| `feat` | a new capability | MINOR |
| `fix` | a bug fix | PATCH |
| `perf` | faster or lighter, same behaviour | PATCH |
| `refactor` | restructuring, same behaviour | none |
| `docs` | documentation only | none |
| `test` | tests only | none |
| `chore` | tooling, dependencies, release housekeeping | none |

Scopes are optional, for example `feat(profile): …`, `fix(worker): …` or `perf(feed): …`. A breaking change adds `!` (`feat!: …`) and a `BREAKING CHANGE:` footer.

## Branches and pull requests

- Work happens on short-lived branches cut from `main`, such as `feat/search`, `fix/video-mute` or `docs/vote`. Keep them to days, not weeks.
- Each branch is merged through a pull request once CI (`check`) is green, using **squash merge**. The squash commit's title is a Conventional Commit, usually the PR title.
- Nobody pushes features straight to `main`. Release housekeeping (`chore(release): …`) goes through a PR like anything else.
- Update `CHANGELOG.md` under `[Unreleased]` in the same PR as the change.

## Cutting a release

1. Decide the version from what is under `[Unreleased]`: any `feat` means MINOR, only fixes mean PATCH.
2. On a branch `chore/release-X.Y.Z`:
   - set `version` in `Cargo.toml` (workspace) and `package.json`, and run `cargo check` so `Cargo.lock` follows;
   - in `CHANGELOG.md`, move `[Unreleased]` items under `## [X.Y.Z] - YYYY-MM-DD` and update the compare links at the bottom;
   - run `bash scripts/check.sh` and the end-to-end suite (`npx playwright test`).
3. Open the PR `chore(release): X.Y.Z`, wait for green CI, and squash-merge it.
4. Tag the merge commit on `main` and push the tag:
   ```sh
   git switch main && git pull
   git tag -a vX.Y.Z -m "Allthing X.Y.Z"
   git push origin vX.Y.Z
   ```
5. Create the GitHub release from the tag, with that version's changelog section as notes:
   ```sh
   gh release create vX.Y.Z --title "Allthing X.Y.Z" --notes-file <(awk '/^## \[X.Y.Z\]/{f=1;next} /^## \[/{f=0} f' CHANGELOG.md)
   ```
6. Workers Builds deploys the merge. Check that `https://allthing.social/version` reports `X.Y.Z` and the merge commit. Then run `E2E_BASE_URL=https://allthing.social npx playwright test`.

## Hotfixes

Branch `fix/…` from `main` and fix with a test. Merge it, then release `X.Y.(Z+1)` the same way. Rolling back is a dashboard action (Workers & Pages → allthing → Deployments → Rollback); follow it with a fix release so `main` and production agree again.
