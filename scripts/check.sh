#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Checks run locally and in CI: formatting, lints for the native and
# WebAssembly targets, unit tests, and the dependency license audit.
set -eu
cd "$(dirname "$0")/.."

cargo fmt --all -- --check
cargo clippy --locked -p allthing-core -p xtask --all-targets -- -D warnings
cargo clippy --locked -p allthing-web -p allthing-worker --target wasm32-unknown-unknown -- -D warnings
cargo test --locked -p allthing-core -p allthing-web -p allthing-worker -p xtask
if command -v cargo-deny >/dev/null 2>&1; then
  cargo deny --log-level error check licenses
else
  echo "cargo-deny is not installed; skipping the license audit (cargo install cargo-deny)" >&2
fi
