#!/bin/sh
# Release pipeline: build (universal when both rustup targets exist) -> bundle
# -> sign -> zip + dmg + sha256, everything in dist/.
# usage: packaging/macos/release.sh [version]   (default: Cargo.toml version)
# env:   SIGNING_IDENTITY (default "quickshort"; "-" = ad-hoc)
set -e
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

VER="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' crates/qs-app/Cargo.toml | head -1)}"
IDENT="${SIGNING_IDENTITY:-quickshort}"
APP="Quickshort.app"
DIST="$ROOT/dist"
mkdir -p "$DIST"

# --- build universal (arm64 + x86_64) when both targets are installed ------
ARCHES=""
for T in aarch64-apple-darwin x86_64-apple-darwin; do
  if rustup target list --installed 2>/dev/null | grep -qx "$T"; then
    ARCHES="$ARCHES $T"
  fi
done
if [ "$(echo "$ARCHES" | wc -w)" -ge 2 ]; then
  BINS=""
  for T in $ARCHES; do
    cargo build --release -p qs-app --target "$T"
    BINS="$BINS target/$T/release/quickshort"
  done
  mkdir -p target/universal-release
  lipo -create $BINS -output target/universal-release/quickshort
  BIN=target/universal-release/quickshort
else
  cargo build --release -p qs-app
  BIN=target/release/quickshort
fi

# --- bundle ----------------------------------------------------------------
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/quickshort"
sed "s/__VERSION__/$VER/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"
cp packaging/macos/Quickshort.icns "$APP/Contents/Resources/Quickshort.icns"

# --- sign (stable identity so TCC grants survive updates) -------------------
if codesign --force --deep --sign "$IDENT" --options runtime "$APP" 2>/dev/null; then
  echo "signed with '$IDENT'"
else
  echo "identity '$IDENT' unavailable — ad-hoc signing"
  codesign --force --deep --sign - "$APP"
fi
codesign --verify --deep --strict "$APP" && echo "signature verified"

# --- archive + dmg ----------------------------------------------------------
ZIP="$DIST/Quickshort-v$VER-macos.zip"
rm -f "$ZIP"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"

DMG="$DIST/Quickshort-v$VER-macos.dmg"
rm -f "$DMG"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
if command -v create-dmg >/dev/null 2>&1; then
  create-dmg --volname Quickshort --hide-extension "$APP" \
    --app-drop-link 380 205 --icon "$APP" 155 205 \
    --window-size 560 380 "$DMG" "$STAGE" || true
fi
[ -f "$DMG" ] || hdiutil create -volname Quickshort -srcfolder "$STAGE" \
  -ov -format UDZO "$DMG" >/dev/null

# --- checksums ---------------------------------------------------------------
( cd "$DIST" && shasum -a 256 "Quickshort-v$VER-macos.zip" > "Quickshort-v$VER-macos.zip.sha256" \
  && shasum -a 256 "Quickshort-v$VER-macos.dmg" > "Quickshort-v$VER-macos.dmg.sha256" )

echo "---"
ls -lh "$DIST" | tail -4
echo "release artifacts ready in $DIST"
