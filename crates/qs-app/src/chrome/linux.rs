//! Linux chrome backend: on Wayland a **layer-shell** corner shelf (desktop
//! overlay, not an app toplevel); on X11 a positioned winit window with
//! skip-taskbar. Pixels go through `wl_shm` on Wayland or XPutImage on X11.
//!
//! Click-through comes from the compositor's own input routing:
//! `wl_surface_set_input_region` / XSHAPE `SK_INPUT` confine delivery to
//! the stack's live zone rects, so moves outside them land on the apps
//! underneath us instead. Unlike the macOS global-monitor trick this
//! happens inside the compositor — no accessibility permission needed.
//!
//! Transparency on X11 requires a running composite manager; on the rare
//! bare setup the window degrades to opaque and stays functional.

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use qs_overlay::sdf::Canvas;
#[cfg(target_os = "linux")]
use winit::platform::wayland::WindowAttributesExtWayland;
#[cfg(target_os = "linux")]
use winit::platform::x11::{WindowAttributesExtX11, WindowType as X11WindowType};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};
use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, Window, WindowId, WindowLevel};

use super::{shelf_logical_size, ChromeCmd, ChromeEv, Corner, FRect, Key, Land, Stack};

/* ==================== window ==================== */

/// One chrome surface per monitor.
pub struct ChromeWin {
    win: Rc<Window>,
    imp: Imp,
    pub stack: Stack,
    /// premul `A<<24|B<<16|G<<8|R` — memory RGBA, the wl_shm ABGR8888 layout
    buf: Vec<u32>,
    sink: RefCell<Option<Rc<dyn Fn(ChromeEv)>>>,
    /// last cursor position in surface-local physical px — MouseInput
    /// carries no position, so Down/Up replay this
    cursor: PhysicalPosition<f64>,
    /// left button held — CursorMoved becomes Dragged, not Move
    held: bool,
    /// ctrl held — turns C into Copy (no menu bar on Linux to do it)
    mods: ModifiersState,
    mon_key: String,
    /// logical size/origin of this monitor
    lw: f64,
    lh: f64,
    mx: f64,
    my: f64,
    scale: f64,
    font: Rc<fontdue::Font>,
    accent: u32,
    /// macOS paints before `setVisible:` — map only after the first
    /// frame with stack pixels so GNOME doesn't flash a fullscreen black
    /// layer while the buffer is still empty.
    shown: bool,
    /// Wayland layer-shell: logical shelf size is fixed; winit only holds
    /// a 1×1 handle for WindowId (not the visible surface).
    fixed_shelf: bool,
    /// macOS-style full-monitor overlay (gtk-layer-shell on GNOME).
    full_monitor: bool,
    /// Pixels go to winit's surface — must map the window (GNOME has no zwlr).
    winit_maps: bool,
}

/// The presenter — chosen once from the raw handle; Wayland marshals
/// surface requests over winit's own wl_display, X11 paints through a
/// private xcb connection.
enum Imp {
    Wl(Wl),
    Gtk(super::gtk_layer::GtkLayer),
    X11(X11),
    /// GTK utility overlay on the tray thread (GNOME — no "Unknown" app).
    GtkAlbum(super::gtk_album::GtkAlbum),
}

/// Physical outer position for the shelf panel on `mon`.
fn shelf_outer_pos(
    mon: &MonitorHandle,
    corner: Corner,
    sw: f64,
    sh: f64,
) -> PhysicalPosition<f64> {
    let sf = mon.scale_factor();
    let mp = mon.position();
    let ms = mon.size();
    let side = 20.0 * sf;
    let top = 20.0 * sf;
    let x = if corner.right() {
        mp.x as f64 + ms.width as f64 - sw * sf - side
    } else if corner == Corner::TC || corner == Corner::BC {
        mp.x as f64 + (ms.width as f64 - sw * sf) / 2.0
    } else {
        mp.x as f64 + side
    };
    let y = if corner.bottom() {
        mp.y as f64 + ms.height as f64 - sh * sf - side
    } else {
        mp.y as f64 + top
    };
    PhysicalPosition::new(x, y)
}

thread_local! {
    /// One hidden winit handle for GTK album stacks — avoids a wl_surface per
    /// monitor (GNOME protocol errors after overlay teardown).
    static GTK_HANDLE: RefCell<Option<Rc<Window>>> = const { RefCell::new(None) };
}

fn gtk_winit_handle(el: &ActiveEventLoop) -> Result<Rc<Window>> {
    GTK_HANDLE.with(|h| {
        if let Some(w) = h.borrow().as_ref() {
            return Ok(w.clone());
        }
        let attrs = Window::default_attributes()
            .with_title("quickshort-chrome")
            .with_decorations(false)
            .with_resizable(false)
            .with_visible(false)
            .with_active(false)
            .with_inner_size(LogicalSize::new(2.0, 2.0));
        let win = Rc::new(
            el.create_window(attrs)
                .context("chrome gtk winit handle")?,
        );
        *h.borrow_mut() = Some(win.clone());
        Ok(win)
    })
}

fn winit_chrome_window(
    el: &ActiveEventLoop,
    mon: &MonitorHandle,
    corner: Corner,
    sw: f64,
    sh: f64,
) -> Result<Rc<Window>> {
    let outer = shelf_outer_pos(mon, corner, sw, sh);
    let mut attrs = Window::default_attributes()
        .with_title("Quickshort")
        .with_transparent(true)
        .with_decorations(false)
        .with_resizable(false)
        .with_visible(false)
        .with_active(false)
        .with_window_level(WindowLevel::AlwaysOnTop)
        .with_inner_size(LogicalSize::new(sw, sh))
        .with_position(outer);
    #[cfg(wayland_platform)]
    {
        use winit::platform::wayland::WindowAttributesExtWayland as _;
        attrs = attrs.with_name("quickshort", "quickshort-chrome");
    }
    #[cfg(x11_platform)]
    {
        use winit::platform::x11::WindowAttributesExtX11 as _;
        attrs = attrs.with_x11_window_type(vec![
            X11WindowType::Utility,
            X11WindowType::Notification,
        ]);
        attrs = attrs.with_name("quickshort", "quickshort-chrome");
    }
    Ok(Rc::new(el.create_window(attrs).context("chrome window")?))
}

fn try_gtk_album(
    mon: &MonitorHandle,
    corner: Corner,
    sw: f64,
    sh: f64,
    scale: f64,
) -> Result<super::gtk_album::GtkAlbum> {
    if std::env::var("WAYLAND_DISPLAY")
        .ok()
        .is_none_or(|s| s.is_empty())
    {
        anyhow::bail!("not wayland");
    }
    let out = mon.name();
    let pos = mon.position();
    super::gtk_album::GtkAlbum::new(
        corner,
        sw,
        sh,
        scale,
        out.clone(),
        pos.x,
        pos.y,
    )
}

