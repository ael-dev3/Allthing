<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Allthing SpacetimeDB module — handoff

Module: `spacetime/spacetimedb` in this repository (Rust, spacetimedb 2.13). Builds with `spacetime build`; `sh spacetime/check.sh` runs its checks (CI's `SpacetimeDB module` job).
Target DB name: **`allthing`** (fall back to `allthing-chat` if the name is taken) on maincloud.
URL once published: `https://maincloud.spacetimedb.com` / ws, database `allthing`. Dashboard: https://spacetimedb.com/allthing
STATUS: published to maincloud as `allthing` (checked 21 October 2026: `GET /v1/database/allthing` answers, and its schema has
these tables and reducers, with `general` seeded as room 1).

## Publish
```
export PATH=$HOME/.local/bin:$HOME/.cargo/bin:$PATH
spacetime login            # interactive, as ael
cd spacetime && spacetime publish -s maincloud allthing -y
spacetime generate --lang typescript --out-dir <allthing-web>/src/module_bindings --module-path spacetimedb
```

## Schema
Public: `user(identity PK, fid idx, verified, online, last_seen)`; `user_key(identity PK, fid, x25519_pub[32], key_version, updated_at)`;
`room(id, name unique, created_by, created_at)` (`general` is seeded as id 1); `chat_message(id, room_id idx, sender, sender_fid, text, sent_at)`;
`typing(identity PK, room_id, expires_at)`.
Private: `dm_message(id, sender idx, recipient idx, sender_key_version, recipient_key_version, nonce[24], ciphertext, sent_at)`, `rate_limit`.
Per-user views: `my_dm_inbox` and `my_dm_outbox` show the caller's DMs only. That rule is enforced on the server. A test confirmed that `dm_message` cannot be queried by clients.

## Reducers
`register(fid)`, `set_public_key(x25519_pub)` (bumps key_version), `create_room(name)` ([a-z0-9-_], 1–64 chars),
`send_message(room_id, text)` (1–4000 bytes; also clears typing), `set_typing(room_id, bool)` (6 s TTL; also cleared on disconnect),
`send_dm(recipient, sender_kv, recipient_kv, nonce, ciphertext)` (24-byte nonce, 16 B–16 KiB ciphertext, recipient must have a key),
`delete_dm(id)` (sender or recipient). Rate limit: 1 write per 250 ms per identity. Presence is tracked through `user.online`, set on connect/disconnect.

## E2E design (the server sees only ciphertext)
- The client generates an X25519 keypair. The private key stays in IndexedDB (non-extractable where possible), and only the public key goes to `set_public_key`.
- Per message: shared = X25519(my_priv, their_pub), then key = HKDF-SHA256(shared, info="allthing-dm-v1"|sorted identities|key versions).
  Encrypt with XChaCha20-Poly1305 (libsodium / @noble/ciphers), a random 24-byte nonce, and AAD = sender|recipient|key versions.
- Both parties can decrypt (static-static DH), so the outbox can be read back. Key versions let clients pick the right key after a rotation.
- Future: X3DH/Double Ratchet for forward secrecy, plus multi-device keys (one key row per device).

## TODOs
1. **FID verification**: `register` currently trusts the FID the client sends (`verified=false`). Plan: Allthing's backend runs SIWF, then signs
   Ed25519(identity_hex|fid|expiry) with a pinned key. A new `verify_fid(attestation)` reducer checks it in-module (ed25519-dalek) and sets verified=true.
   Until that's done, the UI should treat FIDs as unverified.
2. Pruning of expired typing rows (add a scheduled reducer; clients can filter by `expires_at` for now). Message retention/pagination.
3. Rooms are public, with no membership or moderation. Add `room_member` plus visibility views for private rooms.
4. RLS: views are used instead of `client_visibility_filter`. Users can still query the public `user_key`/`user` tables, which is intended.
5. Run wasm-opt for a smaller module.

## Client integration (Allthing web)
1. `npm i spacetimedb`, then generate TS bindings (see above).
2. `DbConnection.builder().withUri('wss://maincloud.spacetimedb.com').withModuleName('allthing').withToken(localStorage token)`.
   Persist the token that comes back in onConnect.
3. After Farcaster sign-in, call `register(fid)` (later `verify_fid`), then `set_public_key`.
4. Subscribe to `room`, `chat_message WHERE room_id=…`, `typing`, `user`, `user_key`, `my_dm_inbox`, `my_dm_outbox`.
5. Throttle `set_typing` (~every 3 s while typing). DMs are encrypted/decrypted entirely on the client.
