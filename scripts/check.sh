#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
export CLANG_MODULE_CACHE_PATH="${CLANG_MODULE_CACHE_PATH:-${TMPDIR:-/tmp}/worklog-clang-cache}"
cargo fmt --all --check
cargo fmt --manifest-path spikes/gpui-recorder/Cargo.toml --all --check
cargo build --locked --release --manifest-path spikes/gpui-recorder/Cargo.toml
cargo test --locked --all-targets
cargo test --locked --manifest-path spikes/gpui-recorder/Cargo.toml --test core
if test "${WORKLOG_DESKTOP_TESTS:-0}" = 1; then
  ./scripts/test-desktop.sh
fi
