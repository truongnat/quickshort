#!/bin/sh
# Linux packaging: .deb (dpkg-deb, no extra tools) + .AppImage (linuxdeploy
# with the gtk plugin so tray-icon's libayatana dependency is bundled).
# usage: packaging/linux/package.sh [version]
set -e
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

VER="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' crates/qs-app/Cargo.toml | head -1)}"
BIN="target/release/quickshort"
DIST="$ROOT/dist"
mkdir -p "$DIST"

cargo build --release -p qs-app

# --- .deb ------------------------------------------------------------------
PKGDIR="$(mktemp -d)/quickshort_${VER}_amd64"
mkdir -p "$PKGDIR/DEBIAN" "$PKGDIR/usr/bin" \
  "$PKGDIR/usr/share/applications" "$PKGDIR/usr/share/icons/hicolor/1024x1024/apps"
cp "$BIN" "$PKGDIR/usr/bin/quickshort"
cp packaging/linux/quickshort.desktop "$PKGDIR/usr/share/applications/quickshort.desktop"
cp docs/assets/icon.png "$PKGDIR/usr/share/icons/hicolor/1024x1024/apps/quickshort.png"
cat > "$PKGDIR/DEBIAN/control" <<EOF
Package: quickshort
Version: $VER
Section: utils
Priority: optional
Architecture: amd64
Maintainer: truongnat <noreply@github.com>
Depends: libgtk-3-0, libayatana-appindicator3-1, libpipewire-0.3-0, tesseract-ocr, tesseract-ocr-eng, wl-clipboard, libnotify-bin, openssh-client
# ffmpeg is a Recommends not a Depends: recording (⌘R) needs it, everything
# else works without — apt installs it anyway unless --no-install-recommends
Recommends: tesseract-ocr-jpn, tesseract-ocr-vie, ffmpeg
Description: Region screenshot tool with annotations and native OCR
 A pure-Rust desktop utility: multi-monitor region capture, annotation
 layer, OS-native OCR, tray resident.
EOF
dpkg-deb --build "$PKGDIR" "$DIST/quickshort_amd64.deb"

# --- .AppImage ---------------------------------------------------------------
TOOLS="$(mktemp -d)"
curl -sL -o "$TOOLS/linuxdeploy.AppImage" \
  "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage"
# linuxdeploy discovers plugins as executables named linuxdeploy-plugin-<name>
# on PATH — the file must keep this exact name, not a .sh suffix.
curl -sL -o "$TOOLS/linuxdeploy-plugin-gtk" \
  "https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh"
chmod +x "$TOOLS/linuxdeploy.AppImage" "$TOOLS/linuxdeploy-plugin-gtk"
export PATH="$TOOLS:$PATH"

APPDIR="$(mktemp -d)/AppDir"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" \
  "$APPDIR/usr/share/icons/hicolor/1024x1024/apps"
cp "$BIN" "$APPDIR/usr/bin/quickshort"
cp packaging/linux/quickshort.desktop "$APPDIR/usr/share/applications/quickshort.desktop"
cp docs/assets/icon.png "$APPDIR/usr/share/icons/hicolor/1024x1024/apps/quickshort.png"
# linuxdeploy wants the desktop file + icon at the appdir root too
cp packaging/linux/quickshort.desktop "$APPDIR/quickshort.desktop"
cp docs/assets/icon.png "$APPDIR/quickshort.png"

# AppImages can't self-exec on FUSE-less hosts without this env; CI has none
export APPIMAGE_EXTRACT_AND_RUN=1
export LDAI_OUTPUT="$DIST/Quickshort-Linux.AppImage"
export OUTPUT="$DIST/Quickshort-Linux.AppImage"
# embed gh-releases-zsync update info — AppImageUpdate can then self-update
# from GitHub Releases (the .zsync it generates lands in dist/ alongside)
export UPDATE_INFORMATION="gh-releases-zsync|truongnat|quickshort|latest|Quickshort-Linux.AppImage.zsync"
# newer linuxdeploy builds dropped the -u flag — UPDATE_INFORMATION env var
# alone is what appimagetool reads for the embedded update info
if ! "$TOOLS/linuxdeploy.AppImage" \
  --appdir "$APPDIR" --plugin gtk \
  --output appimage > "$TOOLS/linuxdeploy.log" 2>&1; then
    echo "linuxdeploy failed; shipping .deb only"
    tail -60 "$TOOLS/linuxdeploy.log" >&2 || true
fi
ls -lh "$DIST"/* 2>/dev/null || true
echo "linux artifacts ready in $DIST"
