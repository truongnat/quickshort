//! Linux chrome backend: one undecorated borderless-fullscreen winit
//! window per monitor, presented as a `wl_shm` buffer on Wayland or an
//! XPutImage onto winit's 32-bit ARGB visual on X11 — picked from the
//! raw window handle winit actually produced, so `XDG_SESSION_TYPE` and
//! distro session quirks can't mismatch our upload path.
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
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};
use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, Window, WindowId, WindowLevel};

use super::{ChromeCmd, ChromeEv, Corner, FRect, Key, Land, Stack};

/* ==================== window ==================== */

/// One chrome surface per monitor.
pub struct ChromeWin {
    win: Window,
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
}

/// The presenter — chosen once from the raw handle; Wayland marshals
/// surface requests over winit's own wl_display, X11 paints through a
/// private xcb connection.
enum Imp {
    Wl(Wl),
    X11(X11),
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
        let attrs = Window::default_attributes()
            .with_title("quickshort chrome")
            .with_transparent(true)
            .with_decorations(false)
            .with_resizable(false)
            .with_visible(true)
            // don't steal focus on map — keyboard nav is picked up on
            // first click instead
            .with_active(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            // fullscreen is a state, so the compositor positions us —
            // no fragile override-redirect or manual stacking games
            .with_fullscreen(Some(Fullscreen::Borderless(Some(mon.clone()))));
        let win = el.create_window(attrs).context("chrome window")?;
        let imp = Imp::new(&win)?;

        let size = win.inner_size();
        let (pw, ph) = (size.width as usize, size.height as usize);
        let (lw, lh) = (pw as f64 / sf, ph as f64 / sf);
        let pos = mon.position();
        let (mx, my) = (pos.x as f64 / sf, pos.y as f64 / sf);

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
        })
    }

    pub fn id(&self) -> WindowId {
        self.win.id()
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
    fn present(&mut self) {
        // track size/scale drift (fractional-scale flips, mode changes)
        // here instead of a dedicated resize path — buffers simply
        // follow inner_size each frame
        let size = self.win.inner_size();
        self.scale = self.win.scale_factor();
        let (pw, ph) = (size.width.max(1) as usize, size.height.max(1) as usize);
        self.lw = pw as f64 / self.scale;
        self.lh = ph as f64 / self.scale;
        if self.buf.len() != pw * ph {
            self.buf = vec![0; pw * ph];
        }
        self.buf.fill(0);
        {
            let mut cv = Canvas {
                buf: &mut self.buf,
                w: pw,
                h: ph,
                scale: self.scale,
            };
            let font = self.font.clone();
            self.stack
                .render(&mut cv, &font, self.accent, Instant::now());
        }
        // zone rects (not input_rects): the wider retention zone is the
        // hysteresis that keeps hover alive between moving thumbs
        let rects = self.stack.zone_rects(self.lw, self.lh);
        self.imp.present(&self.buf, pw, ph, &rects, self.scale);
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
        // no reduce-motion query wired up on Linux — `instant` stands in
        let rm = l.instant;
        let scale = self.scale;
        self.stack
            .land(l.img, l.kind, l.path, from, rm, scale, l.dur_s);
        self.present();
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
                    _ => None,
                };
                if let Some(k) = key {
                    self.emit(ChromeEv::Key(k));
                }
            }
            WindowEvent::RedrawRequested => self.present(),
            _ => {}
        }
        true // the window exists; nothing else needs handling
    }

    /// Animation tick — returns the next wake while a shot is in flight
    /// or, at rest, the idle-hide deadline so the stack slides away.
    pub fn pump(&mut self) -> Option<Instant> {
        if let Imp::Wl(wl) = &mut self.imp {
            // winit reads the display fd; our queue's buffered events
            // (buffer.release) still need dispatching by us
            wl.dispatch();
        }
        let now = Instant::now();
        let was = self.stack.animating();
        self.stack.settle(now); // may kick off the idle-hide tween
        if was || self.stack.animating() {
            self.present();
        }
        if self.stack.animating() {
            Some(now + Duration::from_millis(16))
        } else {
            self.stack.idle_deadline()
        }
    }

    /// unused outside macOS — the host cfg-gates the call site
    pub fn view_ptr(&self) -> *const c_void {
        std::ptr::null()
    }
}

impl Imp {
    fn new(win: &Window) -> Result<Self> {
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
                Wl::new(dh.display.as_ptr(), h.surface.as_ptr()).map(Self::Wl)
            }
            RawWindowHandle::Xlib(h) => X11::new(h.window as u32).map(Self::X11),
            RawWindowHandle::Xcb(h) => X11::new(h.window.get()).map(Self::X11),
            other => anyhow::bail!("chrome: unsupported linux display backend {other:?}"),
        }
    }

    fn present(&mut self, buf: &[u32], pw: usize, ph: usize, rects: &[FRect], scale: f64) {
        match self {
            Self::Wl(w) => w.present(buf, pw, ph, rects),
            Self::X11(x) => x.present(buf, pw, ph, rects, scale),
        }
    }
}

