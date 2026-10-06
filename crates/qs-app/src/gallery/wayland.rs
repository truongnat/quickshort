//! Wayland gallery surface — the palette docked under the top bar.
//!
//! Why this exists: winit windows can't self-position on Wayland (the
//! compositor centers everything, which is what made the gallery look
//! like a dead-center modal). Two surface kinds fix placement:
//!
//! - `Layer`: wlr layer-shell (KDE/sway) anchored top — the ideal
//!   palette surface.
//! - `Full`: GNOME has no layer-shell, so we take the other surface a
//!   compositor MUST position — a maximized xdg toplevel — and paint
//!   the palette top-right inside a transparent ARGB buffer. Unlike
//!   fullscreen, maximize stays on the user's own workspace so the
//!   alpha composites over the real desktop. A press on the
//!   transparent area is our outside-click dismissal (eaten, like a
//!   macOS popover); focus churn during the maximize settle is
//!   ignored, so keyboard-leave only dismisses for `Layer`.
//!
//! It runs its own dispatch thread — winit never sees the surface.
//! Actions round-trip to the host as `HostEvent::Gallery` via the
//! proxy, so the host treats both backends identically.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::Context;
use fontdue::Font;
use qs_overlay::sdf::{rrect_sdf, Canvas};
use winit::event_loop::EventLoopProxy;

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::wl_keyboard::WlKeyboard;
use smithay_client_toolkit::reexports::client::protocol::wl_output::WlOutput;
use smithay_client_toolkit::reexports::client::protocol::wl_pointer::WlPointer;
use smithay_client_toolkit::reexports::client::protocol::wl_seat::WlSeat;
use smithay_client_toolkit::reexports::client::protocol::wl_shm;
use smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface;
use smithay_client_toolkit::reexports::client::{Connection, EventQueue, QueueHandle};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::registry_handlers;
use smithay_client_toolkit::seat::keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers};
use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::xdg::window::{
    Window, WindowConfigure, WindowDecorations, WindowHandler,
};
use smithay_client_toolkit::shell::xdg::XdgShell;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::slot::SlotPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm, delegate_xdg_shell, delegate_xdg_window,
};

use super::{CellData, Hit, Pump, State, Tick, FRINGE, H, W};
use crate::host::HostEvent;
use crate::settings::system_font;
use crate::widgets::Txt;

/// Clearance under the top bar — GNOME's panel is ~32px.
const MARGIN_TOP: i32 = 48;
/// linux/input-event-codes.h — the primary pointer button.
const BTN_LEFT: u32 = 0x110;

/// Owns the dispatch thread; `pump` reports Dead once it unwinds.
pub(crate) struct WlGallery {
    alive: Arc<AtomicBool>,
    _t: JoinHandle<()>,
}

