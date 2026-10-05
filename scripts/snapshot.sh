#!/bin/sh
# Snapshot every settings page → docs/shots/settings-<page>.png
#
# Each shot launches `quickshort --settings` with:
#   QS_PAGE=n    jump straight to page n
#   QS_ACCENT=x  recolor without touching the saved config
#   QS_SHOT=path dump the first rendered frame and exit (QS_EXIT)
# The first frame only lands once macOS delivers RedrawRequested, so we
# poll for the file instead of assuming a fixed delay.
set -e
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

cargo build --release -p qs-app
BIN="$ROOT/target/release/quickshort"
OUT="$ROOT/docs/shots"
mkdir -p "$OUT"

pages="capture general shortcuts appearance storage about"
i=0
for name in $pages; do
    png="$OUT/settings-$name.png"
    rm -f "$png"
    QS_SHOT="$png" QS_PAGE=$i QS_ACCENT="${QS_ACCENT:-blue}" QS_EXIT=1 \
        "$BIN" --settings &
    pid=$!
    n=0
    while [ ! -s "$png" ] && [ $n -lt 300 ]; do
        sleep 0.1
        n=$((n + 1))
    done
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    if [ -s "$png" ]; then
        echo "  $png"
    else
        echo "  FAILED: $png" >&2
        exit 1
    fi
    i=$((i + 1))
done
echo "snapshots -> $OUT"
