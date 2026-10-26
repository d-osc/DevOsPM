#!/bin/sh
# End-to-end smoke test for the DevLang port of dev (mirrors the Rust build's
# scripts/smoke_linux.py happy path plus rollback/extension coverage).
set -e
DEVDIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)"
cd "$DEVDIR"

BASE=/tmp/devospm-devlang-smoke
rm -rf "$BASE"
mkdir -p "$BASE/data" "$BASE/bin" "$BASE/work"

export DEV_ROOT="$BASE/root"
export XDG_DATA_HOME="$BASE/data"
export DEV_BIN_DIR="$BASE/bin"
export DEV_LAUNCHER="$BASE/bin/dev"
# fake launcher so shims/desktop entries have something to point at
printf '#!/bin/sh\nexec d %s/main.dev -- "$@"\n' "$DEVDIR" > "$DEV_LAUNCHER"
chmod 755 "$DEV_LAUNCHER"

run() {
  echo "\$ dev $*"
  d "$DEVDIR/main.dev" -- "$@"
}

echo "== pack =="
mkdir -p "$BASE/work/hello/bin"
cat > "$BASE/work/hello/package.json" <<'EOF'
{"name":"hello","version":"1.0.0","description":"test package","bin":{"hello":"bin/hello"}}
EOF
printf '#!/bin/sh\necho "hello $1"\n' > "$BASE/work/hello/bin/hello"
chmod 755 "$BASE/work/hello/bin/hello"
(cd "$BASE/work" && d "$DEVDIR/main.dev" -- pack hello)
ls -la "$BASE/work" | grep dpk

echo "== install =="
d "$DEVDIR/main.dev" -- install "$BASE/work/hello-1.0.0.dpk"

echo "== list =="
d "$DEVDIR/main.dev" -- list
d "$DEVDIR/main.dev" -- list --json | head -3

echo "== info =="
d "$DEVDIR/main.dev" -- info hello | head -20

echo "== verify =="
d "$DEVDIR/main.dev" -- verify

echo "== run =="
d "$DEVDIR/main.dev" -- run hello world

echo "== shim =="
ls -la "$BASE/bin" | grep hello
"$BASE/bin/hello" shimarg

echo "== desktop entry =="
cat "$BASE/data/applications/dev-hello.desktop"

echo "== upgrade with --replace =="
mkdir -p "$BASE/work/hello2/bin"
cat > "$BASE/work/hello2/package.json" <<'EOF'
{"name":"hello","version":"2.0.0","description":"test package","bin":{"hello":"bin/hello","hi":"bin/hello"}}
EOF
printf '#!/bin/sh\necho "hello v2 $1"\n' > "$BASE/work/hello2/bin/hello"
chmod 755 "$BASE/work/hello2/bin/hello"
(cd "$BASE/work/hello2" && tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner -czf hello-2.0.0.dpk package.json bin/hello)
d "$DEVDIR/main.dev" -- install "$BASE/work/hello2/hello-2.0.0.dpk" --replace
d "$DEVDIR/main.dev" -- run hello world

echo "== rollback =="
d "$DEVDIR/main.dev" -- rollback hello
d "$DEVDIR/main.dev" -- run hello world

echo "== extension =="
mkdir -p "$BASE/work/ext/extension/dist"
cat > "$BASE/work/ext/extension/extension.json" <<'EOF'
{"id":"org.example.status","name":"Status","description":"Example status extension","version":"1.0.0","apiVersion":1,"settingsVersion":1,"enabledByDefault":false,"settings":[{"id":"general","fields":[{"key":"enabled","type":"toggle"}]}]}
EOF
printf 'console.log("hi")\n' > "$BASE/work/ext/extension/dist/app.js"
(cd "$BASE/work/ext" && tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner -czf ext.tar.gz extension)
d "$DEVDIR/main.dev" -- install "$BASE/work/ext/ext.tar.gz"
d "$DEVDIR/main.dev" -- list
d "$DEVDIR/main.dev" -- verify
ls -la "$BASE/data/dev-os/extensions/" | head -5
readlink "$BASE/data/dev-os/extensions/org.example.status"
cat "$BASE/data/dev-os/extension-store/releases/org.example.status"/*/store-install.json

echo "== remove extension with --purge =="
d "$DEVDIR/main.dev" -- remove org.example.status --purge
d "$DEVDIR/main.dev" -- list

echo "== tamper detection =="
GEN=$(d "$DEVDIR/main.dev" -- info hello | grep '"directory"' | head -1 | tr -d ' ",' | cut -d: -f2)
printf 'tampered\n' >> "$BASE/root/packages/$GEN/bin/hello"
d "$DEVDIR/main.dev" -- verify hello && echo "UNEXPECTED: verify passed" || echo "verify correctly failed"

echo "== remove package =="
d "$DEVDIR/main.dev" -- remove hello
d "$DEVDIR/main.dev" -- list
ls "$BASE/root/packages" | wc -l

echo "ALL SMOKE STEPS COMPLETED"
