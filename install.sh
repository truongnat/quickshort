#!/usr/bin/env bash
# Install quickshort: release build -> ~/.local/bin + GNOME custom hotkey.
# usage: ./install.sh [--skip-build] [--hotkey "<Ctrl><Alt>Q"]
set -euo pipefail

cd "$(dirname "$0")"

SKIP_BUILD=0
HOTKEY="${QS_HOTKEY:-<Ctrl><Alt>q}"
while [ $# -gt 0 ]; do
    case "$1" in
        --skip-build) SKIP_BUILD=1 ;;
        --hotkey) HOTKEY="$2"; shift ;;
        *) echo "unknown arg: $1" >&2; exit 1 ;;
    esac
    shift
done

if [ "$SKIP_BUILD" = 0 ]; then
    cargo build --release -p qs-app
fi

BIN="$HOME/.local/bin/quickshort"
install -Dm755 "${CARGO_TARGET_DIR:-/data/cargo-target}/release/quickshort" "$BIN"
echo "installed $BIN"

# --- GNOME custom shortcut via gsettings (no Settings UI needed) ---------
if [[ "${XDG_CURRENT_DESKTOP:-}" != *GNOME* ]]; then
    echo "non-GNOME desktop; bind $BIN manually in your settings"
    exit 0
fi

SCHEMA="org.gnome.settings-daemon.plugins.media-keys"
BIND_SCHEMA="org.gnome.settings-daemon.plugins.media-keys.custom-keybinding"
BIND_PATH="/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/quickshort/"

python3 - "$SCHEMA" "$BIND_PATH" <<'EOF'
import ast, subprocess, sys
schema, path = sys.argv[1], sys.argv[2]
cur = subprocess.check_output(
    ["gsettings", "get", schema, "custom-keybindings"], text=True
).strip()
lst = ast.literal_eval("[]" if cur.startswith("@as") else cur)
if path not in lst:
    lst.append(path)
    subprocess.check_call(
        ["gsettings", "set", schema, "custom-keybindings", str(lst)]
    )
EOF

gsettings set "$BIND_SCHEMA:$BIND_PATH" name    "quickshort"
gsettings set "$BIND_SCHEMA:$BIND_PATH" command "$BIN --capture"
gsettings set "$BIND_SCHEMA:$BIND_PATH" binding "$HOTKEY"

echo "hotkey $HOTKEY -> quickshort (takes effect immediately)"
