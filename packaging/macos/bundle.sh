#!/bin/sh
# Build quickshort.app from the release binary.
# usage: packaging/macos/bundle.sh [target-dir]
set -e
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TARGET="${1:-$ROOT/target/release}"
APP="$ROOT/quickshort.app"

cargo build --release --manifest-path "$ROOT/Cargo.toml"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$TARGET/quickshort" "$APP/Contents/MacOS/quickshort"
VER="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/crates/qs-app/Cargo.toml" | head -1)"
sed "s/__VERSION__/$VER/g" "$ROOT/packaging/macos/Info.plist" > "$APP/Contents/Info.plist"
cp "$ROOT/packaging/macos/Quickshort.icns" "$APP/Contents/Resources/Quickshort.icns"
# stable self-signed identity if present (TCC grants survive rebuilds),
# ad-hoc fallback
codesign --force --sign "${SIGNING_IDENTITY:--}" "$APP" 2>/dev/null \
  || codesign --force --sign - "$APP" 2>/dev/null || true
echo "built $APP"
