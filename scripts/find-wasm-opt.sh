#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Find the optimizer worker-build downloaded. Its dirs_next cache is
# ~/Library/Caches on macOS, even when XDG_CACHE_HOME is set. Optional
# arguments are cache roots to search (also useful for isolated tests).
set -euo pipefail

if [ -n "${WASM_OPT_BIN:-}" ]; then
  printf '%s\n' "$WASM_OPT_BIN"
  exit 0
fi
if command -v wasm-opt >/dev/null 2>&1; then
  command -v wasm-opt
  exit 0
fi

# Match worker-build's Binaryen archive names; don't pick an optimizer
# cached for a different architecture on a shared or migrated machine.
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64|Darwin-aarch64) target=arm64-macos ;;
  Darwin-x86_64) target=x86_64-macos ;;
  Linux-aarch64|Linux-arm64) target=aarch64-linux ;;
  Linux-x86_64) target=x86_64-linux ;;
  *) exit 0 ;;
esac
if [ "$#" -eq 0 ]; then
  set -- "${XDG_CACHE_HOME:-$HOME/.cache}" "$HOME/Library/Caches" "${TMPDIR:-/tmp}"
fi
for cache_root in "$@"; do
  for candidate in "$cache_root"/worker-build/wasm-opt-"$target"-*/bin/wasm-opt; do
    if [ -x "$candidate" ]; then
      printf '%s\n' "$candidate"
      exit 0
    fi
  done
done
