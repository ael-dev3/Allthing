#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Compatibility entry point; Rust validates the isolated SpacetimeDB module.
set -eu
cd "$(dirname "$0")/.."
exec cargo xtask module-check "$@"
