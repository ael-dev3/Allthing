#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Builds the static copy of Allthing for GitHub Pages into dist-pages/: the
# same Rust app, compiled with the `static-host` feature so the browser builds
# each page's payload itself (there is no Worker behind a static host).
# Run scripts/build.sh first: it installs the toolchain and wasm-bindgen.
# Rust owns optimizer discovery and all build logic.
#
#   bash scripts/build-pages.sh [base]   # base path, default /Allthing
set -euo pipefail
cd "$(dirname "$0")/.."

export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PWD/target/tools/bin:$PATH"
exec cargo xtask pages "${1:-/Allthing}"