impl WlGallery {
    /// Binds wayland on a worker thread (EventQueue isn't Send, so
    /// setup can't happen here) and reports readiness over a rendezvous.
    /// Err hands the cells back for the winit-window fallback.
    pub fn spawn(
        cells: Vec<CellData>,
        accent: u32,
        proxy: EventLoopProxy<HostEvent>,
    ) -> Result<Self, (anyhow::Error, Vec<CellData>)> {
        let (tx, rx) =
            std::sync::mpsc::sync_channel::<Result<(), (anyhow::Error, Vec<CellData>)>>(0);
        let alive = Arc::new(AtomicBool::new(true));
        let alive2 = alive.clone();
        let t = std::thread::Builder::new()
            .name("gallery-wl".into())
            .spawn(move || run(cells, accent, proxy, alive2, tx))
            .expect("gallery-wl spawn");
        match rx.recv() {
            Ok(Ok(())) => Ok(Self { alive, _t: t }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err((
                anyhow::anyhow!("gallery-wl died during setup"),
                Vec::new(),
            )),
        }
    }

    pub fn pump(&mut self) -> Pump {
        if self.alive.load(Ordering::Relaxed) {
            Pump::Idle
        } else {
            Pump::Dead
        }
    }
}

type Out<T> = Result<T, (anyhow::Error, Vec<CellData>)>;

fn run(
    cells: Vec<CellData>,
    accent: u32,
    proxy: EventLoopProxy<HostEvent>,
    alive: Arc<AtomicBool>,
    tx: std::sync::mpsc::SyncSender<Out<()>>,
) {
    let (mut app, mut queue) = match App::setup(cells, accent, proxy, alive.clone()) {
        Ok(x) => {
            let _ = tx.send(Ok(()));
            x
        }
        Err(e) => {
            let _ = tx.send(Err(e));
            return;
        }
    };
    let qh = queue.handle();
    loop {
        if app.exit || queue.blocking_dispatch(&mut app).is_err() {
            break;
        }
        // every wake (input event or frame callback) advances the state
        // machine; a Busy tick or a dirty flag paints + re-arms the frame
        match app.st.tick(Instant::now()) {
            Tick::Dead => break,
            Tick::Busy => app.draw(&qh),
            Tick::Idle if app.dirty => app.draw(&qh),
            Tick::Idle => {}
        }
    }
    app.alive.store(false, Ordering::Relaxed);
}

/// How the palette reaches its docked position.
enum Surf {
    /// layer-shell: palette-sized surface anchored under the panel.
    Layer(LayerSurface),
    /// GNOME fallback: maximized toplevel; palette is drawn at
    /// `pal_off` inside a transparent buffer, input clipped to it.
    Full(Window),
}

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    shm_state: Shm,
    compositor_state: CompositorState,
    layer_shell: Option<LayerShell>,
    xdg_shell: Option<XdgShell>,

    st: State,
    font: Font,
    pool: SlotPool,
    surface: Option<Surf>,
    /// wl_buffer scale for the next commit
    scale: i32,
    /// Full only: logical surface size the compositor configured
    /// (work-area when maximized, or the palette dims as fallback).
    full_log: (i32, i32),
    /// Full only: palette top-left in logical surface coords.
    pal_off: (f64, f64),
    /// Full only: logical size of the output our surface covers — tells
    /// apart "maximized under the panel" (work area < output) from
    /// "maximized over it" (== output), which decides the dock margin.
    out_log: Option<(i32, i32)>,
    /// entrance clock restarted at first configure (real visibility).
    /// The timestamp also gates outside-press dismissal — the compositor
    /// can deliver a synthetic activation press during the maximize
    /// settle, which must not count as a user click.
    shown_at: Option<Instant>,
    /// Full only: reused RGB canvas the grid paints before the ARGB blit.
    scratch: Vec<u32>,
    /// state changed outside tick-driven animation — paint one frame
    dirty: bool,
    exit: bool,
    proxy: EventLoopProxy<HostEvent>,
    alive: Arc<AtomicBool>,
}

