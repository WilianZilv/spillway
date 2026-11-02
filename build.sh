#!/usr/bin/env bash
# cargo wrapper; set CARGO_TARGET_DIR to build elsewhere (e.g. a faster filesystem).
set -e
source "$HOME/.cargo/env" 2>/dev/null || true
cd "$(dirname "$0")"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
cargo "$@"