impl ChromeWin {
    pub(super) fn new(
        el: &ActiveEventLoop,
        mon: &MonitorHandle,
        corner: Corner,
        font: Rc<fontdue::Font>,
        accent: u32,
    ) -> Result<Self> {
        let sf = mon.scale_factor();
        let pos = mon.position();
        let ms = mon.size();
        let (mlw, mlh) = (ms.width as f64 / sf, ms.height as f64 / sf);
        let (sw, sh) = shelf_logical_size();
        let (win, imp) = match try_gtk_album(mon, corner, sw, sh, sf) {
            Ok(album) => (gtk_winit_handle(el)?, Imp::GtkAlbum(album)),
            Err(e) => {
                eprintln!("chrome: gtk-album unavailable ({e:#}) — wl/x11 fallback");
                let win = winit_chrome_window(el, mon, corner, sw, sh)?;
                (win.clone(), Imp::new(&win, mon, corner, mlw, mlh, sw, sh, sf)?)
            }
        };
        if let Imp::GtkAlbum(a) = &imp {
            a.bind_winit(win.id());
        }
        let gtk_shelf = matches!(imp, Imp::GtkAlbum(_));
        let fixed_shelf = imp.layer_mode();
        let winit_maps = matches!(&imp, Imp::X11(_));
        // macOS / layer-shell: full-monitor canvas. GNOME gtk-album: shelf only.
        let mut full_monitor = if gtk_shelf {
            false
        } else {
            matches!(imp, Imp::Gtk(_)) || fixed_shelf || winit_maps
        };
        let mut sf = sf;
        if winit_maps {
            win.set_fullscreen(Some(Fullscreen::Borderless(Some(mon.clone()))));
        }
        let (mut lw, mut lh) = if full_monitor {
            (mlw, mlh)
        } else {
            let size = win.inner_size();
            (size.width as f64 / sf, size.height as f64 / sf)
        };
        if let Imp::Gtk(g) = &imp {
            lw = g.lw;
            lh = g.lh;
            sf = g.scale;
        }
        if let Imp::GtkAlbum(g) = &imp {
            lw = g.lw;
            lh = g.lh;
            sf = g.scale;
        }
        let (pw, ph) = (
            (lw * sf).round().max(1.0) as usize,
            (lh * sf).round().max(1.0) as usize,
        );
        let (mx, my) = if gtk_shelf {
            let outer = shelf_outer_pos(mon, corner, sw, sh);
            (outer.x / sf, outer.y / sf)
        } else {
            (pos.x as f64 / sf, pos.y as f64 / sf)
        };

        Ok(Self {
            win,
            imp,
            stack: Stack::new(corner),
            buf: vec![0; pw * ph],
            sink: RefCell::new(None),
            cursor: PhysicalPosition::new(0.0, 0.0),
            held: false,
            mods: ModifiersState::default(),
            mon_key: super::mon_key(mon),
            lw,
            lh,
            mx,
            my,
            scale: sf,
            font,
            accent,
            shown: false,
            fixed_shelf,
            full_monitor,
            winit_maps,
        })
    }

    fn reveal_if_ready(&mut self, painted: bool) {
        // zwlr/gtk layer — winit is only a 1×1 handle; mapping it lands
        // a shelf-sized surface in the compositor's default placement.
        if !self.winit_maps {
            if painted {
                self.shown = true;
            }
            return;
        }
        if !self.shown && painted {
            self.win.set_visible(true);
            self.win.request_redraw();
            self.shown = true;
        }
    }

    pub fn id(&self) -> WindowId {
        self.win.id()
    }

    /// Settings → Appearance: retarget layer-shell anchor (Wayland) or
    /// stack layout (all paths).
    pub(super) fn set_dock_corner(&mut self, corner: Corner) {
        self.stack.set_corner(corner);
        match &mut self.imp {
            Imp::Wl(w) => w.set_layer_corner(corner),
            Imp::Gtk(g) => g.set_layer_corner(corner),
            _ => {}
        }
    }
    pub(super) fn mon_key(&self) -> &str {
        &self.mon_key
    }
    /// Store the host's event sink — `land` binds it once the real
    /// WindowId exists. RefCell because this is `&self`.
    pub(super) fn set_sink(&self, sink: Rc<dyn Fn(ChromeEv)>) {
        *self.sink.borrow_mut() = Some(sink);
    }
    pub fn stack(&self) -> &Stack {
        &self.stack
    }
    pub fn stack_mut(&mut self) -> &mut Stack {
        &mut self.stack
    }

    fn emit(&self, ev: ChromeEv) {
        if let Some(s) = self.sink.borrow().clone() {
            s(ev);
        }
    }

    /// Render the stack into the buffer and push it to the window.
    /// `paint_bounds` caps the work to the dirty region — a fullscreen
    /// copy + full-surface damage per frame was the animation jank.
    fn present(&mut self) {
        let now = Instant::now();
        // track size/scale drift (fractional-scale flips, mode changes)
        // here instead of a dedicated resize path - buffers simply
        // follow inner_size each frame
        self.scale = self.win.scale_factor();
        if !self.fixed_shelf && !self.full_monitor {
            let size = self.win.inner_size();
            let (pw, ph) = (size.width.max(1) as usize, size.height.max(1) as usize);
            self.lw = pw as f64 / self.scale;
            self.lh = ph as f64 / self.scale;
        }
        let (pw, ph) = (
            (self.lw * self.scale).round().max(1.0) as usize,
            (self.lh * self.scale).round().max(1.0) as usize,
        );
        if self.buf.len() != pw * ph {
            self.buf = vec![0; pw * ph];
        }
        let bounds = self.stack.paint_bounds(self.lw, self.lh, now);
        if let Some(b) = bounds {
            let s = self.scale;
            let y0 = ((b.y * s).floor() as usize).min(ph);
            let y1 = (((b.y + b.h) * s).ceil() as usize).min(ph);
            for row in &mut self.buf[y0.min(y1) * pw..y1 * pw] {
                *row = 0;
            }
        } else {
            self.buf.fill(0);
        }
        {
            let mut cv = Canvas {
                buf: &mut self.buf,
                w: pw,
                h: ph,
                scale: self.scale,
            };
            let font = self.font.clone();
            self.stack.render(&mut cv, &font, self.accent, now);
        }
        // zone rects (not input_rects): the wider retention zone is the
        // hysteresis that keeps hover alive between moving thumbs
        let rects = self.stack.zone_rects(self.lw, self.lh);
        // Wayland: arm winit's wl_surface.frame callback before our raw
        // commit. Without it FrameCallbackState stays None and every
        // request_redraw delivers RedrawRequested immediately - an
        // unthrottled busy-loop instead of vsync-paced animation.
        // dirty in physical px for the backends
        let dirty = bounds.map(|b| FRect {
            x: b.x * self.scale,
            y: b.y * self.scale,
            w: b.w * self.scale,
            h: b.h * self.scale,
        });
        if self.fixed_shelf {
            if let Imp::Wl(w) = &mut self.imp {
                w.sync_layer_keyboard(self.stack.modal_open());
            }
        }
        let show = self.shown || bounds.is_some() || self.stack.animating();
        self.imp
            .present(&self.buf, pw, ph, &rects, self.scale, dirty, show);
        self.reveal_if_ready(bounds.is_some() || self.stack.animating());
    }

    /// Fly a finished shot into this monitor's stack.
    pub(super) fn land(&mut self, l: Land) {
        let from = l
            .sel
            .map(|r| FRect {
                x: r.x as f64 - self.mx,
                y: r.y as f64 - self.my,
                w: r.w as f64,
                h: r.h as f64,
            })
            .unwrap_or(FRect {
                x: self.lw / 2.0 - 60.0,
                y: self.lh / 2.0 - 40.0,
                w: 120.0,
                h: 80.0,
            });
        let rm = l.instant || !self.full_monitor;
        let scale = self.scale;
        self.stack
            .land(l.img, l.kind, l.path, from, rm, scale, l.dur_s, l.shared);
        if self.winit_maps {
            if !self.shown {
                self.win.set_visible(true);
                self.shown = true;
            }
            self.win.request_redraw();
        } else {
            self.present();
        }
    }

