# Allthing

**A place for the conversation.** An independent Farcaster web client with a compact, light-mode interface inspired by classic Reddit.

Allthing is being built entirely in Rust: **Leptos** for server-rendered pages and browser **WebAssembly**, **Axum** for HTTP, and **Tokio** for the native runtime. The goal is a readable home for conversations available through **Snapchain** and **Hypersnap**, with visible source information and user-controlled feeds.

## This is a seed

The application runs today with **fictional sample content**. It demonstrates the architecture and interaction model; it is not connected to either live network.

| Available now | Planned |
| --- | --- |
| Server-rendered feed and About page | Snapchain and Hypersnap read adapters |
| Rust/WASM source, channel, search, and sort controls | Verified identity and overlap reconciliation |
| Expandable discussions and nested, collapsible replies | Farcaster QR sign-in and a compatible independent login path |
| Temporary in-memory saves | Explicit, revocable permission to publish |
| Per-source sample metrics and a health endpoint | Persistent preferences, mutes, blocks, and deployment hardening |

Every displayed handle, conversation, age, and count is made up. Saves are temporary component state: they reset on reload or when a post leaves the filtered view. No account access, wallet connection, signer registration, tracking service, or upstream API key is needed.

## Run locally

Install [Rust through rustup](https://rustup.rs/) and [cargo-leptos](https://github.com/leptos-rs/cargo-leptos):

```sh
cargo install cargo-leptos --version 0.3.11 --locked
cargo leptos watch
```

Open **http://127.0.0.1:3000**. The checked-in toolchain file pins Rust and includes the WASM target, rustfmt, and Clippy. On first use, rustup installs those components. No Node.js, npm, JavaScript framework, database, or secret configuration is required.

For a compiled build without the watcher:

```sh
cargo leptos build --cargo-locked
LEPTOS_OUTPUT_NAME=allthing \
LEPTOS_SITE_ROOT=target/site \
LEPTOS_SITE_PKG_DIR=pkg \
LEPTOS_SITE_ADDR=127.0.0.1:3000 \
./target/debug/allthing-server
```

The server needs both its native binary and the generated `target/site` directory. For an optimized build, use `cargo leptos build --release --cargo-locked` and `target/release/allthing-server`. Bind beyond loopback only with an appropriate production configuration and security review.

## Workspace

```text
crates/
  allthing-core/     Shared models, deterministic feed queries, fictional fixtures
  allthing-web/      Leptos components, server function, browser hydration entry
  allthing-server/   Axum routes, static assets, native process and health endpoint
style/              Plain CSS; light mode and responsive layouts
public/             Original static assets
docs/               Product direction, architecture, implementation roadmap
scripts/check.sh    Local validation for both compilation targets
```

The browser and server compile separately. Do not use `--all-features`: the `ssr` and `hydrate` features are intentionally mutually exclusive. Leptos generates a small JavaScript loader to initialize WebAssembly; all authored application behavior is Rust.

```sh
./scripts/check.sh
curl http://127.0.0.1:3000/healthz
# {"status":"ok","mode":"demo"}
```

## Direction

- **Read first.** Chronological feeds, compact lists, and threads that are easy to follow.
- **Show origins.** Snapchain and Hypersnap are overlapping sources, not automatically two disjoint social networks. Reconcile verified post identities and retain provenance.
- **Keep meaning.** Likes and recasts retain their native meaning. Client-specific features must not pretend to be protocol-wide actions.
- **Ask for permission clearly.** Proving identity and authorizing publication are separate operations.
- **Let people choose.** Source selection and transparent user controls should be explicit.

See the [product brief](docs/PRODUCT.md), [architecture](docs/ARCHITECTURE.md), and [roadmap](docs/ROADMAP.md). Allthing is independent and is not affiliated with or endorsed by Farcaster or its operators. The name nods to the *Hyperion Cantos*; no book text or artwork is included.

## License

Original Allthing code, documentation, and artwork are available under the [MIT license](LICENSE). Dependencies retain their own licenses. Future protocol schemas and third-party assets must be reviewed and retain the notices required by their licenses.
