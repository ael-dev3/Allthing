<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Wind-down record

**Development halted: 28 October 2026.** Allthing is no longer maintained. Its hosted infrastructure was deleted on the same day; this file records what existed so the project can be rebuilt from this repository.

## State at halt

- Last tagged release: **0.5.0**. `Cargo.toml` on `main` and the live `/version` endpoint reported **0.5.1** (commit `b9adf2f`) at halt; `CHANGELOG.md` has further unreleased work.
- Last Worker deployment: 2026-10-27 20:16 UTC, version `cbf0c285-4ae6-48de-8663-4d97e83fbc7b`.

## Architecture (summary)

A Farcaster client. Rust compiled to WebAssembly for the browser app (`web/`, `crates/`), served as static assets by a Cloudflare Worker (`crates/allthing-worker`), which also runs `/api/*`, `/version`, `/wasm/*` and `/m/*` (attachments). A cron-driven indexer fills a D1 database used for rankings and activity. Attachments live in R2. Realtime chat, DMs, library (bookmarks/seen), notification read state and mute/block filters live in a SpacetimeDB module (`spacetime/spacetimedb`) on Maincloud, with messages attested by an Ed25519 key whose public half is pinned in the module. See `docs/ARCHITECTURE.md`.

## Infrastructure inventory (as of 28 Oct 2026, before deletion)

### Cloudflare (account `c647739b4535010b0b13e6fc296e2c4e`, Free plan)

| Resource | Name / ID | Notes |
|---|---|---|
| Worker | `allthing` | Deployed by Workers Builds from this repo; config in `wrangler.toml` |
| Custom domains | `allthing.social`, `www.allthing.social` | Worker custom domains (www redirects to apex) |
| Static assets | binding `ASSETS`, dir `dist` | SPA fallback; worker-first for `/api/*`, `/version`, `/wasm/*`, `/m/*` |
| Rate limiter | `API_LIMITER`, namespace_id `1001` | 120 req / 60 s per IP |
| D1 | `allthing-index`, id `184a345b-13ba-4db9-b1a6-4ea3a9c60f8c`, binding `INDEX` | ~4.2 MB; schema in `docs/archive/d1-allthing-index-schema.sql` |
| R2 | `allthing-media`, binding `MEDIA`, location EEUR | 6 objects, ~90 MB (user attachments); r2.dev public URL disabled, no custom domains |
| Cron | `* * * * *`, `*/1 * * * *`, `*/2 * * * *` | indexer and snapshot jobs |
| Vars | `ALLTHING_FID = "3356318"` (@allthingapp) | public |
| Secrets (names only) | `ALLTHING_CUSTODY_KEY`, `ATTEST_KEY`, `KLIPY_API_KEY`, `SESSION_SECRET` | values never stored in the repo |
| KV namespaces | none | |
| Pages projects | none for Allthing | |
| DNS zone | `allthing.social` (nameservers `hadlee.ns.cloudflare.com`, `ridge.ns.cloudflare.com`) | Zone and domain registration are **kept**. Apex/www A/AAAA records were the Worker custom domain records only; no MX/TXT. |

D1 contents at halt (counts only; rows are derived from public Farcaster data and readers' activity, not exported): casts 7405, rankings 3, snapshots 4, activity 216, signer_apps 13010, scheduled 1, reactions 2, media 6, state 4 (indexer cursors).

### SpacetimeDB

- Server: Maincloud (`maincloud.spacetimedb.com`), database name `allthing`, identity `c2000f7712b2effd34dfb76c93bdd2571c90ad730be5971488ec49bc08d9fc95`, owner identity `c200431f50cf410304546489e2600c8446408f1aa727c91d86bc00aef6724d15`.
- Config: `spacetime/spacetime.json` (module path `./spacetimedb`, server `maincloud`). Full schema: `docs/archive/spacetimedb-schema.json`.
- Tables: typing, user, library_post, dm_message, account_filter, rate_limit, room, user_key, notification_state, library_account, chat_message, notification_reader.
- Reducers: clear_seen, create_room, delete_dm, forget_seen, init, mark_notifications_seen, mark_seen_batch, on_connect, on_disconnect, register, send_dm, send_message, set_account_filter, set_bookmark, set_public_key, set_typing, verify_fid, verify_notification_fid.
- Views: my_account_filters, my_dm_inbox, my_dm_outbox, my_library_membership, my_library_page, my_library_state, my_notification_state.
- Row counts at halt (not exported, personal data): user 1, library_post 275, account_filter 2, chat_message 1, room 1, user_key 1, notification_state 1, library_account 1, notification_reader 2, rate_limit 1, dm_message 0, typing 0.

## Redeploying from scratch

1. Cloudflare: create D1 `allthing-index` (`npx wrangler d1 create allthing-index`) and R2 bucket `allthing-media` (`npx wrangler r2 bucket create allthing-media`); put the new D1 id and your `account_id` in `wrangler.toml`.
2. Build: `scripts/build.sh` (see `docs/DEPLOY.md`), then `npx wrangler deploy`. Tables are created by the Worker on first run (`migrations/`).
3. Secrets: `SESSION_SECRET`, `KLIPY_API_KEY`, `ALLTHING_CUSTODY_KEY` via `npx wrangler secret put`; new attestation key with `cargo xtask attest-key | npx wrangler secret put ATTEST_KEY` and commit the regenerated public key file.
4. SpacetimeDB: `cd spacetime && spacetime publish -s maincloud allthing` (see `spacetime/HANDOFF.md`).
5. Point custom domains in `wrangler.toml` at your own domain.
