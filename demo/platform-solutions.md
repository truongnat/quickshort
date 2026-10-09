# Quickshort — 100%-parity solutions across macOS / Windows / Linux

Companion to `platform-feasibility.md`. That doc answered "can it be
built"; this one answers "how do we ship the **full** spec everywhere —
including GNOME-Wayland, which `platform-feasibility.md §2-A` flagged as
degraded-by-compositor-policy".

The unlock is one architectural move — **the Chrome Layer** — plus three
per-OS recipes for the leftover pieces (drag-out, global hotkeys, focus).

---

## 0. The core idea: stop asking the OS to position windows

Every hard problem traces to the same root: we want chrome (album stack,
fly animation, REC/SCROLL dock, toasts, pin overlays) at **arbitrary
screen coordinates**, and on Wayland `set_outer_position` /
`AlwaysOnTop` are protocol-level no-ops (winit `wayland/window/mod.rs`,
verified in `platform-feasibility.md`).

**Don't position a window. Position pixels inside a fullscreen
transparent surface.** The capture overlay already proves the
envelope: one borderless-fullscreen surface per monitor is honored by
every compositor (fullscreen is a *state*, not a position — it works on
mutter today, `qs-overlay/src/lib.rs:1134-1181`). The only missing
ingredient is **per-pixel alpha**, because softbuffer presents opaque
buffers (`widgets.rs:53-56`; Wayland backend writes `Xrgb8888`).

So: keep winit for windows that are real windows (settings, pins,
overlay); add a per-monitor **chrome surface** whose buffer we own:

- **Render**: ARGB8888 buffer we fill ourselves — every chrome element
  is just `blit(x, y)`; the fly animation is *repainting*, not window
  moves (cheaper and smoother than `set_outer_position` at 60fps —
  zero compositor IPC per frame).
- **Input**: the surface only accepts clicks where chrome is painted —
  per-OS input-region mechanism below — so the rest of the desktop is
  genuinely click-through, including the empty 95% of the layer.
- **Lifetime**: the layer doesn't exist until the first chrome appears
  (first capture lands a stack thumb) and is torn down when empty —
  zero resident cost. One ~8MB RGBA buffer per monitor while visible.
- **Z-order**: one always-on-top-ish surface. macOS: NSWindow level;
  Windows: `HWND_TOPMOST`; X11: `WM_NORMAL_ABOVE` + raise; Wayland:
  toplevel with `wl_surface` shown — compositor policy still applies
  but a *just-appeared* fullscreen-interactive surface lands on top in
  practice [verify per compositor].

This single mechanism delivers on **every** compositor, including
mutter: corner docked stack, fly-to-corner, per-monitor docking (the
layer *is* per-output), pin opacity (alpha is in our pixels, not a
window attribute), live badges/progress — because none of it depends on
window placement anymore.

### Per-OS recipe for the chrome surface

