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
4. the browser app: `cargo xtask dist` → `dist/` (fingerprinted WebAssembly and its brotli-11 copy, written with Node's zlib, JS glue, CSS, fonts, `index.html`, `_headers`).

A clean build on 2 CPUs takes about 5 minutes, most of it compiling (see "Build timing" below). `npx wrangler deploy` then uploads the Worker and the `dist/` assets. Only changed assets are uploaded.

### Build timing

A rehearsal from a fresh clone, on a 2-CPU Linux machine with no Rust and empty caches (as on Workers Builds), took **4 min 51 s**:

| Step | Time |
|---|---|
| rustup, the toolchain, and the `wasm-bindgen` download | under 30 s |
| Install `worker-build` (`cargo install`) | 2 min 34 s |
| Worker (`worker-build --release`) | 31 s |
| Browser app (`cargo build`, `wasm-bindgen`, `wasm-opt`) | 1 min 10 s |

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
bash scripts/build.sh             # toolchain and the Cloudflare build
bash scripts/build-pages.sh       # dist-pages/, served under /Allthing
E2E_PAGES=1 npx playwright test   # check it locally
# then commit the contents of dist-pages/ to the gh-pages branch and push it
```

With the redirect in place, `…/Allthing/u/dwr` lands on `https://allthing.social/u/dwr`.

## Secrets

- **`SESSION_SECRET`** (needed for sign-in): the key that signs sign-in cookies ([ARCHITECTURE.md](ARCHITECTURE.md#sign-in)), at least 32 characters, random. Make one with `openssl rand -base64 48` and set it once with `npx wrangler secret put SESSION_SECRET`, or in the Cloudflare dashboard (Workers & Pages → `allthing` → Settings → Variables and Secrets, type *Secret*). It is never committed, logged or sent anywhere. Without it, "Sign in" says sign-in isn't set up, and everything else works. Changing it signs everyone out. For local development, put a different random value in a `.dev.vars` file next to `wrangler.toml` (git-ignored) as `SESSION_SECRET=…`. `wrangler dev` presents requests to the Worker at the route's host, so a sign-in started locally names `allthing.social` (over http).
- **`KLIPY_API_KEY`** (optional, for GIFs): the app key of a [KLIPY](https://partner.klipy.com/) app, which the composer's GIF picker searches with. Create the app and its key in KLIPY's Partner Panel (API Keys), leave ads off for the key, and set it with `npx wrangler secret put KLIPY_API_KEY` or in the Cloudflare dashboard (type *Secret*). It is never committed. KLIPY requires its API and media to be reached from the reader's browser, never through a partner's server, so the Worker hands the key to signed-in browsers (`/api/v1/gifs/key`), and every request a browser makes to KLIPY carries it: it is public by KLIPY's design, not a credential to guard. A key in KLIPY's *testing* mode allows 100 API requests an hour, for all readers together; ask for production access in the Partner Panel before many people use it. Without the secret, the composer has no GIF button, and everything else works. To check that the deployed Worker has it, open `/api/v1/status`: `gifs.keyed` is `true` (it never shows the key).
- **`ALLTHING_FID`** and **`ALLTHING_CUSTODY_KEY`** (optional, for writing from Allthing; **not set on allthing.social**, which has no Farcaster account of its own by the owner's decision, so every write there goes to the Farcaster app: [ARCHITECTURE.md](ARCHITECTURE.md#handing-writes-to-the-farcaster-app)): the fid of a Farcaster account that stands for Allthing, and the private key of that account's custody address, 64 hex digits (`0x` optional). When a reader approves a key, the Farcaster app names this account as the app that asks, so give it Allthing's name and picture. Make a new account for it rather than using a personal one: the custody key controls the whole account. Its custody key is the first Ethereum account that the account's recovery phrase derives (`m/44'/60'/0'/0/0`); a wallet that imports the phrase can export it. Set both with `npx wrangler secret put ALLTHING_FID` and `npx wrangler secret put ALLTHING_CUSTODY_KEY`, or in the dashboard (type *Secret*); `ALLTHING_FID` may also be a plain variable. Never commit, paste or log the key. The Worker uses it only to sign readers' key requests ([ARCHITECTURE.md](ARCHITECTURE.md#liking-and-recasting)), and never sees readers' own keys. Without either, casts have no like or recast buttons, and everything else works. To check, open `/api/v1/status`: `writes.ready` is `true` and `writes.app_fid` names the account (never the key). Keys readers already approved keep working if the account or its key changes; a reader removes one in the Farcaster app.
- **`ALLTHING_SPONSOR`** (optional): `1` to have the account pay the onchain fee for readers' approvals, which the Farcaster app takes from it in warps (it needs at least 100). Without it, any fee is the reader's.
- **`SOLANA_RPC_URL`** (optional): a keyed Solana RPC endpoint, with its key in the URL, for Solana wallet counts. Set it once in the Cloudflare dashboard (Workers & Pages → `allthing` → Settings → Variables and Secrets, type *Secret*), or with `npx wrangler secret put SOLANA_RPC_URL`. It is never committed. Without it, a Solana read shows as unread (see [WALLETS.md](WALLETS.md)). For local development, put it in `.dev.vars` next to `wrangler.toml` (git-ignored), not in the repository. To check that the deployed Worker has it, open `/api/v1/status`: `solana_rpc.keyed` is `true` and `solana_rpc.host` names the provider's host (never the key).

## Local development

```sh
npm ci                                 # wrangler and Playwright
bash scripts/build.sh                  # the pinned toolchain, the Worker and the app
npx wrangler dev                       # http://127.0.0.1:8787, with a local Cache API
```

`wrangler dev` serves `dist/` and runs the Worker from `crates/allthing-worker/build/`; rerun the build after changing Rust code. It gives the `INDEX` binding a local D1 database (in `.wrangler/state`), which starts empty. To run the indexer by hand, call `curl 'http://127.0.0.1:8787/cdn-cgi/handler/scheduled?cron=*+*+*+*+*'` once per run (`/__scheduled` would reach the static assets); `/api/v1/status` shows its progress. A manual deploy from a laptop also works (`npx wrangler login`, then `npx wrangler deploy`) with the owner's own login. Never commit a token, `.dev.vars` or `.env` file; `.gitignore` excludes them.

## Limits to keep in mind

See [BUDGET.md](BUDGET.md) for request, CPU and subrequest budgets. The deployed sizes are well within the Free plan: the Worker is about 2.1 MB (634 KB compressed, limit 3 MB; the cryptography of sign-in and of endorsing readers' keys, and the QR encoder, are about 240 KB of it, 87 KB compressed), and `dist/` holds about 450 files, 378 of them the emoji's and 64 the stickers' (limit 20,000, each up to 25 MiB).