impl App {
    fn setup(
        cells: Vec<CellData>,
        accent: u32,
        proxy: EventLoopProxy<HostEvent>,
        alive: Arc<AtomicBool>,
    ) -> Out<(Self, EventQueue<Self>)> {
        // cells ride in an Option so a wayland failure can hand them
        // back to the caller for the winit fallback
        let mut cells = Some(cells);
        let inner = |cells: &mut Option<Vec<CellData>>,
                     proxy: EventLoopProxy<HostEvent>,
                     alive: Arc<AtomicBool>| -> anyhow::Result<(App, EventQueue<App>)> {
            let conn = Connection::connect_to_env()?;
            let (globals, queue) = registry_queue_init::<App>(&conn)?;
            let qh = queue.handle();
            let compositor = CompositorState::bind(&globals, &qh)?;
            let shm = Shm::bind(&globals, &qh)?;
            // layer-shell is preferred where it exists; GNOME only
            // offers xdg-shell, hence the fullscreen fallback
            let layer_shell = LayerShell::bind(&globals, &qh).ok();
            let xdg_shell = XdgShell::bind(&globals, &qh).ok();
            anyhow::ensure!(
                layer_shell.is_some() || xdg_shell.is_some(),
                "no layer-shell or xdg-shell global"
            );
            let pool = SlotPool::new(64 * 64 * 4, &shm)?;
            let mut app = App {
                registry_state: RegistryState::new(&globals),
                output_state: OutputState::new(&globals, &qh),
                seat_state: SeatState::new(&globals, &qh),
                compositor_state: compositor,
                shm_state: shm,
                layer_shell,
                xdg_shell,
                st: State::new(cells.take().unwrap_or_default(), accent),
                font: system_font()?,
                pool,
                surface: None,
                scale: 1,
                full_log: (0, 0),
                pal_off: (0.0, 0.0),
                out_log: None,
                shown_at: None,
                scratch: Vec::new(),
                dirty: true,
                exit: false,
                proxy,
                alive,
            };
            let wl = app.compositor_state.create_surface(&qh);
            app.surface = Some(match &app.layer_shell {
                Some(shell) => {
                    let ls = shell.create_layer_surface(
                        &qh,
                        wl,
                        Layer::Top,
                        Some("quickshort-gallery"),
                        None, // compositor picks the output — the focused one
                    );
                    ls.set_anchor(Anchor::TOP);
                    ls.set_margin(MARGIN_TOP, 0, 0, 0);
                    // palette semantics: keys without a click; Leave = dismiss
                    ls.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
                    ls.set_size(W as u32, H as u32);
                    ls.commit();
                    Surf::Layer(ls)
                }
                None => {
                    let shell = app.xdg_shell.as_ref().context("xdg-shell absent")?;
                    let win = shell.create_window(wl, WindowDecorations::None, &qh);
                    win.set_app_id("quickshort");
                    // GNOME can't be asked for a position, but it MUST
                    // place a maximized window itself — covering the
                    // work area right under the panel, on the user's
                    // own workspace (unlike fullscreen, which lands on
                    // an empty void). ARGB transparency then shows the
                    // real desktop around the palette.
                    win.set_maximized();
                    win.wl_surface().commit();
                    Surf::Full(win)
                }
            });
            Ok((app, queue))
        };
        match inner(&mut cells, proxy, alive) {
            Ok(r) => Ok(r),
            Err(e) => Err((e, cells.take().unwrap_or_default())),
        }
    }

    fn wl_surface(&self) -> Option<&WlSurface> {
        match self.surface.as_ref()? {
            Surf::Layer(l) => Some(l.wl_surface()),
            Surf::Full(w) => Some(w.wl_surface()),
        }
    }

    /// Paint one frame into a fresh shm slot; the frame callback the
    /// commit carries keeps animation vsync-paced.
    fn draw(&mut self, qh: &QueueHandle<Self>) {
        match self.surface.as_ref() {
            Some(Surf::Layer(_)) => self.draw_layer(qh),
            Some(Surf::Full(_)) => self.draw_full(qh),
            None => {}
        }
    }

    /// Render the grid into a canvas of `scale`-physical pixels.
    fn paint_grid(&mut self, px: &mut [u32], w: usize, h: usize) {
        let mut cv = Canvas {
            buf: px,
            w,
            h,
            scale: self.scale as f64,
        };
        let t = Txt {
            font: &self.font,
            scale: self.scale as f64,
        };
        let now = Instant::now();
        self.st.grid().render(&mut cv, &t, now, self.st.gdy(now));
    }

    fn draw_layer(&mut self, qh: &QueueHandle<Self>) {
        self.dirty = false;
        let Some(surf) = self.wl_surface().cloned() else {
            return;
        };
        let (bw, bh) = ((W * self.scale as f64) as i32, (H * self.scale as f64) as i32);
        let Ok((buffer, canvas)) =
            self.pool.create_buffer(bw, bh, bw * 4, wl_shm::Format::Argb8888)
        else {
            return;
        };
        // shm slots are page-aligned — a u32 view of the ARGB bytes is sound
        let px: &mut [u32] = unsafe {
            std::slice::from_raw_parts_mut(canvas.as_mut_ptr().cast::<u32>(), (bw * bh) as usize)
        };
        self.paint_grid(px, bw as usize, bh as usize);
        let _ = buffer.attach_to(&surf);
        surf.set_buffer_scale(self.scale);
        surf.damage_buffer(0, 0, bw, bh);
        surf.frame(qh, surf.clone());
        surf.commit();
    }