| OS | Buffer / surface | Per-pixel alpha | Input region (click-through outside chrome) |
|---|---|---|---|
| macOS | winit window + custom `NSView` drawing `CGImage` (or keep softbuffer for chrome drawn opaque? **no** — need real alpha: `NSWindow.opaque=NO`, `backgroundColor=clear`, view draws ARGB `CGImage`; all bindings in vendored objc2-app-kit) | yes | override `hitTest`/`NSView.hitTest` → return `nil` outside chrome rects; or `ignoresMouseEvents` toggling. Focusable since `canBecomeKeyWindow`→true already (winit macos impl) |
| Windows | `WS_EX_LAYERED` + `UpdateLayeredWindow(ULW_ALPHA)` with a premultiplied 32bpp `DIBSection` | yes | **free** — documented behavior: hit-testing follows per-pixel alpha, α=0 pixels pass clicks through ([learn.microsoft.com layered windows](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-updatelayeredwindow), [SO#13884561](https://stackoverflow.com/questions/13884561)) |
| X11 | 32-bit ARGB visual window (`x11rb`), composite manager blends it | yes (requires compositor — near-universal now; fallback: opaque rounded-rect layer still positions fine) | `XFixes` region via `XShape` *input* shape set to chrome rects (`xcb_shape_rectangles`, `SK_INPUT`) |
| Wayland (all) | own `wl_surface` + `wl_shm` pool, `ARGB8888` (**mandatory format, every compositor** — [wayland-book shm](https://wayland-book.com/surfaces/shared-memory.html); premultiplied alpha — LVGL bug #9894 confirms the premultiply requirement) | yes, mutter included | `wl_surface.set_input_region` to the union of chrome rects; `null` = click-through ([wayland-book regions](https://wayland-book.com/surfaces-in-depth/surface-regions.html)). **This is core protocol — mutter, KWin, sway all honor it.** |

Why not layer-shell on wlroots/KWin? Because the chrome layer works on
**more** compositors (mutter included) with **one** code path; adding
wlr-layer-shell later is purely an upgrade for edge cases (in-app
fullscreen games pushing the layer down).

Effort: **M per OS** (~800-1200 lines each: buffer alloc + blit loop +
input-region bookkeeping). The renderer is already written — the chrome
widgets already draw into RGB buffers via softbuffer; we keep the same
draw calls and just composite onto an ARGB canvas.

### What moves into the chrome layer vs. stays a window

| Element | In chrome layer? | Why |
|---|---|---|
| Album stack + fly | yes | needs arbitrary position + animation + (later) opacity |
| Toast / REC / SCROLL docks | yes | same; also fixes the scroll-dock-overlaps-capture bug — we choose the rect |
| Lightbox preview | yes (simplest: draw inside layer, it already covers the screen) | avoids a second always-on-top window |
| Pin windows | **borderline** — pins want drag + resize; implement as *draggable region inside the layer* (we get pointer events via input region, drag = repaint). Pin opacity = just alpha in our buffer. Or keep real windows on mac/Win/X11 where positioning works, layer only on Wayland | decision per OS |
| Settings window | **no** — stays a normal winit window | it's a document window, no chrome-layer needs |
| Capture overlay | **no** — stays opaque fullscreen winit | it's *supposed* to own the screen |

---

## 1. Drag-out (file promises) — per-OS, X11 included

| OS | Path | Effort |
|---|---|---|
| macOS | `NSFilePromiseProvider` on the chrome `NSView` (bindings already vendored in objc2-app-kit 0.3.2) | M |
| Windows | `drag` crate save-then-drag now (S); lazy `IDropSource`+`CFSTR_FILEDESCRIPTORW` later (L) | S→L |
| X11 | **Reuse the gtk thread that already exists for the tray** (`tray.rs:135-246`): create a hidden `gtk::Window` under the cursor, `gtk_drag_source_set` + `text/uri-list` of the saved file, `gtk_drag_begin_with_coordinates`. GTK owns the XDND source protocol — we skip the XL hand-rolled second-X-connection entirely | M |
| Wayland | raw `wl_data_source` offering `image/png` + `text/uri-list`, driven by the pointer **serial** — and here's the trick: the chrome layer's input handling is our own Wayland code, so *we hold the serial* that winit 0.30 wouldn't expose. If we keep winit for the surface, winit≥0.31 `start_drag` covers it anyway | M |

Net: drag-out ships on **all four** targets; X11 via gtk is the clever
bit (gtk3 `GtkDragSource`-equivalent API is `gtk_drag_begin*` +
`drag_data_get` — well documented, and the dep is already in Cargo.toml).

## 2. Global hotkeys on Wayland — the "impossible" one, solved per-compositor

`global-hotkey` is X11-only and lies about success on Wayland
(`platform-feasibility.md §2-C`). 100% coverage doesn't mean "portal or
bust" — each Wayland desktop has its own programmatic registration:

| Desktop | Mechanism | How |
|---|---|---|
| GNOME Wayland | **`gsettings` custom keybindings** — `org.gnome.settings-daemon.plugins.media-keys custom-keybindings` list + per-key `custom-keybinding` (name/binding/command). Apps can write these directly; no extension, no portal, works today | Onboarding card "Register shortcut" → we write `/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/quickshort0/` with `command='quickshort --capture'` — **S** |
| KDE Plasma Wayland | `org.kde.KGlobalAccel` D-Bus API (register programmatically) — or the `GlobalShortcuts` portal which KDE implements ([flatpak portal docs](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html)) | M — zbus call |
| sway/wlroots | `swaymsg bindsym <combo> exec quickshort --capture` — runtime binding, persists per-session; config write optional | S — spawn `swaymsg` |
| Hyprland | `hyprctl keyword bind '<combo>, exec, quickshort --capture'` — same story | S |
| wlroots-generic / others | guided step: open compositor config + copyable command (already the documented workaround, `host.rs:444`) | S — UX copy |

This turns "dead on Wayland" into a **per-desktop onboarding row**:
`Shortcut: ⌘⇧S — [Register automatically]` with a status dot. That's a
*better* UX than a conflict warning, and it's how every serious Wayland
tool does it (portal where supported, compositor IPC where it exists).

## 3. Focus on Wayland (the honest residual)

A fullscreen chrome layer can't *claim* keyboard focus without a click
or activation token — protocol design, not a bug. 100% parity strategy:

- Stack keyboard nav (arrows/Space/⌘C/⌫) works **after one click** on
  Wayland — input region delivers pointer events, click grants focus.
  The macshot keymap is fully preserved; only hover-grants-focus is
  GNOME-Wayland-only-missing (works on mac/Win/X11/KWin via
  `xdg_activation` where offered).
- The `Show capture stack` global shortcut path (tray item /
  custom-keybinding → `quickshort --show-stack`) still works; the layer
  appears with an input region covering the stack so the first click
  lands on a thumb, not through it.

That's the entire remaining delta: **one click**. Everything else —
position, animation, opacity, drag-out, per-monitor — is byte-identical
across all five targets.

## 4. Non-negotiable fixes uncovered by this design

1. **Scroll capture on portal backends**: `scroll.rs` polls
   `capture_all()` per 450ms; `portal.rs` creates a session per call →
   per-poll consent dialogs where `persist_mode` isn't honored
   (`platform-feasibility.md §2-D`). Switch scroll/record to the
   existing `LiveStream` push path (`qs-capture/src/lib.rs:43-48`) —
   correctness, not polish.
2. **`popup_monitor` (widgets.rs:14)** docks chrome to the primary
   monitor — with a per-output chrome layer this bug disappears for
   free on every OS (the stack draws on the output the capture came
   from; `sel_monitor` already knows it, `lib.rs:444`).
3. **Pin centering** (`pin.rs:47-58`) — cascade +24px, trivial wherever
   pins live.
4. **Stray-drag cancel** (`lib.rs:1452`) — require `moved==false` for
   cancel, already specced in r2 §2.5.

## 5. Build order (revised for the chrome layer)

1. **macOS chrome layer** — NSWindow+custom NSView+hitTest; album + fly +
   toast move into it. The stack, drag-out (`NSFilePromiseProvider`),
   TCC onboarding, reduced-motion ship together. macOS = reference impl.
2. **Windows chrome layer** — `WS_EX_LAYERED` + `UpdateLayeredWindow`
   DIB; alpha hit-test is free; `drag` crate for drag-out;
   `SPI_GETCLIENTAREAANIMATION`. Straightforward port of step 1.
3. **X11** — ARGB visual + XShape input region + `x11rb` (already an
   indirect dep via winit's X backend? verify [?] — else add); drag-out
   via the gtk-thread trick; `_NET_WM_WINDOW_OPACITY` for real-window
   pins.
4. **Wayland** — raw `wl_surface`/`wl_shm` chrome layer (wayland-client
   + wayland-protocols crates; `linux/wayland.rs` precedent exists in
   qs-capture). `set_input_region` bookkeeping. Per-compositor hotkey
   onboarding (§2). `LiveStream` for scroll/record regardless.

Total new platform code ≈ 3.5-5k lines across four backends — most of it
mechanical (buffer alloc, region math, one blit loop). The renderer and
all UX logic stay shared.

## 6. What this buys that window-placement never could

- Fly animation is **repaint-based** → same easing/frame budget on all
  OSes, no compositor round-trip per frame (smoother than
  `set_outer_position` even on macOS).
- Fan/hover micro-animations, badge pulses, progress bars — all just
  pixels, no per-widget window juggling.
- Click-through pins and "hover the desktop while stack is up" come
  free from input regions.
- One renderer = one behavior = one test surface. The four platform
  shims are pure plumbing.
