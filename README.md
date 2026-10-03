# quickshort

A fast multi-monitor screenshot-selection tool in Rust. Pick a region —
including across monitor boundaries — then Save to PNG, Copy the image,
or extract its text via OCR, from an on-selection toolbar.

Designed around a 4-layer pipeline:

1. **Capture** every monitor into RAM
2. **Overlay** borderless windows (one per monitor) showing the captured
   frames dimmed, tracking a drag selection in a global virtual coordinate
   space
3. **Map** selection coordinates through each monitor's origin and DPI
   scale factor
4. **Stitch** per-monitor crops on a worker thread after the overlay closes

## Platforms

| Session | Status | Backend |
|---|---|---|
| GNOME Wayland (Ubuntu/Fedora default) | ✅ works | `org.gnome.Mutter.ScreenCast` → PipeWire shm frames |
| X11 (Xorg, i3, XFCE…) | ✅ works | RandR layout + `GetImage` |
| KDE Plasma / wlroots Wayland | ❌ | needs xdg-desktop-portal fallback |
| macOS 14+ | ✅ works | `SCScreenshotManager` per-display one-shot; needs Screen Recording permission for the terminal app |
| Windows | ✅ works (untested on hardware) | GDI BitBlt per monitor, PER_MONITOR_AWARE_V2 |

OCR (toolbar's text button) uses the OS-native engine — Vision
`VNRecognizeTextRequest` on macOS, WinRT `Windows.Media.Ocr` on Windows —
so no models ship with the binary. Linux has no native OCR API; the plan
is an opt-in `paddle-ocr-rs` backend (PP-OCRv5 det + cls + `latin` rec
covers vi+en) behind a cargo feature, added when Linux support is
actually installed — until then it fails loud.

On Wayland there is no portal dialog and no subprocess: the backend talks
to mutter's private ScreenCast API (the same one the GNOME portal uses
internally), then reads shared-memory buffers from PipeWire in-process —
one connection multiplexes all monitor streams.

## Requirements (GNOME Wayland)

- Rust toolchain
- `libpipewire-0.3-dev`, `libspa-0.2-dev`
- For bindgen without clang: `.cargo/config.toml` already points at GCC's
  freestanding headers; installing `clang` makes that file unnecessary

## Install

### macOS

**DMG** — grab `Quickshort-v*-macos.dmg` from
[Releases](https://github.com/truongnat/quickshort/releases), open it, drag
**Quickshort** into Applications.

**Homebrew** (custom tap):

```bash
brew tap truongnat/tap
brew install --cask quickshort
```

**Unsigned build** — there is no Developer ID signature or notarization.
On first launch Gatekeeper may warn; right-click the app → **Open** →
**Open** to bypass once. The Homebrew cask strips the quarantine
attribute automatically so `brew install` needs no workaround. The app
is signed with a stable self-signed identity purely so macOS keeps your
Screen Recording / Accessibility grants across updates — this does
**not** make it Gatekeeper-trusted.

Required permissions (System Settings → Privacy & Security):

- **Screen Recording** — needed to capture the screen at all
- **Accessibility** — needed for the global hotkey

**Updates** — the app checks GitHub Releases once a day in the
background and shows a toast when a new version is out; the tray item
turns into *Update to vX — Restart to Install*. Manual check:
tray menu → **Check for Updates…**. Homebrew users can also
`brew upgrade --cask quickshort`.

### Linux

```bash
./install.sh                    # build release -> ~/.local/bin + GNOME hotkey
./install.sh --hotkey '<Super><Shift>S'   # custom binding
```

The script installs the binary and registers a GNOME custom shortcut via
`gsettings` (default `Ctrl+Alt+Q`, effective immediately). On non-GNOME
desktops it installs the binary only — bind it yourself.

## Usage

```bash
quickshort                      # resident tray app (menu + global hotkey)
quickshort --capture            # one-shot: select region -> action -> exit
quickshort --capture --delay 2  # wait before capture (menus, tooltips)
quickshort --capture --out ~/shots/a.png  # explicit output path
```

**Tray app** (default mode): icon sits in the menu bar / system tray. Click
it or press the global hotkey (default `CmdOrCtrl+Shift+S`) to start a
selection. The menu offers Capture Region, Recent Captures (opens saved
files), Open Screenshots Folder, Launch at Login and Quit. Settings live in
`~/.config/quickshort/config.toml`:

```toml
hotkey = "CmdOrCtrl+Shift+S"   # global-hotkey syntax
# save_dir = "~/Pictures/quickshort"
launch_at_login = false
```

macOS app bundle: `packaging/macos/bundle.sh` produces `Quickshort.app`
(LSUIElement tray-only). `packaging/macos/release.sh` runs the full
pipeline — release build (universal when both rustup targets exist),
bundle, self-signed identity, zip + dmg + sha256 into `dist/`. Tagging
`v*` triggers the GitHub workflow which does the same and publishes a
GitHub Release. Linux one-shot hotkey binding is still handled by
`install.sh`.

**Selection toolbar**: drag to select, then pick a drawing tool (pen,
arrow, rect, ellipse, marker, pixelate) — a second row offers 3 stroke
sizes and a color palette — or an action: **Save** writes
`~/Pictures/quickshort/quickshort-<ts>.png`, **Copy** puts the image on
the clipboard, **OCR** puts the recognized text on the clipboard. The
toolbar docks to whichever side of the selection has room. Clicking away
starts a new selection; Esc or a click without dragging cancels. Undo/Redo
buttons track strokes and pixelate ops. In `--capture` mode on Wayland the
process stays alive serving the clipboard until you copy something else
(clipboard is peer-to-peer there).

## Development

```bash
cargo test                      # unit tests (core stitching)
cargo test -p qs-capture mutter -- --ignored   # real capture, needs GNOME Wayland
cargo run -p qs-app
```

Layout: `qs-core` (geometry + stitch, pure), `qs-capture` (per-OS
backends behind `ScreenCapture`), `qs-overlay` (winit + softbuffer,
SDF-drawn toolbar — exposes a reusable `Overlay` session for embedding
in a host event loop), `qs-ocr` (native text extraction),
`qs-app` (one-shot CLI + resident tray app via `tray-icon`/`muda`/
`global-hotkey`).

## Known limits

- Mutter inhibits screencast while the screen is locked
- Overlay needs one window per monitor (Wayland has no global window
  positioning); pointer grab carries the selection across monitors