    fn apply_wl_input(&mut self, ev: WlChromeInput) {
        match ev {
            WlChromeInput::Move { x, y } => {
                self.cursor = PhysicalPosition::new(x * self.scale, y * self.scale);
                self.emit(if self.held {
                    ChromeEv::Dragged { x, y }
                } else {
                    ChromeEv::Move { x, y }
                });
            }
            WlChromeInput::Leave => {
                self.held = false;
                self.emit(ChromeEv::Leave);
            }
            WlChromeInput::Down { x, y } => {
                self.held = true;
                self.emit(ChromeEv::Down { x, y });
            }
            WlChromeInput::Up { x, y } => {
                self.held = false;
                self.emit(ChromeEv::Up { x, y });
            }
            WlChromeInput::RightDown { x, y } => {
                self.emit(ChromeEv::RightDown { x, y });
            }
            WlChromeInput::Key(k) => self.emit(ChromeEv::Key(k)),
        }
    }

    /// Platform event → stack → optional host command. Redraw is handled
    /// by the caller (Chrome::event) via `redraw`.
    pub fn event(&mut self, ev: ChromeEv) -> Option<ChromeCmd> {
        let cmd = match ev {
            ChromeEv::Key(k) => self.stack.key(k),
            e => self.stack.pointer(e, self.lw, self.lh),
        };
        match cmd {
            // a real wl_data_source drag-out is a later phase — until
            // then the drag simply dies here instead of reaching the host
            Some(ChromeCmd::DragBegin { .. }) => None,
            c => c,
        }
    }

    pub fn redraw(&mut self) {
        self.present();
    }

    /// winit window event — translate to ChromeEv for the sink, repaint
    /// on request.
    pub fn window_event(&mut self, ev: &WindowEvent) -> bool {
        match ev {
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = *position;
                let (x, y) = (position.x / self.scale, position.y / self.scale);
                // held + moved = a pending drag; the stack applies the
                // 4px threshold so wobble doesn't escalate
                self.emit(if self.held {
                    ChromeEv::Dragged { x, y }
                } else {
                    ChromeEv::Move { x, y }
                });
            }
            WindowEvent::CursorLeft { .. } => {
                self.held = false;
                self.emit(ChromeEv::Leave);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let (x, y) = (self.cursor.x / self.scale, self.cursor.y / self.scale);
                match (state, button) {
                    (ElementState::Pressed, MouseButton::Left) => {
                        // grab keyboard focus on first real interaction
                        // (a no-op on Wayland — the compositor focuses
                        // fullscreen surfaces itself)
                        self.win.focus_window();
                        self.held = true;
                        self.emit(ChromeEv::Down { x, y });
                    }
                    (ElementState::Released, MouseButton::Left) => {
                        self.held = false;
                        self.emit(ChromeEv::Up { x, y });
                    }
                    (ElementState::Pressed, MouseButton::Right) => {
                        self.win.focus_window();
                        self.emit(ChromeEv::RightDown { x, y });
                    }
                    _ => {}
                }
            }
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let key = match &event.logical_key {
                    WKey::Named(NamedKey::Escape) => Some(Key::Esc),
                    WKey::Named(NamedKey::ArrowLeft) => Some(Key::Left),
                    WKey::Named(NamedKey::ArrowRight) => Some(Key::Right),
                    WKey::Named(NamedKey::Enter) => Some(Key::Enter),
                    WKey::Named(NamedKey::Backspace) => Some(Key::Back),
                    WKey::Named(NamedKey::Space) => Some(Key::Space),
                    WKey::Character(c) if self.mods.control_key() && c.as_str() == "c" => {
                        Some(Key::Copy)
                    }
                    WKey::Character(c) if !self.mods.control_key() && c.as_str() == "o" => {
                        Some(Key::Open)
                    }
                    WKey::Character(c) if !self.mods.control_key() && c.as_str() == "s" => {
                        Some(Key::Share)
                    }
                    _ => None,
                };
                if let Some(k) = key {
                    self.emit(ChromeEv::Key(k));
                }
            }
            WindowEvent::RedrawRequested => {
                // Wayland: winit gates this event on wl_surface.frame —
                // it IS the vsync tick, so settle+present+chain. X11
                // gets the event promptly; the pump's timer drives it.
                if matches!(self.imp, Imp::Wl(_)) {
                    self.stack.settle(Instant::now());
                    self.present();
                    if self.stack.animating() {
                        self.win.request_redraw();
                    }
                } else {
                    self.present();
                }
            }
            _ => {}
        }
        true // the window exists; nothing else needs handling
    }

    /// Animation driver — Wayland ticks on the compositor's frame
    /// callback (winit throttles RedrawRequested to it), X11 has no
    /// reliable vsync so it keeps the 16ms timer.
    pub fn pump(&mut self) -> Option<Instant> {
        if let Imp::Wl(wl) = &mut self.imp {
            wl.dispatch();
            for ev in wl.take_inputs() {
                self.apply_wl_input(ev);
            }
        }
        let now = Instant::now();
        let was = self.stack.animating();
        self.stack.settle(now); // may kick off the idle-hide tween
        match &self.imp {
            Imp::Wl(w) if self.fixed_shelf && !w.is_live() => {
                // layer-shell still wiring — keep trying each tick
                Some(now + Duration::from_millis(16))
            }
            Imp::Wl(_) if self.fixed_shelf => {
                if was || self.stack.animating() || !self.shown {
                    self.present();
                }
                if self.stack.animating() {
                    Some(now + Duration::from_millis(16))
                } else {
                    self.stack.idle_deadline()
                }
            }
            Imp::Wl(_) => {
                if was || self.stack.animating() || !self.shown {
                    self.present();
                }
                if self.stack.animating() {
                    self.win.request_redraw();
                    Some(now + Duration::from_millis(16))
                } else {
                    self.stack.idle_deadline()
                }
            }
            Imp::GtkAlbum(_) | Imp::Gtk(_) | Imp::X11(_) => {
                // present while flying, plus the one settle that lands
                // on the end-state — else the tween freezes a frame shy
                if was || self.stack.animating() {
                    self.present();
                }
                if self.stack.animating() {
                    Some(now + Duration::from_millis(16))
                } else {
                    self.stack.idle_deadline()
                }
            }
        }
    }

    /// unused outside macOS — the host cfg-gates the call site
    pub fn view_ptr(&self) -> *const c_void {
        std::ptr::null()
    }
}

