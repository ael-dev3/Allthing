#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Cloudflare Workers Builds starts without Rust. Bootstrap the pinned
# toolchain, then let the native xtask install tools and build both targets.
set -euo pipefail
cd "$(dirname "$0")/.."

export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
if ! command -v rustup >/dev/null 2>&1; then
  curl --proto '=https' --proto-redir '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
fi
rustup toolchain install >&2
cargo --version >&2
exec cargo xtask build "$@"
