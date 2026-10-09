# Quickshort — platform feasibility, per-OS

Companion to `app-ui-research-r2.md` §8: for every proposed UX feature,
can it be built with the actual stack (winit 0.30 + softbuffer 0.4 +
fontdue/ab_glyph, CPU raster only — no GPU), what API/crate is needed,
effort (S/M/L/XL), and hard blockers.

Method: read the crates first (what exists vs. what's needed), verify
external claims against vendored sources + docs. `[?]` = unverified.
"Wayland-GNOME" = mutter (mutter ScreenCast backend, no layer-shell);
"Wayland-wlr" = wlroots compositors (sway/Hyprland) + KDE KWin.

## What the code already does (verified)

- `qs-overlay/src/lib.rs:1134-1181` — one borderless window per monitor;
  `Fullscreen::Borderless(m)` on non-macOS (this *is* supported on Wayland —
  fullscreen is a state request, not a position), explicit NSWindow
  level/frame surgery on macOS via `qs-overlay/src/macos.rs:16-54`
  (objc2 + raw-window-handle pattern already in tree).
- `qs-app/src/widgets.rs:58-106` — `round_window`: macOS layer cornerRadius,
  Windows `SetWindowRgn`, Linux no-op. softbuffer presents **opaque**
  pixels on all three (widgets.rs:53-56 comment; softbuffer-0.4.8
  `backends/wayland/buffer.rs:102` writes `wl_shm::Format::Xrgb8888` —
  no alpha channel on Wayland).
- Popups (`quick_access.rs:148-158`, `recstop.rs:44-55`, `toast.rs:56-66`):
  borderless + `with_transparent` + `WindowLevel::AlwaysOnTop` +
  `set_outer_position(dock_pos(pos, popup_monitor))`. On Wayland both
  calls are no-ops (see below) — today those windows land wherever the
  compositor puts them.
- `qs-overlay/src/lib.rs:386,1474` — `ModifiersChanged` → `self.mods`
  already tracked; keyboard events flow to the overlay (IME already works).
- `qs-capture/src/linux.rs:13-37` — GNOME→mutter DisplayConfig backend,
  else portal. `portal.rs:82-106` uses `persist_mode=2` + stored
  `restore_token` (skips the consent dialog where supported — KDE);
  a **new** portal session is created per `capture_all`.
- `qs-capture/src/macos.rs:107-130` — `CGPreflightScreenCaptureAccess` /
  `CGRequestScreenCaptureAccess` FFI already linked. Onboarding just
  needs the UI states.
- `pin.rs:37-58` — pins center on primary monitor (the documented
  stacking bug); `drag_window()` used for move (works on Wayland too —
  interactive move is a serial-driven request, not positioning).
- `host.rs:444-451` — already documents Wayland hotkey dead-end and the
  `quickshort --capture` custom-shortcut workaround.
- `host.rs:296-322` — hotkey registration already catches register()
  failure, toasts, and reverts to the old binding.
- Linux tray: gtk 0.18 on a dedicated thread (`tray.rs:135-246`) — a gtk
  context exists in-process for settings queries (e.g. `gtk-enable-animations`).

## §1 Feasibility matrix

Cell = effort + note. "winit-0.31" refers to the merged DnD API (see §2-B).

| Feature | macOS | Windows | X11 | Wayland-GNOME | Wayland-wlr (sway/KWin) |
|---|---|---|---|---|---|
| **Stack window** (always-on-top borderless popup) | S — pattern exists (`quick_access.rs`) | S | S | **M — broken**: `set_outer_position` + `AlwaysOnTop` are no-ops on Wayland (winit `wayland/window/mod.rs:430` `set_window_level{}`; `set_outer_position` unsupported — [winit discussion #2920](https://github.com/rust-windowing/winit/discussions/2920)). Window appears wherever mutter places it, can be covered | M — same protocol limits; fixable via **wlr-layer-shell** (KWin supports it, mutter does not — [wayland.app support list](https://wayland.app/protocols/wlr-layer-shell-unstable-v1), [kwin MR !202](https://invent.kde.org/plasma/kwin/-/merge_requests/202)) |
| **Fly-to-corner animation** | M — animate a real window: `set_outer_position`+`request_inner_size` per frame (~frame cost trivial: ≤600px surface realloc+blit at 60fps); or direct `NSWindow.setFrame` via objc2 | M — `SetWindowPos` per frame | M — `xcb_configure_window` per frame | **X** — no client-side window motion; fallback = 150ms fade | L — layer-shell surfaces can animate via margin updates per frame (compositor repositions); else fade fallback |
| Animate inside a fullscreen transparent window (alt approach) | **X** — softbuffer opaque (widgets.rs:53) | **X** — DIB opaque (widgets.rs:76) | **X** | **X** — Xrgb8888 buffer, no alpha | **X** — same; needs GPU/ARGB surface to do this |
| **Drag-out file promise** | M — `NSFilePromiseProvider` + `NSView.beginDraggingSession` — all bindings in vendored `objc2-app-kit-0.3.2` (`generated/NSFilePromiseProvider.rs`, `NSView.rs:1397`); S if save-then-drag via `drag` crate | M — save-then-drag via `drag` crate (supports winit) = S; lazy `IDropSource`/`IDataObject`+`CFSTR_FILEDESCRIPTORW` = L, needs `windows` features `Win32_System_Ole`/`_Com`/`_DataExchange` | **XL** — winit owns the X connection and eats XDND ClientMessages on its window; a second x11rb connection can own a dedicated drag-source window (xdnd spec impl) — doable, big | **XL today / S on winit≥0.31** — `wl_data_source` needs the input *serial*; winit 0.30 doesn't expose it. winit PR [#4571](https://github.com/rust-windowing/winit/pull/4571) merged: `start_drag` works on Wayland in next release | same |
| **Overlay modifiers + nudge** (⌥⇧Space, arrows) | S — `ModifiersChanged`+`KeyboardInput` already delivered during drag (lib.rs:1474); pointer grab doesn't affect keyboard | S | S | S — **if** overlay has kb focus; focus acquisition on compositor is unverified [?] (no activation token when launched via keybinding) | S |
| **SCROLL/REC dock** (positioned popup) | S | S | S | **M — unfixed**: window lands compositor-placed (mutter centers/whatever); worse: an uncontrolled dock can sit **inside** the scroll-capture rect and be stitched into the image | M — layer-shell gives corner anchoring; else compositor-placed |
| **Pin wheel-scale** | S — rescale `img`+resize+`round_window` per tick; ~1MB CPU blit, trivial | S | S | S — scale works (resizing is allowed); drag still works via `drag_window` | S |
| **Pin opacity** | S — `NSWindow.setAlphaValue` (objc2 in tree; works despite opaque buffer — compositor-level) | S — `SetLayeredWindowAttributes` (auto-adds `WS_EX_LAYERED`); needs `Win32_UI_WindowsAndMessaging` feature in **qs-app's** windows dep | S — `_NET_WM_WINDOW_OPACITY` cardinal on our XID (de-facto standard, honored by compositing WMs; picom/mutter-x11/kwin-x11) | **X** — no client protocol; GDK docs: ["On Wayland, there is no per-window opacity"](https://docs.gtk.org/gdk3/method.Window.set_opacity.html) | M — `wp_alpha_modifier_v1` (works on alpha-less buffers too) — supported on KWin 6.7, Hyprland, sway, river; **not** mutter, wayfire, niri — [wayland.app](https://wayland.app/protocols/alpha-modifier-v1). Needs `wayland-client` `Backend::from_foreign_display` on winit's wl_display [?] |
| **Pin cascade +24px** | S | S | S | **X** — can't set position | X — same (layer-shell could anchor-cascade but that's reimplementation) |
| **Tray IconMenuItem** | S — `muda::IconMenuItem` → NSMenuItem.image | S — HBITMAP impl exists (`muda-0.21.0/platform_impl/windows/mod.rs:606+`) | S — gtk impl exists; but tray goes through AppIndicator/SNI D-Bus menu — icon rendering depends on the shell implementation [?] | same as X11 | same |
| **Hotkey conflict detection** | S — `RegisterEventHotKey` errors → `FailedToRegister` (global-hotkey macos/mod.rs:125-127); can't name owner — honest copy per r2 doc | S — `RegisterHotKey` fails on conflict | S — `Error::AlreadyRegistered` on BadAccess grab (x11/mod.rs:155-164) | **X** — global-hotkey silently dead on Wayland (x11-only impl; dead thread → register() still returns Ok). Hide the UI; keep the "--capture keybinding" advice. Portal `GlobalShortcuts` exists but only KDE implements it, GNOME48 "claims but not really" — [SourceReviver notes](https://github.com/SourceReviver/qt_wayland_globalshortcut_via_portal), [portal docs](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html) | X — same |
| **TCC preflight + grantedPendingRelaunch** | S — FFI already in `qs-capture/macos.rs:107-130`; onboarding is UI states + `NSWorkspace.openSystemPreferences`/`open` URL (or `objc2-core-graphics` `CGWindow` feature for the typed binding — [docs.rs](https://docs.rs/objc2-core-graphics/latest/objc2_core_graphics/fn.CGPreflightScreenCaptureAccess.html)) | n/a | n/a | n/a | n/a |
| **Reduced-motion detection** | S — `NSWorkspace` adopts `NSAccessibility` → `accessibilityDisplayShouldReduceMotion` (`objc2-app-kit-0.3.2/generated/NSAccessibility.rs:164`) | S — `SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION)` ([MS docs](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-systemparametersinfow); Chromium maps it to prefers-reduced-motion — [animation_win.cc](https://chromium.googlesource.com/chromium/src/+/refs/tags/142.0.7444.60/ui/gfx/animation/animation_win.cc)); needs `Win32_UI_WindowsAndMessaging` | S — gsettings `org.gnome.desktop.interface enable-animations` via gtk/gio (tray thread) or zbus | S — same gsettings key | S — same; KDE: `kdeglobals [KDE] AnimationDurationFactor==0` [?] |
| **Popup keyboard focus** (stack keyboard nav, hover-focus) | M — winit's `WinitWindow` overrides `canBecomeKeyWindow`→true even for borderless (`winit-0.30.13/platform_impl/macos/window.rs:114`); `activate()`+`makeKeyAndOrderFront` works but steals focus; resign/restore dance (accessory app ⇒ focus returns on window close) needs care | S — `focus_window`/`SetFocus` normal path | S — `_NET_ACTIVE_WINDOW`/`focus_window` | **X** — no client-initiated activation without `xdg_activation` token (needs a serial we don't have); keys work only after user clicks the window | same — same protocol limit |
| **Per-monitor stack docking** | S — pass capture's monitor to `dock_pos` (fix `popup_monitor`, widgets.rs:14) | S — monitor-from-capture (no global cursor pos needed: `sel_monitor` already knows, lib.rs:444) | S | **X** — winit *reports* monitor rects via xdg-output (`wayland/output.rs:65`) but can't place on them | M — layer-shell surfaces bind to a specific `wl_output` |
| **Crosshair / magnifier** | S — `bright` composite already in memory (lib.rs:248-294); magnifier = blit + circle clip | S | S | S | S |
| **Overlay on Wayland gets Esc/keys** | n/a | n/a | n/a | [?] — fullscreen window focus on a shortcut-spawned process is compositor policy; `request_user_attention` (lib.rs:1161) is a no-op w/o activation token. Needs on-device verification | [?] same |
| **Settings ⌘F search** | S — pure softbuffer widget code | S | S | S | S |

## §2 The hard problems, ranked

### A. Wayland has no window placement, stacking level, or opacity — and GNOME has no escape hatch either
- `set_outer_position` and `WindowLevel::AlwaysOnTop` are silently ignored
  (winit wayland backend: `set_window_level` is an empty fn,
  `platform_impl/linux/wayland/window/mod.rs:430`). Today this means the
  toast/card/REC dock on Wayland land wherever the compositor puts them —
  likely centered, and *coverable by other windows*.
- On wlroots/KDE the escape hatch is `zwlr_layer_shell_v1` (corner-anchored,
  per-output, always-on-top). Mutter does not implement it at all
  ([wayland.app list](https://wayland.app/protocols/wlr-layer-shell-unstable-v1));
  GNOME gets no fix. Rust path: `wayland-client` + `wayland-protocols-wlr`
  on top of winit's display (`Backend::from_foreign_display` [?]), or
  `gtk-layer-shell` if we ever host popups as GTK windows.
- **Opinion:** accept degraded placement on GNOME-Wayland (document it);
  implement layer-shell for wlr/KDE only if Wayland user share justifies
  it (M–L, one seam). Fix the dangerous sub-case: the SCROLL dock must
  never overlap the capture rect — on Wayland we can't guarantee that, so
  gate scroll-capture on non-mutter Wayland differently or place via layer.
- Per-window opacity: nothing on mutter at all; `wp_alpha_modifier_v1`
  covers KWin 6.7+/Hyprland/sway/river ([support table](https://wayland.app/protocols/alpha-modifier-v1)).

### B. Drag-out of a winit window — per-OS by necessity, X11 is the orphan
- winit 0.30 has no outgoing DnD ([issue #3229](https://github.com/rust-windowing/winit/issues/3229)).
- **Good news:** winit PR [#4571](https://github.com/rust-windowing/winit/pull/4571)
  (merged Jul 2026, lands in the next release) adds `ActiveEventLoop::start_drag`
  — implemented for **Windows, macOS, Wayland; X11 is receive-only** and
  out-of-scope. Upgrading is the cheapest path for 3 of 4 targets.
- Independent of that: `drag` crate ([docs.rs](https://docs.rs/drag/latest/drag/))
  works on winit windows for macOS+Windows, but `DragItem::Files` needs a
  file on disk → save-then-drag (fine: our P0 flow saves first anyway).
  On Linux it's GTK-only (`&gtk::ApplicationWindow`) — unusable on winit.
- True lazy promises (in-memory image, materialize on drop):
  macOS `NSFilePromiseProvider` = M, all bindings already vendored in
  objc2-app-kit 0.3.2; Windows FILEDESCRIPTOR/FILECONTENTS = L.
- X11 outgoing XDND: **XL**. winit's own X connection owns the window and
  drops foreign ClientMessages, so the drag source must be a separate
  x11rb-owned window + pointer grab + XdndStatus/Position/Finished handling.
  Recommendation: ship X11 without drag-out (context-menu "Copy file" /
  "Save as" fallback), or wait to see if upstream ever implements X11 send.

### C. Wayland global hotkeys don't exist — conflict UI is moot there
`global-hotkey` dispatches to an X11-only impl on Linux
(`platform_impl/mod.rs`); under Wayland its grabs never fire and — worse —
`register()` returns Ok (dead thread → `rx.recv()` error is swallowed,
x11/mod.rs:44-53). Detect `WAYLAND_DISPLAY` (host.rs:444 already does)
and render the settings row as informational, not conflicted.
The `org.freedesktop.portal.GlobalShortcuts` portal is the "right" path
but effectively KDE-only today.

### D. Portal backend re-creates a session per `capture_all` — scroll capture will hurt on non-GNOME Wayland
`portal.rs:41-161` runs CreateSession→SelectSources→Start→PipeWire
per call. `scroll.rs` polls every 450 ms. On compositors whose portal
honors `persist_mode=2`+`restore_token` (KDE) it's silent but heavy;
on portals without persist (wlroots' xdg-desktop-portal-wlr [?]) the
consent dialog appears **every poll** — unusable. Fix: make scroll (and
record) consume the existing `LiveStream` push stream
(`qs-capture/src/lib.rs:43-48`, `linux/wayland.rs`) instead of re-grabbing.

### E. Popup keyboard focus without focus steal
macOS: solvable — winit borderless windows *can* become key
(`WinitWindow::canBecomeKeyWindow` returns true) and the accessory app
gets keyboard input after `NSApp.activate()`+`makeKey` — the `refocus()`
pattern in `settings.rs:1893-1904` already proves this. The cost is
focus-steal UX bookkeeping (resign on leave, don't activate on pure
hover → "hover grants focus" likely means activate-on-hover on macOS,
which CleanShot accepts). Wayland: impossible by design — keyboard focus
follows the compositor's click semantics; the stack is keyboard-capable
only after a click (or a global `Show stack` shortcut on X11/Win/mac).

## §3 Recommended abstraction seams

Add a small platform layer in `qs-app` (the raw-window-handle pattern is
already established — `widgets.rs:58`, `qs-overlay/src/macos.rs:16`):

```rust
// crates/qs-app/src/plat/mod.rs — NEW
pub enum Session { Mac, Win, X11, Wayland }   // Linux decided at RUNTIME
pub fn session() -> Session;                  // env probe; cfg! won't work on Linux

// per-OS, behind #[cfg] / match session():
pub fn set_window_opacity(win: &Window, a: f32);       // mac alphaValue / Win32 LWA / X11 atom / Wayland=noop
pub fn reduced_motion() -> bool;                     // NSAccessibility / SPI / gsettings
pub fn dock_monitor(el: &ActiveEventLoop, capture: Rect) -> MonitorHandle; // sel's monitor, not primary
pub fn grant_key_focus(win: &Window);                // makeKey dance / SetFocus / _NET_ACTIVE_WINDOW / noop

pub trait DragSource {
    // payload: path on disk now, or bytes + filename for lazy promises
    fn start(&self, win: &Window, payload: DragPayload) -> Result<()>;
}
// impls: macos.rs (NSFilePromiseProvider / drag crate),
//        windows.rs (drag crate / IDropSource),
//        x11.rs (xdnd source — optional, XL),
//        wayland.rs (winit≥0.31 start_drag, else Unsupported)
```

Rules of thumb:
- **Never** `cfg!(target_os = "linux")` to mean X11/Wayland — it's a
  runtime property; all Linux seams need `Session` dispatch.
- Keep winit-version seam behind `DragSource` so the future upgrade
  (winit 0.31 `DataTransfer`) swaps impls without touching UI.
- Layer-shell (wlr/KDE) should be an additive placement strategy
  (`enum Placement { Absolute(Rect), Anchored{output,corner}, Compositor }`),
  not a parallel window stack.

## §4 Suggested build order (de-risk)

1. **macOS** — stack window + keyboard nav (S), TCC onboarding (S, FFI
   done), reduced-motion (S), IconMenuItem (S), conflict states (S).
   Then the two M's: NSFilePromiseProvider drag-out and fly animation.
   macOS has zero hard blockers for the whole spec.
2. **Windows** — same list minus TCC; drag-out via `drag` crate
   (save-then-drag) first, lazy FILEDESCRIPTOR later; pin opacity +
   layered attrs; `SPI_GETCLIENTAREAANIMATION`. All ≤M.
3. **X11** — everything macOS/Windows got except drag-out (defer) and
   minus hover-focus subtleties (focus is straightforward). Opacity via
   `_NET_WM_WINDOW_OPACITY` is S.
4. **Wayland** — *accept* on GNOME: docks land where mutter puts them,
   pins are plain windows, no opacity, fade-not-fly, click-to-focus,
   hotkey stays the `--capture` keybinding story. Spend effort only on:
   (a) switch scroll-capture to the `LiveStream` path so portal users
   don't get per-poll dialogs (§2-D — affects correctness not just UX);
   (b) optionally layer-shell placement for sway/Hyprland/KDE (M–L);
   (c) winit ≥0.31 upgrade when released → free drag-out on Wayland.
