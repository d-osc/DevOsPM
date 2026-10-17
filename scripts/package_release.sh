#!/bin/bash
# Assemble the GitHub Release payload in /tmp/devospm-release for every
# supported architecture (static musl via scripts/build_release.sh).
set -e
cd /mnt/c/Users/ondev/Projects/DevOsPM || exit 1
OUT=/tmp/devospm-release
rm -rf "$OUT" && mkdir -p "$OUT"

# Example packages, built with the native-arch musl binary; pack output is
# architecture-independent, so the x86_64 artifact stays the shipped binary.
DEV=target/aarch64-unknown-linux-musl/release/dev
"$DEV" pack ./examples/hello --output "$OUT/hello-1.0.0.dpk"
"$DEV" pack ./examples/hello --output "$OUT/hello-1.0.0.tar.gz"
"$DEV" pack ./examples/greeting-extension --output "$OUT/greeting-extension-1.0.0.dpk"

# Binary bundles: dev + installer + docs, flat like previous releases.
# Deterministic archives: fixed mtime/order/owner so checksums are stable.
for PAIR in "x86_64:x86_64" "arm64:aarch64"; do
    NAME="${PAIR%%:*}"
    TARGET="${PAIR##*:}"
    cp "target/$TARGET-unknown-linux-musl/release/dev" "$OUT/dev-linux-$NAME"
    STAGE=$(mktemp -d)
    cp "target/$TARGET-unknown-linux-musl/release/dev" "$STAGE/dev"
    cp scripts/install.sh "$STAGE/install.sh"
    cp LICENSE "$STAGE/LICENSE"
    cp README.md "$STAGE/README.md"
    touch -d '@0' "$STAGE/dev" "$STAGE/install.sh" "$STAGE/LICENSE" "$STAGE/README.md"
    tar --sort=name --mtime='@0' --owner=0 --group=0 --numeric-owner \
        -cf - -C "$STAGE" dev install.sh LICENSE README.md \
        | gzip -n > "$OUT/dev-linux-$NAME.tar.gz"
    rm -rf "$STAGE"
done

cd "$OUT"
sha256sum dev-linux-x86_64 dev-linux-x86_64.tar.gz dev-linux-arm64 dev-linux-arm64.tar.gz \
    hello-1.0.0.dpk hello-1.0.0.tar.gz greeting-extension-1.0.0.dpk > SHA256SUMS
ls -la
echo ---
file dev-linux-x86_64 dev-linux-arm64 | cut -d: -f1-3
cat SHA256SUMS
