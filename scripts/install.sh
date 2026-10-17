#!/bin/sh
set -eu

project=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
destination=${DEV_BIN_DIR:-"$HOME/.local/bin"}

if [ "$#" -gt 1 ]; then
    printf 'Usage: sh scripts/install.sh [path/to/dev]\n' >&2
    exit 2
fi

if [ "$#" -eq 1 ]; then
    binary=$1
elif [ -f "$project/dist/linux-x86_64/dev" ]; then
    binary=$project/dist/linux-x86_64/dev
else
    (cd "$project" && cargo build --release --locked)
    binary=${CARGO_TARGET_DIR:-"$project/target"}/release/dev
fi

if [ ! -f "$binary" ]; then
    printf 'Binary not found: %s\n' "$binary" >&2
    exit 1
fi

mkdir -p -- "$destination"
install -m 755 -- "$binary" "$destination/dev"
printf 'Installed: %s/dev\n' "$destination"
printf 'Add to your shell profile if needed: export PATH="%s:$PATH"\n' "$destination"
"$destination/dev" --version
