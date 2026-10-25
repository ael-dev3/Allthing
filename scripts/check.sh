#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Compatibility entry point; repository checks and validation run in Rust.
set -eu
cd "$(dirname "$0")/.."
exec cargo xtask check "$@"