impl Imp {
    fn new(
        win: &Window,
        mon: &MonitorHandle,
        corner: Corner,
        mlw: f64,
        mlh: f64,
        sw: f64,
        sh: f64,
        scale: f64,
    ) -> Result<Self> {
        match win
            .window_handle()
            .map_err(|e| anyhow!("chrome handle: {e}"))?
            .as_raw()
        {
            RawWindowHandle::Wayland(h) => {
                let RawDisplayHandle::Wayland(dh) = win
                    .display_handle()
                    .map_err(|e| anyhow!("chrome display: {e}"))?
                    .as_raw()
                else {
                    anyhow::bail!("wayland surface without wayland display");
                };
                let display = dh.display.as_ptr();
                let out = mon.name();
                match Wl::new_layer_foreign(display, corner, mlw, mlh, scale, out.as_deref()) {
                    Ok(w) => {
                        eprintln!("chrome: layer-shell on winit display");
                        return Ok(Self::Wl(w));
                    }
                    Err(e) => eprintln!("chrome: foreign layer: {e:#}"),
                }
                match Wl::new_layer(corner, mlw, mlh, scale, out.as_deref()) {
                    Ok(w) => return Ok(Self::Wl(w)),
                    Err(e) => eprintln!("chrome: env layer: {e:#}"),
                }
                anyhow::bail!("wayland chrome: gtk album should be created before winit window");
            }
            RawWindowHandle::Xlib(h) => X11::new(h.window as u32).map(Self::X11),
            RawWindowHandle::Xcb(h) => X11::new(h.window.get()).map(Self::X11),
            other => anyhow::bail!("chrome: unsupported linux display backend {other:?}"),
        }
    }

    fn layer_mode(&self) -> bool {
        matches!(self, Self::Wl(w) if w.layer_mode())
    }

    fn present(
        &mut self,
        buf: &[u32],
        pw: usize,
        ph: usize,
        rects: &[FRect],
        scale: f64,
        dirty: Option<FRect>,
        visible: bool,
    ) {
        match self {
            Self::Wl(w) => w.present(buf, pw, ph, rects, dirty),
            Self::Gtk(g) => g.present(buf, pw, ph, rects, scale, dirty),
            Self::GtkAlbum(g) => g.present(buf, pw, ph, rects, scale, dirty, visible),
            Self::X11(x) => x.present(buf, pw, ph, rects, scale, dirty),
        }
    }
}

/* ==================== wayland ==================== */

use std::os::fd::AsFd;
use std::os::unix::io::{AsRawFd, BorrowedFd, RawFd};

use wayland_client::protocol::{
    wl_buffer::{self, WlBuffer},
    wl_compositor::{self, WlCompositor},
    wl_keyboard::{self, WlKeyboard},
    wl_output::{self, WlOutput},
    wl_pointer::{self, WlPointer},
    wl_region::{self, WlRegion},
    wl_registry::{self, WlRegistry},
    wl_seat::{self, Capability, WlSeat},
    wl_shm::{self, WlShm},
    wl_shm_pool::{self, WlShmPool},
    wl_surface::{self, WlSurface},
};
use wayland_client::{Connection as WlConn, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};
use wayland_sys::client::{wayland_client_handle, wl_proxy};

/// Pointer events from a zwlr_layer_surface (winit doesn't route these).
enum WlChromeInput {
    Move { x: f64, y: f64 },
    Leave,
    Down { x: f64, y: f64 },
    Up { x: f64, y: f64 },
    RightDown { x: f64, y: f64 },
    Key(Key),
}

/// Linux evdev scancode (wl keycode = evdev + 8).
fn wl_keycode(code: u32) -> Option<Key> {
    match code {
        9 => Some(Key::Esc),
        22 => Some(Key::Back),
        36 => Some(Key::Enter),
        113 => Some(Key::Left),
        114 => Some(Key::Right),
        65 => Some(Key::Space),
        _ => None,
    }
}

/// wl_surface request opcodes (the core protocol is wire-stable).
const SURFACE_ATTACH: u32 = 1;
const SURFACE_DAMAGE: u32 = 2;
const SURFACE_SET_INPUT_REGION: u32 = 5;
const SURFACE_COMMIT: u32 = 6;
/// damage_buffer arrived at v4; winit binds surfaces at v4+ so buffer
/// coordinates are safe — but we still gate on the proxy version.
const SURFACE_DAMAGE_BUFFER: u32 = 9;

/// Requests destined for winit's wl_surface are marshalled raw: the
/// proxy lives on winit's event queue and adopting it via
/// `Backend::manage_object` would clobber winit's dispatch state. Our
/// own objects (shm buffers, the input region) ride on a private event
/// queue on the same wl_display — marshalling *to* a foreign proxy is
/// safe; only receiving events for it isn't.
///
/// The calls go through `wl_proxy_marshal` via the dlopen handle (winit
/// already loads libwayland-client for the window itself).
struct Wl {
    surface: *mut wl_proxy,
    conn: WlConn,
    queue: EventQueue<WlState>,
    state: WlState,
    /// Desktop overlay — not winit's xdg-toplevel surface.
    layer_mode: bool,
    /// Layer surface configured — safe to attach buffers.
    live: bool,
}

fn layer_anchor_full() -> Anchor {
    Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right
}

fn layer_anchor(corner: Corner) -> Anchor {
    let mut a = Anchor::empty();
    match corner {
        Corner::TL | Corner::TC | Corner::TR => a |= Anchor::Top,
        Corner::BL | Corner::BC | Corner::BR => a |= Anchor::Bottom,
    }
    match corner {
        Corner::TL | Corner::BL => a |= Anchor::Left,
        Corner::TR | Corner::BR => a |= Anchor::Right,
        _ => {}
    }
    a
}

fn layer_margins(corner: Corner) -> (i32, i32, i32, i32) {
    let m = 20;
    match corner {
        Corner::TR => (m, m, 0, 0),
        Corner::TL => (m, 0, 0, m),
        Corner::BR => (0, m, m, 0),
        Corner::BL => (0, 0, m, m),
        Corner::TC => (m, 0, 0, 0),
        Corner::BC => (0, 0, m, 0),
    }
}

/// One pooled shm buffer. `busy` flips off on wl_buffer.release — the
/// compositor is done reading and the slot can be rewritten. `dead`
/// marks a stale-size slot slated for teardown once released.
struct WlBuf {
    fd: RawFd,
    size: usize,
    w: i32,
    h: i32,
    pool: WlShmPool,
    buf: WlBuffer,
    /// the pixel conversion the buffer's format demands
    argb: bool,
    busy: bool,
    dead: bool,
    /// Physical-px rows dirtied since this slot was last written —
    /// slots alternate between frames, so a partial write must cover
    /// the catch-up union or the stale bands ghost.
    catch_up: Option<FRect>,
}

/// wayland-client bookkeeping state bound to our private event queue —
/// `WlBuffer::Release` is the only event we act on; every other object
/// is bound with a `()` udata and a noop dispatch.
#[derive(Default)]
struct WlState {
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    layer_shell: Option<ZwlrLayerShellV1>,
    layer_surface: Option<ZwlrLayerSurfaceV1>,
    chrome_surface: Option<WlSurface>,
    outputs: Vec<(WlOutput, String)>,
    seat: Option<WlSeat>,
    pointer: Option<WlPointer>,
    keyboard: Option<WlKeyboard>,
    /// formats the compositor advertised on wl_shm — argb8888/xrgb8888
    /// are the only guaranteed ones; abgr8888 (our native packing) is
    /// optional and weston refuses create_buffer with it
    shm_formats: Vec<wl_shm::Format>,
    bufs: Vec<Option<WlBuf>>,
    /// slot indices released by the server since the last dispatch
    released: Vec<usize>,
    inputs: Vec<WlChromeInput>,
    pointer_scale: f64,
    pointer_x: f64,
    pointer_y: f64,
    layer_configured: bool,
    /// registry globals seen (debug when layer-shell is missing)
    wl_globals: Vec<String>,
}