    /// Fullscreen fallback: transparent buffer covering the output,
    /// palette composited at pal_off with its fringe as real alpha
    /// shadow. Only the palette rect takes input.
    fn draw_full(&mut self, qh: &QueueHandle<Self>) {
        self.dirty = false;
        let (fw, fh) = self.full_log;
        if fw <= 0 || fh <= 0 {
            return; // no configure yet — can't size the buffer
        }
        let Some(surf) = self.wl_surface().cloned() else {
            return;
        };
        let scale = self.scale as f64;
        let (bw, bh) = ((fw as f64 * scale) as i32, (fh as f64 * scale) as i32);
        let (pw, ph) = ((W * scale) as usize, (H * scale) as usize);
        let Ok((buffer, canvas)) =
            self.pool.create_buffer(bw, bh, bw * 4, wl_shm::Format::Argb8888)
        else {
            return;
        };
        let px: &mut [u32] = unsafe {
            std::slice::from_raw_parts_mut(canvas.as_mut_ptr().cast::<u32>(), (bw * bh) as usize)
        };
        px.fill(0); // transparent black outside the palette
        self.scratch.resize(pw * ph, 0);
        let mut scratch = std::mem::take(&mut self.scratch);
        self.paint_grid(&mut scratch, pw, ph);
        blit_palette(
            px,
            bw as usize,
            &scratch,
            pw,
            ph,
            (self.pal_off.0 * scale) as usize,
            (self.pal_off.1 * scale) as usize,
            scale,
        );
        self.scratch = scratch;
        let _ = buffer.attach_to(&surf);
        surf.set_buffer_scale(self.scale);
        surf.damage_buffer(0, 0, bw, bh);
        surf.frame(qh, surf.clone());
        surf.commit();
    }
}

/// Composite the RGB palette into the fullscreen ARGB buffer: opaque
/// inside the card, and the fringe ring becomes a true alpha shadow
/// (the grid renders RGB-only — coverage is recomputed here).
#[allow(clippy::too_many_arguments)]
fn blit_palette(
    dst: &mut [u32],
    dst_w: usize,
    pal: &[u32],
    pw: usize,
    ph: usize,
    ox: usize,
    oy: usize,
    scale: f64,
) {
    for py in 0..ph {
        for px in 0..pw {
            let d = rrect_sdf(
                px as f64 / scale,
                py as f64 / scale,
                W / 2.0,
                H / 2.0,
                W / 2.0,
                H / 2.0,
                14.0,
            );
            let argb = if d <= 0.0 {
                0xFF00_0000 | pal[py * pw + px]
            } else {
                let a = 0.35 * (1.0 - d / 2.5).clamp(0.0, 1.0);
                if a <= 0.0 {
                    continue;
                }
                (((a * 255.0) as u32) << 24) | FRINGE
            };
            dst[(oy + py) * dst_w + ox + px] = argb;
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &WlSurface,
        factor: i32,
    ) {
        if self.wl_surface() == Some(surface) && factor >= 1 && factor != self.scale {
            self.scale = factor;
            self.dirty = true;
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _transform: smithay_client_toolkit::reexports::client::protocol::wl_output::Transform,
    ) {
    }

    /// The frame callback fired — the run loop wakes on it like any
    /// other event; nothing to do here.
    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _time: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &WlSurface,
        output: &WlOutput,
    ) {
        if self.wl_surface() != Some(surface) {
            return;
        }
        if let Some(info) = self.output_state.info(output) {
            let s = info.scale_factor.max(1);
            if s != self.scale {
                self.scale = s;
                self.dirty = true;
            }
            if matches!(self.surface, Some(Surf::Full(_))) {
                self.out_log = info.logical_size;
            }
        }
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _output: &WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {}
    fn update_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {}
    fn output_destroyed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {}
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm_state
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Keyboard => {
                let _ = self.seat_state.get_keyboard(qh, &seat, None);
            }
            Capability::Pointer => {
                let _ = self.seat_state.get_pointer(qh, &seat);
            }
            _ => {}
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: WlSeat,
        _capability: Capability,
    ) {
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &WlPointer,
        events: &[PointerEvent],
    ) {
        let Some(mine) = self.wl_surface().cloned() else {
            return;
        };
        let (ox, oy) = self.pal_off;
        for ev in events {
            if ev.surface != mine {
                continue;
            }
            match ev.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    if self.st.moved(ev.position.0 - ox, ev.position.1 - oy) {
                        self.dirty = true;
                    }
                }
                PointerEventKind::Leave { .. } => {
                    if self.st.pointer_left() {
                        self.dirty = true;
                    }
                }
                PointerEventKind::Press { button, .. } if button == BTN_LEFT => {
                    let (x, y) = (ev.position.0 - ox, ev.position.1 - oy);
                    let settled = self
                        .shown_at
                        .is_some_and(|t| t.elapsed().as_millis() > 350);
                    let outside = matches!(self.surface, Some(Surf::Full(_)))
                        && settled
                        && (x < 0.0 || y < 0.0 || x > W || y > H);
                    if outside {
                        // the transparent area is still our surface —
                        // a press there is the popover's outside-click
                        self.st.begin_close();
                    } else if let Hit::Event(e) = self.st.clicked() {
                        let _ = self.proxy.send_event(HostEvent::Gallery(e));
                    }
                    self.dirty = true;
                }
                // wl axis +ve scrolls the view down — our wheel() wants +ve = up
                PointerEventKind::Axis { vertical, .. } => {
                    self.st.wheel(-vertical.absolute);
                }
                _ => {}
            }
        }
    }
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _surface: &WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
    }

    /// Keyboard focus moved elsewhere — popover semantics, dismiss.
    /// For `Full`, clicks outside the palette fall through the input
    /// region to the desktop, which is what triggers this leave.
    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        surface: &WlSurface,
        _serial: u32,
    ) {
        if self.wl_surface() == Some(surface)
            && matches!(self.surface, Some(Surf::Layer(_)))
        {
            // layer-shell focus is meaningful (Exclusive interactivity);
            // for Full, GNOME bounces focus during the maximize settle —
            // dismissal there is Esc / × / an outside-click press
            self.st.begin_close();
        }
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        if event.keysym == Keysym::Escape {
            self.st.escape();
        }
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _serial: u32,
        _event: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _serial: u32,
        _modifiers: Modifiers,
        _layout: u32,
    ) {
    }
}

