# quickshort

A fast multi-monitor screenshot-selection tool in Rust. Pick a region —
including across monitor boundaries — and get a stitched image on your
clipboard + a PNG file.

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
| Windows / macOS | ❌ | fail-loud stubs |

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

```bash
./install.sh                    # build release -> ~/.local/bin + GNOME hotkey
./install.sh --hotkey '<Super><Shift>S'   # custom binding
```

The script installs the binary and registers a GNOME custom shortcut via
`gsettings` (default `Ctrl+Alt+Q`, effective immediately). On non-GNOME
desktops it installs the binary only — bind it yourself.

## Usage

```bash
quickshort                      # select region -> clipboard + quickshort-<ts>.png
quickshort --delay 2            # wait before capture (menus, tooltips)
quickshort --out ~/shots/a.png  # explicit output path
```

Esc or a click without dragging cancels. After capture the process stays
alive serving the clipboard until you copy something else (Wayland
clipboard is peer-to-peer; run via hotkey or `quickshort &`).

## Development

```bash
cargo test                      # unit tests (core stitching)
cargo test -p qs-capture mutter -- --ignored   # real capture, needs GNOME Wayland
cargo run -p qs-app
```

Layout: `qs-core` (geometry + stitch, pure), `qs-capture` (per-OS
backends behind `ScreenCapture`), `qs-overlay` (winit + softbuffer),
`qs-app` (orchestration).

## Known limits

- Mutter inhibits screencast while the screen is locked
- Overlay needs one window per monitor (Wayland has no global window
  positioning); pointer grab carries the selection across monitors
