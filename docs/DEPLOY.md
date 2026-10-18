<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Deploying Allthing

Allthing is a single Cloudflare Worker named `allthing`, on the Workers Free plan. **Workers Builds** deploys it from this repository's `main` branch. No API token, account secret or session lives in the repository, and none is needed: Workers Builds creates its own token when the repository is connected.

## Connect the repository (once)

In the Cloudflare dashboard for account `c647739b4535010b0b13e6fc296e2c4e`, go to **Workers & Pages → Create → Import a repository**, pick `ael-dev3/Allthing`, and enter:

| Setting | Value |
|---|---|
| Project / Worker name | `allthing` (must match `name` in `wrangler.toml`) |
| Production branch | `main` |
| Build command | `bash scripts/build.sh` |
| Deploy command | `npx wrangler deploy` |
| Root directory | `/` (the repository root; leave the field empty if it is blank by default) |
| API token | Create new token (the default) |
| Build variables | none needed |

Optional: turn off **builds for non-production branches** to save build minutes, since previews are not used.

Pushing to `main` then builds and deploys. The first deploy also:

- attaches the custom domains `allthing.social` and `www.allthing.social` from `routes` in `wrangler.toml`. Cloudflare creates their DNS records and certificates, which can take a few minutes; the zone's DNS should not already have CNAME records for them;
- makes the `API_LIMITER` rate-limiting binding live (no setup; its counters are per Cloudflare location);
- binds the D1 database `allthing-index` (created by the owner in the same account; its id is in `wrangler.toml`) as `INDEX`, and starts the cron trigger that runs the indexer every minute. The Worker creates the index's tables itself on its first run (`migrations/0001_index.sql`, every statement `IF NOT EXISTS`). Applying the migrations with wrangler is optional, and records them as applied: `npx wrangler d1 migrations apply allthing-index --remote`. If the deploy fails because the Workers Builds token can't bind D1, give that token the **D1: Edit** permission (**My Profile → API Tokens**).

### What the build does

`scripts/build.sh` runs on the Workers Builds image (Ubuntu, 2 vCPUs and a 20-minute limit on the Free plan). The image has no Rust, so the script installs it:

1. rustup and the toolchain pinned in `rust-toolchain.toml` (Rust 1.99.1 with the `wasm32-unknown-unknown` target);
2. `wasm-bindgen` CLI at the version in `Cargo.lock` (a prebuilt binary from its GitHub release), and `worker-build` (compiled with `cargo install`);
3. the Worker: `worker-build --release` in `crates/allthing-worker` → `crates/allthing-worker/build/`;
4. the browser app: `cargo xtask dist` → `dist/` (fingerprinted WebAssembly, JS glue, CSS, fonts, `index.html`, `_headers`);
5. the spam-label shards: `dist/data/labels/spam/`. If the live site already serves shards for the current upstream dataset, they are copied from it; otherwise the dataset (about 125 MB) is downloaded from [warpcast/labels](https://github.com/warpcast/labels), checked against its Git LFS hash, and compacted by `cargo xtask labels`. If that fails, the build still succeeds and profiles say the label data is unavailable. Set the build variable `SKIP_LABELS=1` to skip this step.

A clean build on 2 CPUs takes about 5 minutes, most of it compiling (see "Build timing" below). `npx wrangler deploy` then uploads the Worker and the `dist/` assets. Only changed assets are uploaded.

### Build timing

A rehearsal from a fresh clone, on a 2-CPU Linux machine with no Rust and empty caches (as on Workers Builds), took **4 min 51 s**:

| Step | Time |
|---|---|
| rustup, the toolchain, and the `wasm-bindgen` download | under 30 s |
| Install `worker-build` (`cargo install`) | 2 min 34 s |
| Worker (`worker-build --release`) | 31 s |
| Browser app (`cargo build`, `wasm-bindgen`, `wasm-opt`) | 1 min 10 s |
| Label shards, only when the upstream dataset changed | about 1 min more |

## After the first deploy

1. Open <https://allthing.social>. `https://allthing.social/api/v1/status` should answer with JSON, and `https://allthing.social/version` with the version and commit just deployed.
2. Run the end-to-end suite against production: `E2E_BASE_URL=https://allthing.social npx playwright test`.
3. **Recommended:** make `www` redirect with a real HTTP 301 at no Worker cost. Go to the zone's **Rules → Redirect Rules**, choose the template **Redirect from WWW to root**, and deploy it. Without it, `www.allthing.social` still redirects: API calls get a 301 from the Worker, and pages are sent to the apex by the first script that runs (`web/boot.js`).
4. Optional protections that stop abusive traffic before it reaches the Worker, so it doesn't count against the 100,000 requests a day: a **WAF rate-limiting rule** on `/api/` and **Bot Fight Mode** (both under the zone's Security settings).

## Rolling back

In the dashboard, **Workers & Pages → allthing → Deployments** lists every version; pick one and choose **Rollback**. A rollback restores the Worker and its static assets together.

## The GitHub Pages copy

`https://ael-dev3.github.io/Allthing/` now redirects to `https://allthing.social`, keeping the path and query. The `gh-pages` branch holds a two-file redirect (`index.html` and `404.html`).

The static build of the app still works and can be published from the `gh-pages` branch again, without GitHub Actions. That build runs the payload builders in the browser (see [ARCHITECTURE.md](ARCHITECTURE.md#the-static-copy-github-pages)). To update it:

```sh
bash scripts/build.sh             # toolchain, label shards (dist/data), Cloudflare build
bash scripts/build-pages.sh       # dist-pages/, served under /Allthing
E2E_PAGES=1 npx playwright test   # check it locally
# then commit the contents of dist-pages/ to the gh-pages branch and push it
```

With the redirect in place, `…/Allthing/u/dwr` lands on `https://allthing.social/u/dwr`.

## Local development

```sh
npm ci                                 # wrangler and Playwright
SKIP_LABELS=1 bash scripts/build.sh    # or without SKIP_LABELS to build the label shards
npx wrangler dev                       # http://127.0.0.1:8787, with a local Cache API
```

`wrangler dev` serves `dist/` and runs the Worker from `crates/allthing-worker/build/`; rerun the build after changing Rust code. It gives the `INDEX` binding a local D1 database (in `.wrangler/state`), which starts empty. To run the indexer by hand, call `curl 'http://127.0.0.1:8787/cdn-cgi/handler/scheduled?cron=*+*+*+*+*'` once per run (`/__scheduled` would reach the static assets); `/api/v1/status` shows its progress. A manual deploy from a laptop also works (`npx wrangler login`, then `npx wrangler deploy`) with the owner's own login. Never commit a token, `.dev.vars` or `.env` file; `.gitignore` excludes them.

## Limits to keep in mind

See [BUDGET.md](BUDGET.md) for request, CPU and subrequest budgets. The deployed sizes are well within the Free plan: the Worker is about 1.1 MB (360 KB compressed, limit 3 MB), and `dist/` holds about 50 files (limit 20,000, each up to 25 MiB).