impl LayerShellHandler for App {
    /// The compositor removed the surface — exit now, no slide-out.
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        self.exit = true;
    }

    /// SCTK already acked the serial — flag a repaint and restart the
    /// entrance on the first one (when the surface becomes real).
    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        _configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        if self.shown_at.is_none() {
            self.shown_at = Some(Instant::now());
            self.st.reopen();
        }
        self.dirty = true;
    }
}

impl WindowHandler for App {
    /// Compositor asked to close — exit now, no slide-out.
    fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _window: &Window) {
        self.exit = true;
    }

    /// Maximized configure carries the work-area size — that fixes the
    /// buffer dims and the palette's docked offset. A compositor that
    /// ignores the maximize request reports no size: fall back to the
    /// palette's own dims (compositor then centers it — same shape as
    /// the winit path, still functional).
    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _window: &Window,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        let (w, h) = match configure.new_size {
            (Some(w), Some(h)) => (w.get() as f64, h.get() as f64),
            _ => (W, H),
        };
        self.full_log = (w as i32, h as i32);
        self.pal_off = if w > W + 1.0 {
            // maximized: dock at the top-right — the tray lives in the
            // panel's right corner, so the palette reads as dropping
            // out of it (macOS menu-bar pattern). If the surface also
            // covers the panel zone, clear the ~32px bar height.
            let over_panel = self.out_log.is_some_and(|(_, oh)| h as i32 >= oh);
            ((w - W - 20.0).max(0.0), if over_panel { 40.0 } else { 10.0 })
        } else {
            (0.0, 0.0)
        };
        if self.shown_at.is_none() {
            self.shown_at = Some(Instant::now());
            self.st.reopen();
        }
        self.dirty = true;
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_seat!(App);
delegate_pointer!(App);
delegate_keyboard!(App);
delegate_layer!(App);
delegate_xdg_shell!(App);
delegate_xdg_window!(App);
delegate_registry!(App);
