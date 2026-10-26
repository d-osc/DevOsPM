#!/bin/sh
# Install the DevLang build of dev into ~/.local (launcher on PATH, sources
# under ~/.local/share/devos/dev-cli/current so `dev self-update` can swap
# them). Requires the DevLang runtime `d` on PATH.
set -e

DEVLANG_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)"
if [ ! -f "$DEVLANG_DIR/main.dev" ]; then
    echo "dev: cannot locate main.dev next to install.sh ($DEVLANG_DIR)" >&2
    exit 1
fi
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
CLI_ROOT="$DATA_HOME/devos/dev-cli"
BIN_DIR="${DEV_BIN_DIR:-$HOME/.local/bin}"

command -v d >/dev/null 2>&1 || {
    echo "dev: the DevLang runtime 'd' was not found on PATH" >&2
    echo "dev: install DevLang first: https://github.com/d-osc/DevLang" >&2
    exit 1
}

mkdir -p "$CLI_ROOT/versions" "$BIN_DIR"
TARGET="$CLI_ROOT/versions/port-$(date +%Y%m%d%H%M%S)"
rm -rf "$TARGET"
cp -r "$DEVLANG_DIR" "$TARGET"
rm -rf "$TARGET/.staging-"* 2>/dev/null || true

# Point `current` at the new tree through a temporary symlink.
TEMP_LINK="$CLI_ROOT/.current-$$"
ln -s "$TARGET" "$TEMP_LINK"
mv -T "$TEMP_LINK" "$CLI_ROOT/current"

# Launcher: `dev` on PATH running the current tree through `d`.
cat > "$BIN_DIR/dev" <<LAUNCHER
#!/bin/sh
DEV_CLI_ROOT="$CLI_ROOT" DEV_LAUNCHER="$BIN_DIR/dev" exec d "$CLI_ROOT/current/main.dev" -- "\$@"
LAUNCHER
chmod 755 "$BIN_DIR/dev"

echo "Installed dev (DevLang build) to $CLI_ROOT/current"
echo "Launcher: $BIN_DIR/dev"