impl Wl {
    fn layer_mode(&self) -> bool {
        self.layer_mode
    }

    fn is_live(&self) -> bool {
        !self.layer_mode || self.live
    }

    fn take_inputs(&mut self) -> Vec<WlChromeInput> {
        std::mem::take(&mut self.state.inputs)
    }

    fn set_layer_corner(&mut self, corner: Corner) {
        if !self.layer_mode {
            return;
        }
        if let Some(layer) = &self.state.layer_surface {
            let _ = corner;
            layer.set_anchor(layer_anchor_full());
            layer.set_margin(0, 0, 0, 0);
            let _ = self.conn.flush();
        }
    }

    fn sync_layer_keyboard(&mut self, modal: bool) {
        if !self.layer_mode {
            return;
        }
        if let Some(layer) = &self.state.layer_surface {
            let ki = if modal {
                KeyboardInteractivity::Exclusive
            } else {
                KeyboardInteractivity::OnDemand
            };
            layer.set_keyboard_interactivity(ki);
            let _ = self.conn.flush();
        }
    }

    /// Present on winit's wl_surface (corner shelf window) when layer-shell
    /// isn't offered — no roundtrip (winit already owns the registry).
    fn new_foreign(display: *mut c_void, surface: *mut c_void) -> Result<Self> {
        let backend =
            unsafe { wayland_backend::client::Backend::from_foreign_display(display.cast()) };
        let conn = WlConn::from_backend(backend);
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        let registry = conn.display().get_registry(&qh, ());
        let mut state = WlState::default();
        queue
            .roundtrip(&mut state)
            .context("foreign wl registry")?;
        drop(registry);
        if state.compositor.is_none() || state.shm.is_none() {
            anyhow::bail!("foreign wl missing compositor/shm");
        }
        eprintln!("chrome: winit shelf present (no layer-shell)");
        Ok(Self {
            surface: surface as *mut wl_proxy,
            conn,
            queue,
            state,
            layer_mode: false,
            live: true,
        })
    }

    fn new_layer_foreign(
        display: *mut c_void,
        corner: Corner,
        lw: f64,
        lh: f64,
        scale: f64,
        output_name: Option<&str>,
    ) -> Result<Self> {
        let backend =
            unsafe { wayland_backend::client::Backend::from_foreign_display(display.cast()) };
        let conn = WlConn::from_backend(backend);
        Self::finish_layer(conn, corner, lw, lh, scale, output_name)
    }

    /// zwlr_layer_shell on a **separate** socket — try when foreign display
    /// doesn't advertise the global (some compositors are picky).
    fn new_layer(
        corner: Corner,
        lw: f64,
        lh: f64,
        scale: f64,
        output_name: Option<&str>,
    ) -> Result<Self> {
        let conn = WlConn::connect_to_env().context("WAYLAND_DISPLAY for album layer")?;
        Self::finish_layer(conn, corner, lw, lh, scale, output_name)
    }

    fn finish_layer(
        conn: WlConn,
        corner: Corner,
        lw: f64,
        lh: f64,
        scale: f64,
        output_name: Option<&str>,
    ) -> Result<Self> {
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        let registry = conn.display().get_registry(&qh, ());
        let mut state = WlState {
            pointer_scale: scale,
            ..WlState::default()
        };
        queue
            .roundtrip(&mut state)
            .context("layer wl registry")?;
        let _ = queue.roundtrip(&mut state);
        drop(registry);
        let compositor = state
            .compositor
            .as_ref()
            .context("layer: no wl_compositor")?;
        let layer_shell = state.layer_shell.as_ref().with_context(|| {
            format!(
                "layer: no zwlr_layer_shell_v1 (compositor={}, shm={}, outputs={}, globals=[{}])",
                state.compositor.is_some(),
                state.shm.is_some(),
                state.outputs.len(),
                state.wl_globals.join(", ")
            )
        })?;
        let output = output_name.and_then(|want| {
            state
                .outputs
                .iter()
                .find(|(_, n)| n == want)
                .map(|(o, _)| o)
        });
        if output_name.is_some() && output.is_none() {
            eprintln!(
                "chrome: wl_output {:?} not found — anchoring layer to default output",
                output_name
            );
        }
        let surface = compositor.create_surface(&qh, ());
        let layer = layer_shell.get_layer_surface(
            &surface,
            output,
            Layer::Overlay,
            "quickshort-album".into(),
            &qh,
            (),
        );
        // Full output like macOS — stack layout picks the dock corner.
        layer.set_anchor(layer_anchor_full());
        layer.set_margin(0, 0, 0, 0);
        layer.set_keyboard_interactivity(KeyboardInteractivity::OnDemand);
        let pw = (lw * scale).round().max(1.0) as u32;
        let ph = (lh * scale).round().max(1.0) as u32;
        layer.set_size(pw, ph);
        state.layer_surface = Some(layer);
        state.chrome_surface = Some(surface);
        if let Some(seat) = state.seat.as_ref() {
            seat.get_pointer(&qh, ());
            seat.get_keyboard(&qh, ());
        }
        queue
            .roundtrip(&mut state)
            .context("layer surface configure")?;
        if !state.layer_configured {
            anyhow::bail!("layer surface never configured");
        }
        let surface_ptr = state
            .chrome_surface
            .as_ref()
            .context("layer surface missing")?
            .id()
            .as_ptr() as *mut wl_proxy;
        eprintln!(
            "chrome: layer-shell album docked ({}×{} logical)",
            lw, lh
        );
        Ok(Self {
            surface: surface_ptr,
            conn,
            queue,
            state,
            layer_mode: true,
            live: true,
        })
    }

