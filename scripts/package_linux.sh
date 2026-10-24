#!/bin/sh
# Build .deb and .rpm packages (amd64/x86_64 and arm64/aarch64) from the static
# musl binaries produced by scripts/build_release.sh. Requires nfpm
# (https://nfpm.goreleaser.com/install/) on PATH.
#
# Usage: sh scripts/package_linux.sh [output-dir [amd64|arm64]]
#   output-dir defaults to dist/packages. The optional architecture limits the
#   build to one binary (CI builds each architecture on its own runner).
set -eu

project=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
out=${1:-"$project/dist/packages"}
only=${2:-}

if ! command -v nfpm >/dev/null 2>&1; then
    printf 'nfpm not found. Install it: https://nfpm.goreleaser.com/install/\n' >&2
    exit 1
fi

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$project/Cargo.toml" | head -n 1)
[ -n "$version" ] || { printf 'Cannot read version from Cargo.toml\n' >&2; exit 1; }

mkdir -p -- "$out"
cd "$project"

# "<nfpm arch>:<rust target arch>"
for pair in "amd64:x86_64" "arm64:aarch64"; do
    arch=${pair%%:*}
    target=${pair##*:}
    if [ -n "$only" ] && [ "$only" != "$arch" ]; then
        continue
    fi
    bin=$project/target/$target-unknown-linux-musl/release/dev
    if [ ! -f "$bin" ]; then
        printf 'Missing %s (run scripts/build_release.sh first)\n' "$bin" >&2
        exit 1
    fi
    # nfpm.yaml reads the binary from a fixed path.
    mkdir -p dist/stage
    install -m 755 -- "$bin" dist/stage/dev
    for format in deb rpm; do
        ARCH=$arch VERSION=$version \
            nfpm package --config packaging/nfpm.yaml --packager "$format" --target "$out/"
    done
done

ls -la "$out"
