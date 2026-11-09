#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
git config core.hooksPath hooks
printf '%s\n' 'Installed hooks/pre-commit (build + tests).'
