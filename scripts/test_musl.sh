#!/bin/bash
# Run the test suite and smoke checks against the static musl build.
set -e
export PATH="$HOME/.local/zig:$PATH"
cd /mnt/c/Users/ondev/Projects/DevOsPM || exit 1
cargo zigbuild --locked --target aarch64-unknown-linux-musl --tests
BIN=$(ls -t target/aarch64-unknown-linux-musl/debug/deps/dev-* | grep -v '\.d$' | head -1)
"$BIN" --test-threads=1
python3 scripts/smoke_linux.py target/aarch64-unknown-linux-musl/release/dev | tail -1
python3 scripts/smoke_download.py target/aarch64-unknown-linux-musl/release/dev | tail -1
