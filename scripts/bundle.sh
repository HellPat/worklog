#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
cargo build --release
bundle="$PWD/target/Team-up.app"
mkdir -p "$bundle/Contents/MacOS"
cp target/release/team-up "$bundle/Contents/MacOS/team-up"
cp packaging/Info.plist "$bundle/Contents/Info.plist"
codesign --force --sign - "$bundle"
codesign --verify --strict "$bundle"
printf '%s\n' "$bundle"
