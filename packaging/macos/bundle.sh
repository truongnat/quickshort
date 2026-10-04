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
# ad-hoc fallback — auto-detect the `quickshort` cert when the env var
# isn't set so a bare `./bundle.sh` still gets the stable identity
IDENT="${SIGNING_IDENTITY:-}"
if [ -z "$IDENT" ]; then
    # find-identity -v misses self-signed certs lacking trust settings even
    # though codesign can use them — resolve by cert SHA-1 instead
    IDENT=$(security find-certificate -c quickshort -Z \
        ~/Library/Keychains/login.keychain-db 2>/dev/null \
        | awk '/SHA-1/ {print $3; exit}')
fi
codesign --force --sign "${IDENT:--}" "$APP" 2>/dev/null \
  || codesign --force --sign - "$APP" 2>/dev/null || true
# the LaunchAgent runs the raw release binary — same cert AND identifier
# as the .app or the TCC designated requirement splits into two grants
codesign --force --sign "${IDENT:--}" --identifier ai.quickshort \
  "$TARGET/quickshort" 2>/dev/null || true
echo "built $APP"
