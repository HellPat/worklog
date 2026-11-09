#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
npm ci --prefix spikes/gpui-recorder/tools
mkdir -p spikes/gpui-recorder/evidence
cargo run --locked --example spike_audio_fixture -- fixtures/asr/spoken.wav spikes/gpui-recorder/evidence/audio-transcript.json
cd spikes/gpui-recorder
export SPIKE_RUN="${SPIKE_RUN:-desktop-$(date +%Y%m%d-%H%M%S)}"
export CLANG_MODULE_CACHE_PATH="${CLANG_MODULE_CACHE_PATH:-${TMPDIR:-/tmp}/worklog-clang-cache}"
cargo test --locked --test ui -- --fail-fast
