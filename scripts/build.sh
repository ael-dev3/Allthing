#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Builds everything Cloudflare serves for Allthing:
#   - crates/allthing-worker/build/  the Worker (Rust, via worker-build)
#   - dist/                          static assets: the WebAssembly app, styles,
#                                    fonts and the spam-label shards
#
# Cloudflare Workers Builds runs this as the build command (its image has no
# Rust, so this installs it); `npx wrangler deploy` then uploads both. It
# works the same on a laptop.
#
#   bash scripts/build.sh                # everything
#   SKIP_LABELS=1 bash scripts/build.sh  # skip the ~125 MB label download
set -euo pipefail
cd "$(dirname "$0")/.."

log() { printf '\n==> %s\n' "$*" >&2; }
version_of() { awk -v name="$1" '$0 == "name = \"" name "\"" { getline; gsub(/version = |"/, ""); print; exit }' Cargo.lock; }

# 1. Rust, at the version and with the wasm32 target pinned in rust-toolchain.toml.
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v rustup >/dev/null 2>&1; then
  log "Installing rustup"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
fi
rustup toolchain install >&2
cargo --version >&2

TOOLS="$PWD/target/tools"
mkdir -p "$TOOLS/bin"
export PATH="$TOOLS/bin:$PATH"
platform="$(uname -s)-$(uname -m)"

# 2. wasm-bindgen CLI matching the wasm-bindgen crate in Cargo.lock.
WBG_VERSION="$(version_of wasm-bindgen)"
if ! wasm-bindgen --version 2>/dev/null | grep -q " $WBG_VERSION\$"; then
  log "Installing wasm-bindgen-cli $WBG_VERSION"
  release="wasm-bindgen-$WBG_VERSION-x86_64-unknown-linux-musl"
  if [ "$platform" = "Linux-x86_64" ] &&
    curl -fsSL "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/$WBG_VERSION/$release.tar.gz" | tar -xz -C "$TOOLS"; then
    cp "$TOOLS/$release/wasm-bindgen" "$TOOLS/bin/"
  else
    cargo install wasm-bindgen-cli --version "$WBG_VERSION" --locked --root "$TOOLS"
  fi
fi
WASM_BINDGEN_BIN="$(command -v wasm-bindgen)"
export WASM_BINDGEN_BIN

# 3. worker-build, matching the worker crate.
WB_VERSION="$(version_of worker)"
if ! worker-build --version 2>/dev/null | grep -q "^$WB_VERSION\$"; then
  log "Installing worker-build $WB_VERSION"
  cargo install worker-build --version "$WB_VERSION" --locked --root "$TOOLS"
fi

# Version stamp: the workspace version is in Cargo.toml; the commit comes
# from Workers Builds or the local checkout.
ALLTHING_COMMIT="$(printf '%s' "${WORKERS_CI_COMMIT_SHA:-$(git rev-parse HEAD 2>/dev/null || echo dev)}" | cut -c1-7)"
export ALLTHING_COMMIT

# 4. The Worker. worker-build also fetches esbuild and wasm-opt.
log "Building the Worker"
(cd crates/allthing-worker && worker-build --release)

# 5. The browser app, optimized with the wasm-opt worker-build fetched.
if [ -z "${WASM_OPT_BIN:-}" ]; then
  WASM_OPT_BIN="$(ls "${XDG_CACHE_HOME:-$HOME/.cache}"/worker-build/wasm-opt-*/bin/wasm-opt 2>/dev/null | head -n 1 || true)"
fi
export WASM_OPT_BIN
log "Building the browser app"
cargo xtask dist

# 6. Spam labels from the Farcaster app's public dataset (Git LFS). Optional:
#    without it, profiles say the label data is unavailable.
if [ -z "${SKIP_LABELS:-}" ]; then
  log "Building spam-label shards"
  mkdir -p target/labels
  oid="$(curl -fsSL https://raw.githubusercontent.com/warpcast/labels/main/spam.jsonl 2>/dev/null | awk '/^oid sha256:/ { sub(/^oid sha256:/, ""); print }' || true)"
  file="target/labels/spam-${oid:-unknown}.jsonl"
  version="lfs:${oid:0:12}"
  # Unchanged upstream? Copy the shards the live site already serves (a few
  # MB of free static assets) instead of downloading the dataset again.
  live="${ALLTHING_LIVE_URL:-https://allthing.social}/data/labels/spam"
  if [ -n "$oid" ] && [ ! -s "$file" ] && meta="$(curl -fsSL "$live/meta.json" 2>/dev/null)" &&
    shards="$(printf '%s' "$meta" | python3 -c 'import json,sys; m=json.load(sys.stdin); print(m["shards"] if m.get("version")==sys.argv[1] else "")' "$version")" &&
    [ -n "$shards" ]; then
    out=dist/data/labels/spam
    rm -rf "$out" && mkdir -p "$out"
    copied=yes
    for ((i = 0; i < shards; i++)); do
      name="$(printf '%04d.txt' "$i")"
      curl -fsSL --retry 2 -o "$out/$name" "$live/$name" || { copied=no; break; }
    done
    if [ "$copied" = yes ]; then
      printf '%s' "$meta" >"$out/meta.json"
      log "Reused $shards label shards ($version) from the live site"
      SKIP_DOWNLOAD=1
    else
      rm -rf "$out"
    fi
  fi
  if [ -z "${SKIP_DOWNLOAD:-}" ] && [ -n "$oid" ] && [ ! -s "$file" ]; then
    if ! curl -fsSL --retry 3 -o "$file.part" "https://media.githubusercontent.com/media/warpcast/labels/main/spam.jsonl"; then
      rm -rf target/labels/repo
      git clone --quiet --depth 1 https://github.com/warpcast/labels target/labels/repo &&
        git -C target/labels/repo lfs pull --include spam.jsonl &&
        cp target/labels/repo/spam.jsonl "$file.part" || true
    fi
    if [ -s "$file.part" ] && [ "$(sha256sum "$file.part" | cut -d' ' -f1)" = "$oid" ]; then
      mv "$file.part" "$file"
    else
      rm -f "$file.part"
    fi
  fi
  if [ -n "${SKIP_DOWNLOAD:-}" ]; then
    :
  elif [ -s "$file" ]; then
    cargo xtask labels "$file" "$version"
  else
    echo "warning: could not download the spam-label dataset; profiles will show labels as unavailable" >&2
  fi
fi

log "Built dist/ and crates/allthing-worker/build/"