    /// Non-blocking read + dispatch for our private Wayland connection.
    fn pump_connection(&mut self) {
        let _ = self.conn.flush();
        loop {
            if self.queue.dispatch_pending(&mut self.state).unwrap_or(0) == 0 {
                break;
            }
        }
        if let Some(guard) = self.conn.prepare_read() {
            let fd = {
                let backend = self.conn.backend();
                backend.poll_fd().as_fd().as_raw_fd()
            };
            let mut p = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&mut p, 1, 0) } > 0 && p.revents & libc::POLLIN != 0 {
                let _ = guard.read();
            }
        }
        loop {
            if self.queue.dispatch_pending(&mut self.state).unwrap_or(0) == 0 {
                break;
            }
        }
    }

    /// Drain buffered events — marks released buffers reusable. Called
    /// from `pump` because winit owns the display fd's read path.
    fn dispatch(&mut self) {
        if self.layer_mode {
            self.pump_connection();
        } else if self.queue.dispatch_pending(&mut self.state).is_err() {
            return;
        }
        for i in self.state.released.drain(..) {
            if let Some(Some(b)) = self.state.bufs.get_mut(i) {
                b.busy = false;
            }
        }
        // dead slots go once the server has let go of them — slot
        // indices are wl_buffer udata so slots stay put (`None` until
        // swept) and a late release just finds its slot gone
        let mut i = 0;
        while i < self.state.bufs.len() {
            match &self.state.bufs[i] {
                Some(b) if b.dead && !b.busy => self.state.bufs[i] = None,
                // interior holes get reused by `slot`; trailing holes
                // are swept so the vec can't grow unboundedly
                None if i == self.state.bufs.len() - 1 => {
                    self.state.bufs.truncate(i);
                }
                _ => i += 1,
            }
        }
    }

    /// Reuse or create a pooled slot holding `w×h` pixels; the map +
    /// copy happens here, so attach can run immediately after.
    fn slot(
        &mut self,
        qh: &QueueHandle<WlState>,
        pixels: &[u32],
        w: i32,
        h: i32,
        dirty: Option<FRect>,
    ) -> Option<usize> {
        let need = (w * h) as usize * 4;
        let free = self.state.bufs.iter().position(|b| {
            b.as_ref().map_or(true, |b| {
                !b.busy && !b.dead && b.size == need && b.w == w && b.h == h
            })
        });
        let idx = match free {
            Some(i) => i,
            None => {
                self.state.bufs.push(None);
                self.state.bufs.len() - 1
            }
        };
        let created = self.state.bufs[idx].is_none();
        if created {
            // wrong-size survivors become dead weight — `dispatch` frees
            // each once its release arrives
            for b in self.state.bufs.iter_mut().flatten() {
                if b.size != need || b.w != w || b.h != h {
                    b.dead = true;
                }
            }
            let fd = shm_fd(need)?;
            let pool = self.state.shm.as_ref()?.create_pool(
                unsafe { BorrowedFd::borrow_raw(fd) },
                need as i32,
                qh,
                (),
            );
            // only argb8888/xrgb8888 are spec-guaranteed; our buffer is
            // packed ABGR so that format wins when offered, else we pay
            // an R↔B swap for ARGB
            let fmt = if self.state.shm_formats.contains(&wl_shm::Format::Abgr8888) {
                wl_shm::Format::Abgr8888
            } else {
                wl_shm::Format::Argb8888
            };
            let buf = pool.create_buffer(0, w, h, w * 4, fmt, qh, idx);
            self.state.bufs[idx] = Some(WlBuf {
                fd,
                size: need,
                w,
                h,
                pool,
                buf,
                argb: fmt == wl_shm::Format::Argb8888,
                busy: false,
                dead: false,
                catch_up: None,
            });
        }
        // `dirty` is the physical-px region the caller repainted. This
        // slot's pixels are from whenever it was last written (usually
        // two frames back, slots alternate), so a partial write must
        // cover the union of every dirty band missed since then —
        // tracked in `catch_up`. Just-created slots always full-write
        // (their shm is uninitialized).
        {
            let b = self.state.bufs[idx].as_mut()?;
            match (dirty, b.catch_up.take(), created) {
                (Some(d), catch_up, false) => {
                    let r = catch_up.map_or(d, |c| {
                        let x0 = c.x.min(d.x);
                        let y0 = c.y.min(d.y);
                        FRect {
                            x: x0,
                            y: y0,
                            w: (c.x + c.w).max(d.x + d.w) - x0,
                            h: (c.y + c.h).max(d.y + d.h) - y0,
                        }
                    });
                    let y0 = (r.y.floor() as i32).clamp(0, h) as usize;
                    let y1 = ((r.y + r.h).ceil() as i32).clamp(0, h) as usize;
                    if y1 > y0 {
                        write_shm_rows(b.fd, pixels, b.size, w as usize, y0, y1, b.argb)?;
                    }
                }
                _ => write_shm(b.fd, pixels, b.size, b.argb)?,
            }
            b.busy = true;
        }
        // bands this slot skipped become the siblings' catch-up — each
        // writes them when its turn comes
        for (i, other) in self.state.bufs.iter_mut().enumerate() {
            if i != idx {
                if let Some(o) = other {
                    o.catch_up = match (o.catch_up, dirty) {
                        (Some(c), Some(d)) => Some({
                            let x0 = c.x.min(d.x);
                            let y0 = c.y.min(d.y);
                            FRect {
                                x: x0,
                                y: y0,
                                w: (c.x + c.w).max(d.x + d.w) - x0,
                                h: (c.y + c.h).max(d.y + d.h) - y0,
                            }
                        }),
                        (c, d) => c.or(d),
                    };
                }
            }
        }
        Some(idx)
    }

    fn present(
        &mut self,
        buf: &[u32],
        pw: usize,
        ph: usize,
        rects: &[FRect],
        dirty: Option<FRect>,
    ) {
        if self.layer_mode && (!self.live || self.surface.is_null()) {
            return;
        }
        let (w, h) = (pw as i32, ph as i32);
        let qh = self.queue.handle();
        let Some(idx) = self.slot(&qh, buf, w, h, dirty) else {
            return; // shm setup failed — leave the window transparent
        };
        let bufp = self.state.bufs[idx].as_ref().map(|b| b.buf.id().as_ptr());
        let Some(bufp) = bufp else { return };

        // fresh wl_region per present — the server copies it into the
        // pending input region, so a one-shot object avoids mutating
        // shared state across commits and the giant-subtract trick
        // (±2³⁰ rects) some compositors choke on. Destroy after the
        // request is queued is safe: regions are copied, not referenced.
        if let Some(comp) = &self.state.compositor {
            let region = comp.create_region(&qh, ());
            for r in rects {
                let x0 = r.x.floor() as i32;
                let y0 = r.y.floor() as i32;
                let rw = (r.x + r.w).ceil() as i32 - x0;
                let rh = (r.y + r.h).ceil() as i32 - y0;
                if rw > 0 && rh > 0 {
                    region.add(x0, y0, rw, rh);
                }
            }
            unsafe {
                (wayland_client_handle().wl_proxy_marshal)(
                    self.surface,
                    SURFACE_SET_INPUT_REGION,
                    region.id().as_ptr(),
                );
            }
            region.destroy();
        }
        unsafe {
            let wl = wayland_client_handle();
            // damage_buffer (v4) scopes the repaint to the painted band;
            // fall back to full-surface damage on v3 surfaces — the shm
            // rows beyond the written band are stale either way and the
            // damage hint only costs compositor work, never correctness,
            // because slot() caught up every missed dirty band
            let ver = (wl.wl_proxy_get_version)(self.surface);
            match (ver >= 4, dirty) {
                (true, Some(d)) => {
                    let x0 = d.x.floor() as i32;
                    let y0 = d.y.floor() as i32;
                    let dw = (d.x + d.w).ceil() as i32 - x0;
                    let dh = (d.y + d.h).ceil() as i32 - y0;
                    (wl.wl_proxy_marshal)(
                        self.surface,
                        SURFACE_DAMAGE_BUFFER,
                        x0,
                        y0,
                        dw.max(1),
                        dh.max(1),
                    );
                }
                _ => {
                    (wl.wl_proxy_marshal)(self.surface, SURFACE_DAMAGE, 0i32, 0i32, w, h);
                }
            }
            (wl.wl_proxy_marshal)(self.surface, SURFACE_ATTACH, bufp, 0i32, 0i32);
            (wl.wl_proxy_marshal)(self.surface, SURFACE_COMMIT);
        }
        let _ = self.conn.flush();
    }
}

impl Drop for Wl {
    fn drop(&mut self) {
        // explicit destroys must run while the connection is still up
        // (winit disconnects the display after `exiting`, so Host/Demo
        // clear chrome.wins there); region objects are one-shot now
        for b in self.state.bufs.drain(..).flatten() {
            drop(b);
        }
    }
}

