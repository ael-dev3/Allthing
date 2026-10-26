<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Allthing SpacetimeDB module — handoff

Module: `spacetime/spacetimedb` in this repository (Rust, spacetimedb 2.13). Builds with `spacetime build`; from the repository root, `cargo xtask module-check` runs its isolated checks (CI's `SpacetimeDB module` job). `sh spacetime/check.sh` is a thin compatibility wrapper. The native command clears workspace `RUSTFLAGS` and retains rustfmt, native/Wasm Clippy, unit tests, the release Wasm build and the license audit when `cargo-deny` is installed (required by CI).
Target DB name: **`allthing`** (fall back to `allthing-chat` if the name is taken) on maincloud.
URL once published: `https://maincloud.spacetimedb.com` / ws, database `allthing`. Dashboard: https://spacetimedb.com/allthing
STATUS: the existing `allthing` database was confirmed on 21 October 2026, with `general` seeded as room 1. That historical check predates the notification-state additions below and does not confirm their deployment; publish the updated module and verify its schema/reducers before announcing sync live.

## Publish
```
export PATH=$HOME/.local/bin:$HOME/.cargo/bin:$PATH
spacetime login            # interactive, as ael
cd spacetime && spacetime publish -s maincloud allthing -y
spacetime generate --lang typescript --out-dir ../web/stdb/module_bindings --module-path spacetimedb
```

## Schema
Public: `user(identity PK, fid idx, verified, online, last_seen)`; `user_key(identity PK, fid, x25519_pub[32], key_version, updated_at)`;
`room(id, name unique, created_by, created_at)` (`general` is seeded as id 1); `chat_message(id, room_id idx, sender, sender_fid, text, sent_at)`;
`typing(identity PK, room_id, expires_at)`.
Private: `dm_message(id, sender idx, recipient idx, sender_key_version, recipient_key_version, nonce[24], ciphertext, sent_at)`, `rate_limit`,
`notification_reader(identity PK, fid)` and `notification_state(fid PK, seen_at)` (Unix seconds; no notification content).
Per-user views: `my_dm_inbox` and `my_dm_outbox` show the caller's DMs only. That rule is enforced on the server. A test confirmed that `dm_message` cannot be queried by clients. `my_notification_state` returns zero or one row for the FID bound to the calling identity by `notification_reader`; unverified identities and public chat claims grant no access.

## Reducers
`register(fid)`, `set_public_key(x25519_pub)` (bumps key_version), `create_room(name)` ([a-z0-9-_], 1–64 chars),
`send_message(room_id, text)` (1–4000 bytes; also clears typing), `set_typing(room_id, bool)` (6 s TTL; also cleared on disconnect),
`send_dm(recipient, sender_kv, recipient_kv, nonce, ciphertext)` (24-byte nonce, 16 B–16 KiB ciphertext, recipient must have a key),
`delete_dm(id)` (sender or recipient). Chat writes are limited to 1 per 250 ms per identity. Presence is tracked through `user.online`, set on connect/disconnect.

Notification reducers are separate: `verify_notification_fid(fid, expiry, signature)` requires the same pinned, identity-bound Worker attestation even when chat's development fallback is unverified; it locks a nonzero FID to this notification identity without creating/updating a public chat user, key or presence. `mark_notifications_seen(seen_at)` derives the FID from that private binding and takes the maximum with its existing watermark; stale/equal values do not rewrite it. Negative times and times over 60 seconds ahead of the server are rejected; accepted near-future values clamp to server time. These reducers do not use chat's send-rate limiter.

## E2E design (the server sees only ciphertext)
- The client generates an X25519 keypair. The private key stays in IndexedDB (non-extractable where possible), and only the public key goes to `set_public_key`.
- Per message: shared = X25519(my_priv, their_pub), then key = HKDF-SHA256(shared, info="allthing-dm-v1"|sorted identities|key versions).
  Encrypt with XChaCha20-Poly1305 (`allthing-core::e2e` in Rust), a random 24-byte nonce, and AAD = sender|recipient|key versions.
- Both parties can decrypt (static-static DH), so the outbox can be read back. Key versions let clients pick the right key after a rotation.
- Future: X3DH/Double Ratchet for forward secrecy, plus multi-device keys (one key row per device).

## TODOs
1. ~~FID verification~~: done. `verify_fid(fid, expiry, signature)` checks the Worker's Ed25519 attestation of
   `allthing-spacetimedb-fid-v1|identity_hex|fid|expiry` against the key pinned in `src/attest_key.rs`. `cargo xtask attest-key` generates a replacement only when explicitly rotating it; pipe its private output directly to Wrangler as described in docs/DEPLOY.md#secrets. Only verified identities may speak once the key is pinned.
2. Pruning of expired typing rows (add a scheduled reducer; clients can filter by `expires_at` for now). Message retention/pagination.
3. Rooms are public, with no membership or moderation. Add `room_member` plus visibility views for private rooms.
4. RLS: views are used instead of `client_visibility_filter`. Users can still query the public `user_key`/`user` tables, which is intended.
5. Run wasm-opt for a smaller module.

## Client integration (Allthing web)
The UI and encryption logic are Rust. `web/stdb/bridge.ts` is the narrow adapter to the official TypeScript SDK; generated bindings live in `web/stdb/module_bindings`. The Rust SDK is not substituted because its BSL-1.1 license fails the repository's current license policy. `cargo xtask dist` bundles the adapter through the installed esbuild CLI and checks the pinned SDK version; the CSP codec adapter remains covered by wire-format and browser tests.

1. Use `npm ci` for the pinned SDK and regenerate bindings from the Rust module when its schema changes (see above).
2. The shared adapter connects to `wss://maincloud.spacetimedb.com`, database `allthing`. Messages keeps its per-account token and X25519 key in IndexedDB `allthing-stdb`, store `identities`; it does not reuse notification credentials.
3. After Farcaster sign-in, call `register`, obtain the Worker's identity-bound attestation, call `verify_fid`, then `set_public_key`.
4. Subscribe to `room`, `chat_message WHERE room_id=…`, `typing`, `user`, `user_key`, `my_dm_inbox`, `my_dm_outbox`.
5. Throttle `set_typing` (~every 3 s while typing). DMs are encrypted/decrypted entirely on the client.

### Private notification and Library client

`notification_sync.rs` uses the same SDK module script through `stdb_bridge`, but a separate connection and per-FID `{identity, token}` record in IndexedDB `allthing-notification-sync`, store `identities` (no DM key). After its delayed/API-quiet/browser-idle status capability check, it obtains a fresh Worker attestation, calls `verify_notification_fid` and subscribes to `SELECT * FROM my_notification_state`. When `spacetime.library` is advertised, the same private socket also follows caller Library metadata and bounded membership/page queries. Foreground Library/feed readiness skips the initial delay and unrelated API waits, while still yielding to browser idle time. It never calls `register` or `set_public_key` or subscribes to public chat/presence tables.

The existing localStorage notification watermark is the offline/legacy cache. Rust `ReadState` merges it with the initial private snapshot before uploading local progress, retains failed writes, and takes maxima on acknowledgements/remote updates without echoing them. One pending maximum is written at a time, coalesced after 200 ms; 30-second operation deadlines, retry backoff capped at five minutes and online/focus/visibility wakeups handle disconnects. Sign-out/account-generation changes fence stale responses and detach the old socket. The page reconciles its initial remote baseline once; later remote reads update its badge/cache while preserving that visit's row tint. Tab selection remains in the URL.


### Private Library

`library.rs` adds private `library_account` / `library_post` state, reusing `notification_reader` identity attestation. `my_library_state()` returns only caller metadata; `my_library_membership(hashes_csv)` accepts at most 100 canonical unique hashes and returns an explicit default row for absent membership; `my_library_page(kind,generation,after,limit)` uses bounded indexed ranges, at most 50 rows plus one continuation. Kind 0 is seen, kind 1 bookmarks; cursor is `<UnixSeconds>:<0xhash>`. SDK view rows are a set: sort the bounded result descending by time/hash and filter the exact caller, generation, kind and cursor before exposing it. No external SQL WHERE/ORDER/LIMIT belongs on parameterized views.

`mark_seen_batch` uses per-entry expected revisions and an account generation; `forget_seen` retains a revision tombstone. `set_bookmark` is compare-and-set with explicit false tombstones. `clear_seen` checks expected generation and an operation ID, making a lost-ack retry idempotent while fencing stale offline impressions. Clear does not remove bookmarks. Seen timestamps at or before `cleared_at` are intentionally ignored, including the same whole second. There is no history TTL or post-count eviction. These tables are never public; verified other FIDs cannot query a reader's view data.

Publish additively with the matching 2.13 CLI, `--delete-data=never`, before merging the Worker capability flag. Inspect the migration plan; preserve the pinned attestation key and existing notification/chat/DM schema and reducers. Never use a database clear or break-clients migration to ship Library.
