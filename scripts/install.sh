#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
bundle="$PWD/target/Team-up.app"
test -x "$bundle/Contents/MacOS/team-up"
codesign --verify --strict "$bundle"
install_directory="${1:-$HOME/Applications}"
mkdir -p "$install_directory"
staging="$install_directory/.Team-up-install-$$.app"
ditto "$bundle" "$staging"
codesign --verify --strict "$staging"
if test -e "$install_directory/Team-up.app"; then
    previous="$HOME/Library/Application Support/team-up/previous-builds"
    mkdir -p "$previous"
    mv "$install_directory/Team-up.app" "$previous/Team-up-$(date +%Y%m%d-%H%M%S).app"
fi
mv "$staging" "$install_directory/Team-up.app"
printf '%s\n' "$install_directory/Team-up.app"
open "$install_directory/Team-up.app"