impl Drop for WlBuf {
    fn drop(&mut self) {
        self.buf.destroy();
        self.pool.destroy();
        if self.fd >= 0 {
            unsafe { libc::close(self.fd) };
        }
    }
}

/// memfd first (no unlink dance), `shm_open` for pre-3.17 kernels.
fn shm_fd(size: usize) -> Option<RawFd> {
    unsafe {
        let name = b"qs-chrome\0";
        let mut fd = libc::memfd_create(name.as_ptr().cast(), libc::MFD_CLOEXEC);
        if fd < 0 {
            static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path =
                std::ffi::CString::new(format!("/qs-chrome-{}-{n}", std::process::id())).ok()?;
            fd = libc::shm_open(
                path.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                0o600,
            );
            if fd >= 0 {
                libc::shm_unlink(path.as_ptr()); // stays valid until close
            }
        }
        if fd < 0 || libc::ftruncate(fd, size as libc::off_t) != 0 {
            if fd >= 0 {
                libc::close(fd);
            }
            return None;
        }
        Some(fd)
    }
}

/// Write pixels into the shm fd. `argb` selects the channel order:
/// our buffer is ABGR (A<<24|B<<16|G<<8|R = memory R,G,B,A) while
/// wl_shm Argb8888 wants A<<24|R<<16|G<<8|B — swap R↔B per pixel.
fn write_shm(fd: RawFd, pixels: &[u32], size: usize, argb: bool) -> Option<()> {
    unsafe {
        let p = libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        );
        if p == libc::MAP_FAILED {
            return None;
        }
        let dst = p.cast::<u32>();
        if argb {
            for (i, &px) in pixels.iter().enumerate() {
                let v = px & 0xff00_ff00 | (px >> 16) & 0xff | (px << 16) & 0x00ff_0000;
                dst.add(i).write_unaligned(v);
            }
        } else {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), dst.cast(), pixels.len());
        }
        libc::munmap(p, size);
    }
    Some(())
}

/// Row-banded variant: only rows `[y0, y1)` of a `w`-wide buffer are
/// written — the rest of the shm keeps the previous frame's pixels.
/// Animation frames dirty a corner, not the whole monitor, so this
/// skips most of the shm memcpy that made wl presents expensive.
fn write_shm_rows(
    fd: RawFd,
    pixels: &[u32],
    size: usize,
    w: usize,
    y0: usize,
    y1: usize,
    argb: bool,
) -> Option<()> {
    unsafe {
        let p = libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        );
        if p == libc::MAP_FAILED {
            return None;
        }
        let dst = p.cast::<u32>();
        for y in y0..y1 {
            let row = &pixels[y * w..(y + 1) * w];
            let d = dst.add(y * w);
            if argb {
                for (i, &px) in row.iter().enumerate() {
                    let v = px & 0xff00_ff00 | (px >> 16) & 0xff | (px << 16) & 0x00ff_0000;
                    d.add(i).write_unaligned(v);
                }
            } else {
                std::ptr::copy_nonoverlapping(row.as_ptr(), d.cast(), row.len());
            }
        }
        libc::munmap(p, size);
    }
    Some(())
}

impl Dispatch<WlSurface, ()> for WlState {
    fn event(
        _: &mut Self,
        _: &WlSurface,
        _: wl_surface::Event,
        _: &(),
        _: &WlConn,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<ZwlrLayerShellV1, ()> for WlState {
    fn event(
        _: &mut Self,
        _: &ZwlrLayerShellV1,
        _: zwlr_layer_shell_v1::Event,
        _: &(),
        _: &WlConn,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<ZwlrLayerSurfaceV1, ()> for WlState {
    fn event(
        state: &mut Self,
        layer: &ZwlrLayerSurfaceV1,
        ev: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &WlConn,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure { serial, .. } = ev {
            layer.ack_configure(serial);
            state.layer_configured = true;
        }
    }
}
impl Dispatch<WlSeat, ()> for WlState {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        ev: wl_seat::Event,
        _: &(),
        _: &WlConn,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = ev
        {
            if caps.contains(Capability::Pointer) && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
            }
            if caps.contains(Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            }
        }
    }
}
impl Dispatch<WlKeyboard, ()> for WlState {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        ev: wl_keyboard::Event,
        _: &(),
        _: &WlConn,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Key {
            key,
            state: WEnum::Value(wl_keyboard::KeyState::Pressed),
            ..
        } = ev
        {
            if let Some(k) = wl_keycode(key) {
                state.inputs.push(WlChromeInput::Key(k));
            }
        }
    }
}
impl Dispatch<WlOutput, ()> for WlState {
    fn event(
        state: &mut Self,
        output: &WlOutput,
        ev: wl_output::Event,
        _: &(),
        _: &WlConn,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = ev {
            if let Some((_, n)) = state.outputs.iter_mut().find(|(o, _)| o == output) {
                *n = name.to_string();
            }
        }
    }
}
impl Dispatch<WlPointer, ()> for WlState {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        ev: wl_pointer::Event,
        _: &(),
        _: &WlConn,
        _: &QueueHandle<Self>,
    ) {
        let scale = state.pointer_scale;
        let on_surface = |surf: &WlSurface| {
            state
                .chrome_surface
                .as_ref()
                .is_some_and(|s| s == surf)
        };
        match ev {
            wl_pointer::Event::Enter { surface, surface_x, surface_y, .. } => {
                if on_surface(&surface) {
                    state.pointer_x = surface_x / scale;
                    state.pointer_y = surface_y / scale;
                    state.inputs.push(WlChromeInput::Move {
                        x: state.pointer_x,
                        y: state.pointer_y,
                    });
                }
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                state.pointer_x = surface_x / scale;
                state.pointer_y = surface_y / scale;
                state.inputs.push(WlChromeInput::Move {
                    x: state.pointer_x,
                    y: state.pointer_y,
                });
            }
            wl_pointer::Event::Leave { .. } => state.inputs.push(WlChromeInput::Leave),
            wl_pointer::Event::Button {
                state: WEnum::Value(st),
                button,
                ..
            } => {
                let x = state.pointer_x;
                let y = state.pointer_y;
                match (st, button) {
                    (wl_pointer::ButtonState::Pressed, 0x110) => {
                        state.inputs.push(WlChromeInput::Down { x, y });
                    }
                    (wl_pointer::ButtonState::Released, 0x110) => {
                        state.inputs.push(WlChromeInput::Up { x, y });
                    }
                    (wl_pointer::ButtonState::Pressed, 0x111) => {
                        state.inputs.push(WlChromeInput::RightDown { x, y });
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlRegistry, ()> for WlState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &WlConn,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            state.wl_globals.push(interface.to_string());
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind::<WlCompositor, _, _>(name, 1, qh, ()));
                }
                "wl_shm" => {
                    state.shm = Some(registry.bind::<WlShm, _, _>(name, 1, qh, ()));
                }
                "zwlr_layer_shell_v1" if state.layer_shell.is_none() => {
                    state.layer_shell =
                        Some(registry.bind::<ZwlrLayerShellV1, _, _>(name, 1, qh, ()));
                }
                other if other.contains("layer") => {
                    eprintln!("chrome: wl global {other} (ignored)");
                }
                "wl_seat" if state.seat.is_none() => {
                    state.seat = Some(registry.bind::<WlSeat, _, _>(name, 7, qh, ()));
                }
                "wl_output" => {
                    let o = registry.bind::<WlOutput, _, _>(name, 4, qh, ());
                    state.outputs.push((o, String::new()));
                }
                _ => {}
            }
        }
    }
}

