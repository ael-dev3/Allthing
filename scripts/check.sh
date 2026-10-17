#!/bin/sh
set -eu

# SSR and browser hydration are intentionally separate feature sets.
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo clippy -p allthing-web --target wasm32-unknown-unknown --no-default-features --features hydrate --locked -- -D warnings
cargo leptos build --cargo-locked
