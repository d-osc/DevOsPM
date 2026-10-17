#!/bin/bash
# Build release artifacts for tagging: static musl binaries via zig.
set -e
export PATH="$HOME/.local/zig:$PATH"
cd /mnt/c/Users/ondev/Projects/DevOsPM || exit 1
echo "== zig: $(zig version) =="
cargo zigbuild --release --locked --target x86_64-unknown-linux-musl
cargo zigbuild --release --locked --target aarch64-unknown-linux-musl
file target/x86_64-unknown-linux-musl/release/dev
file target/aarch64-unknown-linux-musl/release/dev