/* ==================== wayland ==================== */

use std::os::unix::io::{BorrowedFd, RawFd};

use wayland_client::protocol::{
    wl_buffer::{self, WlBuffer},
    wl_compositor::{self, WlCompositor},
    wl_region::{self, WlRegion},
    wl_registry::{self, WlRegistry},
    wl_shm::{self, WlShm},
    wl_shm_pool::{self, WlShmPool},
};
use wayland_client::{Connection as WlConn, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_sys::client::{wayland_client_handle, wl_proxy};

/// wl_surface request opcodes (the core protocol is wire-stable).
const SURFACE_ATTACH: u32 = 1;
const SURFACE_DAMAGE: u32 = 2;
const SURFACE_SET_INPUT_REGION: u32 = 5;
const SURFACE_COMMIT: u32 = 6;

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
}

/// wayland-client bookkeeping state bound to our private event queue —
/// `WlBuffer::Release` is the only event we act on; every other object
/// is bound with a `()` udata and a noop dispatch.
#[derive(Default)]
struct WlState {
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    /// formats the compositor advertised on wl_shm — argb8888/xrgb8888
    /// are the only guaranteed ones; abgr8888 (our native packing) is
    /// optional and weston refuses create_buffer with it
    shm_formats: Vec<wl_shm::Format>,
    bufs: Vec<Option<WlBuf>>,
    /// slot indices released by the server since the last dispatch
    released: Vec<usize>,
}

impl Wl {
    fn new(display: *mut c_void, surface: *mut c_void) -> Result<Self> {
        let backend =
            unsafe { wayland_backend::client::Backend::from_foreign_display(display.cast()) };
        let conn = WlConn::from_backend(backend);
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        let registry = conn.display().get_registry(&qh, ());
        let mut state = WlState::default();
        queue
            .roundtrip(&mut state)
            .context("wl registry roundtrip")?;
        // wl_registry has no destroy request — dropping it is enough
        drop(registry);
        if state.compositor.is_none() {
            anyhow::bail!("compositor offers no wl_compositor");
        }
        if state.shm.is_none() {
            anyhow::bail!("compositor offers no wl_shm");
        }
        Ok(Self {
            surface: surface as *mut wl_proxy,
            conn,
            queue,
            state,
        })
    }

    /// Drain buffered events — marks released buffers reusable. Called
    /// from `pump` because winit owns the display fd's read path.
    fn dispatch(&mut self) {
        if self.queue.dispatch_pending(&mut self.state).is_err() {
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
    fn slot(&mut self, qh: &QueueHandle<WlState>, pixels: &[u32], w: i32, h: i32) -> Option<usize> {
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
        if self.state.bufs[idx].is_none() {
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
            });
        }
        let b = self.state.bufs[idx].as_mut()?;
        write_shm(b.fd, pixels, b.size, b.argb)?;
        b.busy = true;
        Some(idx)
    }

    fn present(&mut self, buf: &[u32], pw: usize, ph: usize, rects: &[FRect]) {
        let (w, h) = (pw as i32, ph as i32);
        let qh = self.queue.handle();
        let Some(idx) = self.slot(&qh, buf, w, h) else {
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
            // full-surface damage — the repaint is a whole-buffer swap
            (wl.wl_proxy_marshal)(self.surface, SURFACE_DAMAGE, 0i32, 0i32, w, h);
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
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind::<WlCompositor, _, _>(name, 1, qh, ()));
                }
                "wl_shm" => {
                    state.shm = Some(registry.bind::<WlShm, _, _>(name, 1, qh, ()));
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

    fn present(&mut self, buf: &[u32], pw: usize, ph: usize, rects: &[FRect], scale: f64) {
        // A<<24|B<<16|G<<8|R → A<<24|R<<16|G<<8|B, one pixel per 32-bit
        // unit regardless of window depth (ZPixmap pads 24bpp to 4B)
        self.xbuf.resize(buf.len() * 4, 0);
        for (i, px) in buf.iter().enumerate() {
            let p = px & 0xff00_ff00 | (px >> 16) & 0xff | (px << 16) & 0x00ff_0000;
            self.xbuf[i * 4..i * 4 + 4].copy_from_slice(&p.to_ne_bytes());
        }
        let rows_per_band = (self.max_band / (pw * 4).max(1)).max(1);
        let mut y = 0usize;
        while y < ph {
            let rows = rows_per_band.min(ph - y);
            let off = y * pw * 4;
            let _ = xproto::put_image(
                &self.conn,
                xproto::ImageFormat::Z_PIXMAP,
                self.win,
                self.gc,
                pw as u16,
                rows as u16,
                0,
                y as i16,
                0,
                self.depth,
                &self.xbuf[off..off + rows * pw * 4],
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
