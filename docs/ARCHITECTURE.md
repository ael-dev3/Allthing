# Architecture

## Implemented foundation

```text
Browser
  HTML + CSS immediately
  Leptos components hydrate as WASM
          |
          | generated read-only server function
          v
Axum / Tokio process
  Leptos SSR + static asset serving
  get_seed_feed -> fictional fixture provider
  GET /healthz -> { status: "ok", mode: "demo" }
          |
          v
allthing-core
  shared serializable view models
  source / channel / text queries and deterministic sorting
```

The `allthing-web` crate builds twice: native SSR with `ssr`, and `wasm32-unknown-unknown` with `hydrate`. `cargo-leptos` coordinates those builds and emits the loader, WASM, and CSS into `target/site`. The native `allthing-server` depends on the SSR build. No TypeScript application or Node runtime is required.

Feed data is loaded by a Leptos server function during SSR and serialized for hydration. Source, sort, channel, search, thread, and save controls are client-side Rust signals. Sample filters operate on the complete small fixture; production pagination must move querying to an indexed backend.

The core contains no network access or credentials. Identifiers such as `demo-reader` and relative ages are presentation fixtures, not protocol identifiers or timestamps. The seed is not an implementation of protocol signature verification, ingestion, or deduplication.

## Planned data boundary

```text
Snapchain read adapter ----\
                           > verify -> normalize -> reconcile -> index/cache -> feed API
Hypersnap read adapter ----/
```

Start with independent read adapters behind a shared interface. Return typed errors, provenance, upstream cursors, and freshness. Bound upstream requests, result sizes, retries, and timeouts. Avoid making the browser contact arbitrary reader-supplied endpoints through the server.

### Identity and overlap

- Reconcile verified author FID and canonical message identity, not matching text.
- Preserve observed sources and source-specific snapshots.
- Do not sum likes, recasts, or replies from overlapping sources.
- Distinguish message deletion, unavailable data, provider filtering, and client filtering.
- Handle ordering, pagination, removals, historical coverage, and divergent snapshots deliberately.
- Reconstruct reply trees with bounded depth and missing-parent handling.

The seed keeps metrics separate and uses the Snapchain snapshot for display when present, otherwise the first available snapshot. This is a deterministic UI rule, not a statement that one source is authoritative. The UI does not yet expose a snapshot selector.

### Transport and storage

Use supported Snapchain/Hypersnap APIs or RPC clients once their capabilities and licenses are checked. A Rust indexer with persistent storage can later serve normalized feeds and thread queries. Choose the storage engine after measuring the read paths; a database is unnecessary for running this seed.

Do not copy third-party protocol schemas into the MIT tree without a license review. A project's availability on GitHub does not imply MIT-compatible redistribution.

## Planned authentication boundary

```text
QR challenge or compatible account proof
  -> validate domain, nonce, expiry, signature, and current FID authority
  -> application session
  -> separate, explicit signer authorization if the user wants to write
```

Farcaster QR sign-in authenticates an identity; it does not by itself grant cast-writing authority. An arbitrary verified wallet must not be treated as custody or signer authority. Sessions require secure, HttpOnly, appropriately scoped cookies, CSRF defenses, revocation, and origin checks before production.

Hypersnap authentication and signer capabilities are evolving. Evaluate current unified signer APIs, including off-chain/gasless keys where supported. Do not rely exclusively on legacy on-chain-only lookups or assume placeholder login endpoints are usable. Keep independent authentication adapters capability-driven and fail closed.

No authentication endpoint, signer store, key material, or publication path is included in the seed.

## Deployment and security work still needed

- TLS termination, production response headers, and a tested CSP compatible with the actual loader.
- Rate limits, request-size limits, bounded ingestion and rendering, and safe media proxying if added.
- Configuration validation, graceful provider failures, structured logs with redaction, and metrics without personal data.
- Persistent storage, migrations, backup/restore tests, and signer encryption only when those components exist.
- Session and permission tests before enabling any write actions.
- Separate native/WASM dependency checks and upstream license review.

The local server binds to loopback by default and has no privileged actions. Deployment configuration, hosted services, CI credentials, and personal development settings do not belong in source control.

## Primary references

API details must be checked again when implementing adapters; these are research starting points rather than frozen contracts.

- [Farcaster documentation](https://docs.farcaster.xyz/)
- [Farcaster AuthKit source](https://github.com/farcasterxyz/auth-kit)
- [Snapchain source](https://github.com/farcasterxyz/snapchain)
- [Hypersnap source and RPC definitions](https://github.com/farcasterorg/hypersnap)
- [Leptos](https://leptos.dev/)
- [cargo-leptos](https://github.com/leptos-rs/cargo-leptos)
- [Axum](https://docs.rs/axum/)
