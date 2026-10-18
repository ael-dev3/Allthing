#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Checks run locally and in CI: formatting, lints for the native and
# WebAssembly targets, unit tests, and the dependency license audit.
set -eu
cd "$(dirname "$0")/.."

# One version: the workspace Cargo.toml; package.json must agree.
cargo_version="$(awk -F'"' '/^\[workspace.package\]/ {w=1} w && /^version/ {print $2; exit}' Cargo.toml)"
npm_version="$(awk -F'"' '/"version"/ {print $4; exit}' package.json)"
if [ "$cargo_version" != "$npm_version" ]; then
  echo "version mismatch: Cargo.toml $cargo_version, package.json $npm_version" >&2
  exit 1
fi

cargo fmt --all -- --check
cargo clippy --locked -p allthing-core -p xtask --all-targets -- -D warnings
cargo clippy --locked -p allthing-data -p allthing-web -p allthing-worker --target wasm32-unknown-unknown -- -D warnings
cargo clippy --locked -p allthing-web --features static-host --target wasm32-unknown-unknown -- -D warnings
cargo test --locked -p allthing-core -p allthing-data -p allthing-web -p allthing-worker -p xtask
if command -v cargo-deny >/dev/null 2>&1; then
  cargo deny --log-level error check licenses
else
  echo "cargo-deny is not installed; skipping the license audit (cargo install cargo-deny)" >&2
fi
