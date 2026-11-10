#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
export CLANG_MODULE_CACHE_PATH="${CLANG_MODULE_CACHE_PATH:-${TMPDIR:-/tmp}/worklog-clang-cache}"
for formula in Formula/*.rb; do
  test ! -f "$formula" || ruby -c "$formula"
done
cargo fmt --all --check
cargo fmt --manifest-path spikes/gpui-recorder/Cargo.toml --all --check
cargo build --locked --release --manifest-path spikes/gpui-recorder/Cargo.toml
cargo test --locked --all-targets
cargo test --locked --release --manifest-path spikes/gpui-recorder/Cargo.toml --lib --test core --test reader --test cli
if test "${WORKLOG_DESKTOP_TESTS:-0}" = 1; then
  ./scripts/test-desktop.sh
fi