// everything else we bind is event-silent by construction — the only
// signal needed is buffer.release below
impl Dispatch<WlCompositor, ()> for WlState {
    fn event(
        _: &mut Self,
        _: &WlCompositor,
        _: wl_compositor::Event,
        _: &(),
        _: &WlConn,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlShm, ()> for WlState {
    fn event(
        state: &mut Self,
        _: &WlShm,
        ev: wl_shm::Event,
        _: &(),
        _: &WlConn,
        _: &QueueHandle<Self>,
    ) {
        if let wl_shm::Event::Format {
            format: WEnum::Value(f),
        } = ev
        {
            state.shm_formats.push(f);
        }
    }
}
impl Dispatch<WlShmPool, ()> for WlState {
    fn event(
        _: &mut Self,
        _: &WlShmPool,
        _: wl_shm_pool::Event,
        _: &(),
        _: &WlConn,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlRegion, ()> for WlState {
    fn event(
        _: &mut Self,
        _: &WlRegion,
        _: wl_region::Event,
        _: &(),
        _: &WlConn,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlBuffer, usize> for WlState {
    fn event(
        state: &mut Self,
        _proxy: &WlBuffer,
        event: wl_buffer::Event,
        idx: &usize,
        _conn: &WlConn,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            state.released.push(*idx);
        }
    }
}

/* ==================== x11 ==================== */

use x11rb::connection::{Connection as XcbConn, RequestConnection};
use x11rb::protocol::{shape, xproto};
use x11rb::wrapper::ConnectionExt;

/// X11 presenter — a private xcb connection (any client may paint and
/// reshape a foreign window once it knows the XID). winit's
/// `with_transparent` already gave the window a 32-bit ARGB visual.
struct X11 {
    conn: x11rb::rust_connection::RustConnection,
    win: u32,
    gc: u32,
    depth: u8,
    /// staging for the R/B swap — our buf is premul RGBA, X11 ARGB32
    /// pixmaps want native-endian A<<24|R<<16|G<<8|B
    xbuf: Vec<u8>,
    /// put_image max request cap, in bytes of image data per call
    max_band: usize,
}

impl X11 {
    fn new(window: u32) -> Result<Self> {
        let (conn, _) = x11rb::connect(None).context("xcb connect")?;
        let depth = xproto::get_geometry(&conn, window)
            .context("xcb get_geometry")?
            .reply()
            .context("xcb get_geometry reply")?
            .depth;
        let gc = conn.generate_id().context("xcb gen_id")?;
        xproto::create_gc(&conn, gc, window, &xproto::CreateGCAux::new())
            .context("xcb create_gc")?;
        // a fullscreen album shouldn't sit in the taskbar or alt-tab —
        // winit has no X11 flag for this, so append the atoms ourselves
        let wm_state = atom(&conn, "_NET_WM_STATE");
        let skip_tb = atom(&conn, "_NET_WM_STATE_SKIP_TASKBAR");
        let skip_pager = atom(&conn, "_NET_WM_STATE_SKIP_PAGER");
        if wm_state != x11rb::NONE {
            let data: Vec<u32> = [skip_tb, skip_pager]
                .into_iter()
                .filter(|&a| a != x11rb::NONE)
                .collect();
            if !data.is_empty() {
                let _ = conn.change_property32(
                    xproto::PropMode::APPEND,
                    window,
                    wm_state,
                    xproto::AtomEnum::ATOM,
                    &data,
                );
            }
        }
        let _ = conn.flush();
        // PutImage can't exceed the (BigRequests-extended) maximum
        // request length — band the image if the cap kicks in
        let max_band = conn.maximum_request_bytes().saturating_sub(64);
        Ok(Self {
            conn,
            win: window,
            gc,
            depth,
            xbuf: Vec::new(),
            max_band,
        })
    }

    fn present(
        &mut self,
        buf: &[u32],
        pw: usize,
        ph: usize,
        rects: &[FRect],
        scale: f64,
        dirty: Option<FRect>,
    ) {
        // A<<24|B<<16|G<<8|R → A<<24|R<<16|G<<8|B, one pixel per 32-bit
        // unit regardless of window depth (ZPixmap pads 24bpp to 4B).
        // `dirty` (physical px) scopes conversion + upload to the band
        // that actually changed.
        let (x0, y0, x1, y1) = match dirty {
            Some(d) => (
                (d.x.floor() as usize).min(pw),
                (d.y.floor() as usize).min(ph),
                ((d.x + d.w).ceil() as usize).min(pw),
                ((d.y + d.h).ceil() as usize).min(ph),
            ),
            None => (0, 0, pw, ph),
        };
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let dw = x1 - x0;
        self.xbuf.resize(dw * (y1 - y0) * 4, 0);
        let mut i = 0;
        for row in buf[y0 * pw..y1 * pw].chunks_exact(pw) {
            for &px in &row[x0.min(row.len())..(x0 + dw).min(row.len())] {
                let p = px & 0xff00_ff00 | (px >> 16) & 0xff | (px << 16) & 0x00ff_0000;
                self.xbuf[i * 4..i * 4 + 4].copy_from_slice(&p.to_ne_bytes());
                i += 1;
            }
        }
        let rows_per_band = (self.max_band / (dw * 4).max(1)).max(1);
        let mut y = 0usize;
        let dh = y1 - y0;
        while y < dh {
            let rows = rows_per_band.min(dh - y);
            let off = y * dw * 4;
            let _ = xproto::put_image(
                &self.conn,
                xproto::ImageFormat::Z_PIXMAP,
                self.win,
                self.gc,
                dw as u16,
                rows as u16,
                x0 as i16,
                (y0 + y) as i16,
                0,
                self.depth,
                &self.xbuf[off..off + rows * dw * 4],
            );
            y += rows;
        }
        let rects: Vec<xproto::Rectangle> = rects
            .iter()
            .filter_map(|r| {
                // X windows are physical px — scale the logical zone
                let x0 = (r.x * scale).floor() as i32;
                let y0 = (r.y * scale).floor() as i32;
                let x1 = ((r.x + r.w) * scale).ceil() as i32;
                let y1 = ((r.y + r.h) * scale).ceil() as i32;
                let x0 = x0.clamp(0, i16::MAX as i32);
                let y0 = y0.clamp(0, i16::MAX as i32);
                let (w, h) = (x1 - x0, y1 - y0);
                (w > 0 && h > 0).then_some(xproto::Rectangle {
                    x: x0 as i16,
                    y: y0 as i16,
                    width: w.min(u16::MAX as i32) as u16,
                    height: h.min(u16::MAX as i32) as u16,
                })
            })
            .collect();
        // SO::SET + empty rects = click-through-everything; union of
        // zone rects = the hover hysteresis area stays live
        let _ = shape::rectangles(
            &self.conn,
            shape::SO::SET,
            shape::SK::INPUT,
            xproto::ClipOrdering::UNSORTED,
            self.win,
            0,
            0,
            &rects,
        );
        let _ = self.conn.flush();
    }
}

fn atom(conn: &impl RequestConnection, name: &str) -> u32 {
    xproto::intern_atom(conn, false, name.as_bytes())
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| r.atom)
        .unwrap_or(x11rb::NONE)
}
