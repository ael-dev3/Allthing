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
4. the browser app: `cargo xtask dist` → `dist/` (fingerprinted WebAssembly and its brotli-11 copy, written with Node's zlib, JS glue, the parts loaded when used and their loader, CSS, fonts, `index.html`, `_headers`).

A clean build on 2 CPUs takes about 5 minutes, most of it compiling (see "Build timing" below). `npx wrangler deploy` then uploads the Worker and the `dist/` assets. Only changed assets are uploaded.

### Build timing

A rehearsal from a fresh clone, on a 2-CPU Linux machine with no Rust and empty caches (as on Workers Builds), took **4 min 51 s**:

| Step | Time |
|---|---|
| rustup, the toolchain, and the `wasm-bindgen` download | under 30 s |
| Install `worker-build` (`cargo install`) | 2 min 34 s |
| Worker (`worker-build --release`) | 31 s |
| Browser app (`cargo build`, wasm-split, `wasm-bindgen`, `wasm-opt`) | 1 min 10 s |

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
- **`ALLTHING_CUSTODY_KEY`** (for writing from Allthing): the private key of the custody address of the account in `ALLTHING_FID`, which `wrangler.toml` sets to 3356318, @allthingapp: Allthing's own account, which its owner registered for it. With it, a signed-in reader approves Allthing once in the Farcaster app, and then casts, replies, quotes, likes, recasts, follows and deletes from Allthing, signed in their own browser ([ARCHITECTURE.md](ARCHITECTURE.md#liking-and-recasting)). The Farcaster app names @allthingapp as the app that asks, and Allthing shows those casts "via Allthing". Without it, every write goes to the Farcaster app ([ARCHITECTURE.md](ARCHITECTURE.md#handing-writes-to-the-farcaster-app)).
  - **What it is.** The first Ethereum account that the account's Farcaster recovery phrase derives (`m/44'/60'/0'/0/0`): 64 hex digits, `0x` optional. Its address must be the account's custody address, `0x501aa8834f58e97fb57fab7a1486ea29e4b8cd73` for FID 3356318, or readers' approvals fail.
  - **What it can do.** It controls the whole @allthingapp account: it can move the account to another address, change its recovery address, and add or remove its keys. So can anyone who reads it, or who gets code that reads it deployed. The Worker uses it for one thing: signing readers' key requests (an EIP-712 signature over the account's fid, the reader's public key and a deadline, `allthing-worker/src/app_key.rs`). It never logs or returns it, and never holds a reader's key.
  - **Getting it.** Only the account's owner can, from its recovery phrase, on his own device. The Farcaster app shows the phrase under Settings → Advanced (the wording may differ). Import it into a wallet app he already trusts on that device ("import a recovery phrase"): its first account must have the custody address above, or stop there. That account's "show private key" is the value. Remove the account from the wallet afterwards if it shouldn't stay. Never type the phrase or the key into a website, and never send either to anyone, the person setting the secret included: the owner pastes it into Cloudflare himself.
  - **Setting it.** Only in Cloudflare: the dashboard (Workers & Pages → `allthing` → Settings → Variables and Secrets → Add, type *Secret*, name `ALLTHING_CUSTODY_KEY`), or `npx wrangler secret put ALLTHING_CUSTODY_KEY` and paste it at the prompt. Never in the repository, a chat, an issue or a log. Then open `/api/v1/status`: `writes.ready` is `true`, `writes.app_fid` is `3356318`, and `writes.address`, the address the key belongs to, is `0x501aa8834f58e97fb57fab7a1486ea29e4b8cd73`. Any other address means the wrong key. The key itself never shows.
  - **Taking it back.** Delete the secret: writes go to the Farcaster app again at once, and keys readers already approved keep working until they remove Allthing in the Farcaster app. If it may have leaked, move the account to a new custody address straight away.
- **`ALLTHING_FID`** (in `wrangler.toml`, public): the account Allthing asks for readers' keys as, 3356318, @allthingapp. A fork sets its own. For the account's picture, Allthing serves its logo as a 512×512 PNG at `https://allthing.social/pfp.png` (`web/pfp.png`), since Farcaster apps show an SVG picture as blank. The URL is cached for a year (`web/_headers`), so a new logo gets a new file name.
- **`HYPERSNAP_ORIGIN`**: never set it. Only the end-to-end tests do, to put a stand-in on their own machine in the place of Hypersnap's node; the Worker ignores anything but a local port (`http://127.0.0.1:<port>`).
- **`ALLTHING_SPONSOR`** (optional): `1` to have the account pay the onchain fee for readers' approvals, which the Farcaster app takes from it in warps (it needs at least 100). Without it, any fee is the reader's. Set it as a *Secret* too: a deploy drops plain variables set only in the dashboard (Wrangler's `keep_vars` is off).
- **`SOLANA_RPC_URL`** (optional): a keyed Solana RPC endpoint, with its key in the URL, for Solana wallet counts. Set it once in the Cloudflare dashboard (Workers & Pages → `allthing` → Settings → Variables and Secrets, type *Secret*), or with `npx wrangler secret put SOLANA_RPC_URL`. It is never committed. Without it, a Solana read shows as unread (see [WALLETS.md](WALLETS.md)). For local development, put it in `.dev.vars` next to `wrangler.toml` (git-ignored), not in the repository. To check that the deployed Worker has it, open `/api/v1/status`: `solana_rpc.keyed` is `true` and `solana_rpc.host` names the provider's host (never the key).

## Attachments (R2)

Readers' images and videos ([ARCHITECTURE.md](ARCHITECTURE.md#attachments)) are kept in a Cloudflare R2 bucket, `allthing-media`, which the Worker reaches through its `MEDIA` binding. Until the binding is there, nothing else changes: uploads answer 404, the composer shows no attach button, and `/api/v1/status` says `"media": {"ready": false}`. The binding is commented out in `wrangler.toml` because `wrangler deploy` fails when a bound bucket doesn't exist, and Workers Builds deploys every push to `main`.

**What it costs.** R2 is free, each month, up to 10 GB-month of storage, 1 million Class A operations (an upload is one) and 10 million Class B operations (reading a file is one), and it has no egress fees ([R2 pricing](https://developers.cloudflare.com/r2/pricing/)). The free tier is for the Standard storage class only. The Worker never lets the files stored pass 9 GiB in all, under 10 GB however a gigabyte is counted, takes 2,000 uploads a day at most from everyone together (62,000 writes a month), and holds each reader to 300 MiB and 60 files a day ([BUDGET.md](BUDGET.md#12-attachments-r2)). R2 is priced apart from Workers, which the Workers Paid plan doesn't cover either, so the Workers plan stays Free ([Workers pricing](https://developers.cloudflare.com/workers/platform/pricing/)). Enabling R2 is a checkout that adds an R2 subscription to the account ([R2: get started](https://developers.cloudflare.com/r2/get-started/)), and Cloudflare wants a valid payment method on file before it enables a subscription ([billing policy](https://developers.cloudflare.com/billing/understand/billing-policy/)). Usage past the free tier would be billed to it; the caps keep storage under it, and operations far under it.

Once, in the Cloudflare dashboard for account `c647739b4535010b0b13e6fc296e2c4e`:

1. **Enable R2.** Go to **Storage & databases → R2 object storage → Overview**, and complete the checkout that adds R2 to the account. It asks for a payment method if there is none on file. The Workers plan doesn't change.
2. **Create the bucket.** On the same page, **Create bucket**: name it `allthing-media`, leave its location on **Automatic** (the default) and its default storage class on **Standard** (not Infrequent Access, which the free tier doesn't cover and which charges for reads), and **Create bucket** ([creating buckets](https://developers.cloudflare.com/r2/get-started/s3/), [data location](https://developers.cloudflare.com/r2/reference/data-location/), [storage classes](https://developers.cloudflare.com/r2/buckets/storage-classes/)). From a laptop logged in to wrangler, `npx wrangler r2 bucket create allthing-media` does the same. Leave its public access off: no public bucket, `r2.dev` address, custom domain or API token is needed, since the Worker reads and writes the bucket through its binding and serves the files itself, at `https://allthing.social/m/…`.
3. **Bind it.** Merge the pull request that uncomments these three lines in `wrangler.toml` (or uncomment them in one of your own):

   ```toml
   [[r2_buckets]]
   binding = "MEDIA"
   bucket_name = "allthing-media"
   ```

   Workers Builds then deploys it. Its token already has what an R2 binding needs: **Workers R2 Storage: Edit** is among the permissions it is created with ([Workers Builds configuration](https://developers.cloudflare.com/workers/ci-cd/builds/configuration/)).
4. **Check.** `https://allthing.social/api/v1/status` shows `"media": {"ready": true}`. Signed in, `https://allthing.social/api/v1/media` answers the limits: `{"max_image_bytes":10485760,"max_video_bytes":52428800,"max_embeds":2}`.

Afterwards:

- **Keep the bucket and the binding** once readers have attached files: their casts embed `https://allthing.social/m/…` for good, and without the binding those addresses answer 404, so every Farcaster app would show the casts' images and videos as broken.
- **Removing a file by hand** (abuse, a takedown): delete the object in the dashboard (**R2 → `allthing-media` → Objects**, its name is the URL's path after `/m/`), then its row in D1, so the total no longer counts it: `npx wrangler d1 execute allthing-index --remote --command "DELETE FROM media WHERE key = '<fid>/<id>.<ext>'"`, and `npx wrangler d1 execute allthing-index --remote --command "UPDATE media_total SET bytes = (SELECT COALESCE(SUM(bytes), 0) FROM media), files = (SELECT COUNT(*) FROM media) WHERE name = 'all'"`. Browsers that showed it may keep their copy, as each file is cached for a year.

## Local development

```sh
npm ci                                 # wrangler and Playwright
bash scripts/build.sh                  # the pinned toolchain, the Worker and the app
npx wrangler dev                       # http://127.0.0.1:8787, with a local Cache API
```

`wrangler dev` serves `dist/` and runs the Worker from `crates/allthing-worker/build/`; rerun the build after changing Rust code. It gives the `INDEX` binding a local D1 database (in `.wrangler/state`), which starts empty. It has no `MEDIA` binding, as `wrangler.toml` comments it out until the bucket exists: to try attachments locally, uncomment the R2 block without committing it, and `wrangler dev` keeps the files in a local R2 beside D1 (`e2e/media-upload.fixture.spec.ts` runs a Worker that way). To run the indexer by hand, call `curl 'http://127.0.0.1:8787/cdn-cgi/local/scheduled?cron=*+*+*+*+*'` once per run (`/__scheduled` would reach the static assets); `/api/v1/status` shows its progress. A manual deploy from a laptop also works (`npx wrangler login`, then `npx wrangler deploy`) with the owner's own login. Never commit a token, `.dev.vars` or `.env` file; `.gitignore` excludes them.

## Limits to keep in mind

See [BUDGET.md](BUDGET.md) for request, CPU and subrequest budgets. The deployed sizes are well within the Free plan: the Worker is about 2.1 MB (634 KB compressed, limit 3 MB; the cryptography of sign-in and of endorsing readers' keys, and the QR encoder, are about 240 KB of it, 87 KB compressed), and `dist/` holds about 450 files, 378 of them the emoji's and 64 the stickers' (limit 20,000, each up to 25 MiB).
