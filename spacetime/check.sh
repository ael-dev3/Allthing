#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Checks for the SpacetimeDB module (spacetime/spacetimedb), which is its own
# crate outside the workspace: formatting, Clippy, unit tests, a release
# build for wasm32 (what `spacetime publish` uploads) and the license audit.
set -eu
cd "$(dirname "$0")/spacetimedb"
# The workspace's wasm32 rustflags (.cargo/config.toml) are for the browser
# app; the module builds without them, as `spacetime build` would elsewhere.
export RUSTFLAGS=""
cargo fmt -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --target wasm32-unknown-unknown -- -D warnings
cargo test --locked
cargo build --locked --release --target wasm32-unknown-unknown
if command -v cargo-deny >/dev/null 2>&1; then
  # cargo-deny finds the repository's deny.toml in a parent directory.
  cargo deny --log-level error check licenses
else
  echo "cargo-deny is not installed; skipping the license audit" >&2
fi
