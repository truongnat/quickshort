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

# --- system deps (Debian/Ubuntu) ----------------------------------------
# build:  pkg-config libpipewire-0.3-dev libspa-0.2-dev clang libdbus-1-dev
# run:    pipewire x11-utils tesseract-ocr wl-clipboard libnotify-bin ffmpeg
# ocr:    tesseract-ocr-<lang> for each in QS_OCR_LANGS
if command -v apt-get >/dev/null 2>&1; then
    PKGS=()
    need() { command -v "$1" >/dev/null 2>&1 || PKGS+=("$2"); }
    pc()   { pkg-config --exists "$1" 2>/dev/null || PKGS+=("$2"); }
    if [ "$SKIP_BUILD" = 0 ]; then
        pc libpipewire-0.3 libpipewire-0.3-dev
        pc libspa-0.2 libspa-0.2-dev
        need clang clang
        pc dbus-1 libdbus-1-dev
        need pkg-config pkg-config
    fi
    need tesseract tesseract-ocr
    need wl-copy wl-clipboard
    need notify-send libnotify-bin
    need ffmpeg ffmpeg  # screen recording encodes through it
    for l in ${QS_OCR_LANGS:-eng jpn vie}; do
        tesseract --list-langs 2>/dev/null | grep -qx "$l" || PKGS+=("tesseract-ocr-$l")
    done
    if [ ${#PKGS[@]} -gt 0 ]; then
        echo "installing: ${PKGS[*]}"
        sudo apt-get update -qq && sudo apt-get install -y "${PKGS[@]}"
    fi
else
    echo "note: ensure pipewire dev headers, tesseract-ocr, wl-clipboard, libnotify and ffmpeg are installed" >&2
fi

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
