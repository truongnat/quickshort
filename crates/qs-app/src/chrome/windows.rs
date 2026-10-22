//! Windows chrome backend: one `WS_EX_LAYERED` popup stretched over the
//! monitor, presented per frame with `UpdateLayeredWindow` out of a
//! premultiplied top-down DIB section.
//!
//! Hit-testing on a layered window is per-pixel alpha — transparent
//! pixels pass clicks straight through to the windows beneath — except
//! the painted chrome doesn't cover the wider retention zone the stack
//! needs for hover hysteresis (`Stack::zone_rects`). `SetWindowRgn`
//! would fix the input region but clips the PAINTED output too, which
//! would guillotine the fly animation mid-flight. Instead `present`
//! stamps α=1 into the zone's dead pixels: a 1/255-black blend is
//! invisible to the eye yet alive to the hit-test.
//!
//! `WS_EX_NOACTIVATE` is deliberately NOT set — an unfocusable window
//! can't take the keyboard nav (Esc/arrows/Enter/⌫). `WS_EX_TOOLWINDOW`
//! keeps the surface off the taskbar and Alt+Tab; the focus steal on
//! click is the same trade the macOS backend makes with a keyable
//! borderless window.
//!
//! `qs_capture::platform_init` opts the process into
//! PER_MONITOR_AWARE_V2 before the event loop exists, so winit reports
//! monitor rects in physical px (logical = physical / scale_factor) and
//! `UpdateLayeredWindow`'s screen-space placement uses the same px.

use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::size_of;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use qs_overlay::sdf::Canvas;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::core::BOOL;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, SIZE};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC,
    HGDIOBJ,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, SystemParametersInfoW, UpdateLayeredWindow,
    GWL_EXSTYLE, HWND_TOPMOST, SPI_GETCLIENTAREAANIMATION, SWP_NOACTIVATE,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, ULW_ALPHA, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::monitor::MonitorHandle;
use winit::window::{Window, WindowId, WindowLevel};

use super::{ChromeCmd, ChromeEv, Corner, FRect, Key, Land, Stack};

/* ==================== DIB surface ==================== */

/// Premultiplied 32bpp top-down DIB selected into a memory DC — the
/// `UpdateLayeredWindow` frame source. Bits stay mapped for the
/// window's life; each `present` rewrites them.
struct Dib {
    dc: HDC,
    bmp: HBITMAP,
    /// object the DIB replaced — restored before the DC dies
    old: HGDIOBJ,
    /// frame pixels (B,G,R,A bytes), `w * h` u32s
    bits: *mut c_void,
    w: usize,
    h: usize,
}

impl Dib {
    fn new(w: usize, h: usize) -> Result<Self> {
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                anyhow::bail!("CreateCompatibleDC failed");
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(h as i32), // negative → top-down rows
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
                .context("CreateDIBSection")?;
            if bits.is_null() {
                anyhow::bail!("CreateDIBSection returned no bits");
            }
            let old = SelectObject(dc, HGDIOBJ(bmp.0));
            Ok(Self {
                dc,
                bmp,
                old,
                bits,
                w,
                h,
            })
        }
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.dc, self.old);
            let _ = DeleteObject(HGDIOBJ(self.bmp.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

/* ==================== window ==================== */

/// One chrome surface per monitor.
pub struct ChromeWin {
    win: Window,
    hwnd: HWND,
    dib: Dib,
    pub stack: Stack,
    /// host's EventLoopProxy closure — set via set_sink() after land()
    /// knows the WindowId
    sink: RefCell<Option<Rc<dyn Fn(ChromeEv)>>>,
    /// render buffer — premul `A<<24|B<<16|G<<8|R` (memory R,G,B,A)
    buf: Vec<u32>,
    /// last cursor position in logical px — MouseInput carries none
    cursor: (f64, f64),
    /// left button currently held — distinguishes Dragged from Move
    left_down: bool,
    /// Ctrl held — gates the Ctrl+C → Key::Copy shortcut
    ctrl: bool,
    mon_key: String,
    /// logical size/origin of this monitor (winit virtual coords)
    lw: f64,
    lh: f64,
    mx: f64,
    my: f64,
    /// screen origin for UpdateLayeredWindow — physical px
    px: i32,
    py: i32,
    scale: f64,
    font: Rc<fontdue::Font>,
    accent: u32,
    /// client-area animations off → land shots instantly
    rm: bool,
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
        let (mp, ms) = (mon.position(), mon.size());
        // under PMv2 awareness winit reports physical px; logical =
        // physical / sf — the same split `monitor_for` applies to `sel`
        let (mx, my) = (mp.x as f64 / sf, mp.y as f64 / sf);
        let (lw, lh) = (ms.width as f64 / sf, ms.height as f64 / sf);
        let (pw, ph) = (ms.width as usize, ms.height as usize);

        let attrs = Window::default_attributes()
            .with_title("quickshort chrome")
            .with_transparent(true)
            .with_decorations(false)
            .with_resizable(false)
            .with_visible(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_position(PhysicalPosition::new(mp.x, mp.y))
            .with_inner_size(PhysicalSize::new(ms.width, ms.height));
        let win = el.create_window(attrs).context("chrome window")?;

        let RawWindowHandle::Win32(h) = win
            .window_handle()
            .map_err(|e| anyhow!("chrome handle: {e}"))?
            .as_raw()
        else {
            anyhow::bail!("not a Win32 window");
        };
        let hwnd = HWND(h.hwnd.get() as *mut c_void);

        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(
                hwnd,
                GWL_EXSTYLE,
                ex | WS_EX_LAYERED.0 as isize | WS_EX_TOOLWINDOW.0 as isize,
            );
            // pin the window rect exactly to the monitor — layered
            // hit-testing is per-pixel alpha inside this rect, so slack
            // here would eat clicks meant for the windows beneath
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                mp.x,
                mp.y,
                ms.width as i32,
                ms.height as i32,
                SWP_NOACTIVATE,
            )
            .context("chrome window placement")?;
        }

        let mut w = Self {
            win,
            hwnd,
            dib: Dib::new(pw, ph)?,
            stack: Stack::new(corner),
            sink: RefCell::new(None),
            buf: vec![0; pw * ph],
            cursor: (0.0, 0.0),
            left_down: false,
            ctrl: false,
            mon_key: super::mon_key(mon),
            lw,
            lh,
            mx,
            my,
            px: mp.x,
            py: mp.y,
            scale: sf,
            font,
            accent,
            rm: animations_disabled(),
        };
        // push a frame before the window shows — a layered window that
        // has never seen UpdateLayeredWindow can flash opaque black
        w.present();
        w.win.set_visible(true);
        Ok(w)
    }

    pub fn id(&self) -> WindowId {
        self.win.id()
    }

    pub(super) fn set_dock_corner(&mut self, corner: Corner) {
        self.stack.set_corner(corner);
    }
    pub(super) fn mon_key(&self) -> &str {
        &self.mon_key
    }
    /// Rebind the event sink once the real WindowId exists — the generic
    /// `land` calls this right after `new`.
    pub(super) fn set_sink(&self, sink: Rc<dyn Fn(ChromeEv)>) {
        *self.sink.borrow_mut() = Some(sink);
    }
    pub fn stack(&self) -> &Stack {
        &self.stack
    }
    pub fn stack_mut(&mut self) -> &mut Stack {
        &mut self.stack
    }

    /// Send a ChromeEv through the host's proxy sink (set by `land`).
    fn emit(&self, ev: ChromeEv) {
        if let Some(s) = &*self.sink.borrow() {
            s(ev);
        }
    }

    /// Render the stack into the DIB and push the frame with
    /// `UpdateLayeredWindow` (screen-space destination in physical px).
    fn present(&mut self) {
        let (pw, ph) = (self.dib.w, self.dib.h);
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

        // per-pixel alpha hit-testing only covers what's painted; stamp
        // α=1 into the retention zone's dead pixels so the hover
        // hysteresis works (see the module docs — a window region can't
        // do this without clipping the fly animation)
        let s = self.scale;
        for r in self.stack.zone_rects(self.lw, self.lh) {
            let x0 = (r.x * s).floor().max(0.0) as usize;
            let y0 = (r.y * s).floor().max(0.0) as usize;
            let x1 = ((r.x + r.w) * s).ceil().min(pw as f64).max(0.0) as usize;
            let y1 = ((r.y + r.h) * s).ceil().min(ph as f64).max(0.0) as usize;
            for y in y0..y1 {
                for px in &mut self.buf[y * pw + x0..y * pw + x1] {
                    if *px >> 24 == 0 {
                        *px |= 1 << 24;
                    }
                }
            }
        }

        // GDI wants premul memory order B,G,R,A (u32 0xAARRGGBB); the
        // buffer is R,G,B,A — swap the outer channels as the pixels
        // land in the DIB
        let dst = unsafe { std::slice::from_raw_parts_mut(self.dib.bits as *mut u32, pw * ph) };
        for (d, &p) in dst.iter_mut().zip(&self.buf) {
            *d = (p & 0xFF00_FF00) | ((p & 0xFF) << 16) | ((p >> 16) & 0xFF);
        }

        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let pt_dst = POINT {
            x: self.px,
            y: self.py,
        };
        let pt_src = POINT { x: 0, y: 0 };
        let size = SIZE {
            cx: pw as i32,
            cy: ph as i32,
        };
        unsafe {
            // hdcDst None — the system composites straight to the screen
            let _ = UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&pt_dst),
                Some(&size),
                Some(self.dib.dc),
                Some(&pt_src),
                COLORREF(0), // unused with ULW_ALPHA
                Some(&blend),
                ULW_ALPHA,
            );
        }
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
        let rm = self.rm || l.instant;
        let scale = self.scale;
        self.stack
            .land(l.img, l.kind, l.path, from, rm, scale, l.dur_s, l.shared);
        self.present();
    }

    /// Platform event → stack → optional host command. Redraw is handled
    /// by the caller (Chrome::event) via `redraw`.
    pub fn event(&mut self, ev: ChromeEv) -> Option<ChromeCmd> {
        let cmd = match ev {
            ChromeEv::Key(k) => self.stack.key(k),
            e => self.stack.pointer(e, self.lw, self.lh),
        };
        // consumed locally, never reaches the host — real drag-out
        // (IDropSource/CF_HDROP) is a follow-up
        match cmd {
            Some(ChromeCmd::DragBegin { .. }) => None,
            other => other,
        }
    }

    pub fn redraw(&mut self) {
        self.present();
    }

    /// winit window event → repaint plus ChromeEv forwarding through the
    /// sink `land` bound. Always consumes — this is our window.
    pub fn window_event(&mut self, ev: &WindowEvent) -> bool {
        match ev {
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y) = (position.x / self.scale, position.y / self.scale);
                self.cursor = (x, y);
                // a held left button is a candidate drag-out — the stack
                // resolves press-vs-drag past its own threshold
                if self.left_down {
                    self.emit(ChromeEv::Dragged { x, y });
                } else {
                    self.emit(ChromeEv::Move { x, y });
                }
            }
            WindowEvent::CursorLeft { .. } => self.emit(ChromeEv::Leave),
            WindowEvent::MouseInput { state, button, .. } => {
                let (x, y) = self.cursor;
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        self.left_down = true;
                        self.emit(ChromeEv::Down { x, y });
                    }
                    (MouseButton::Left, ElementState::Released) => {
                        self.left_down = false;
                        self.emit(ChromeEv::Up { x, y });
                    }
                    (MouseButton::Right, ElementState::Pressed) => {
                        self.emit(ChromeEv::RightDown { x, y });
                    }
                    _ => {}
                }
            }
            WindowEvent::ModifiersChanged(m) => {
                self.ctrl = m.state().control_key();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                // physical codes — the nav keys are layout-independent
                if event.state == ElementState::Pressed {
                    let k = match event.physical_key {
                        PhysicalKey::Code(KeyCode::Escape) => Some(Key::Esc),
                        PhysicalKey::Code(KeyCode::ArrowLeft) => Some(Key::Left),
                        PhysicalKey::Code(KeyCode::ArrowRight) => Some(Key::Right),
                        PhysicalKey::Code(KeyCode::Enter | KeyCode::NumpadEnter) => {
                            Some(Key::Enter)
                        }
                        PhysicalKey::Code(KeyCode::Backspace) => Some(Key::Back),
                        PhysicalKey::Code(KeyCode::Space) => Some(Key::Space),
                        PhysicalKey::Code(KeyCode::KeyC) if self.ctrl => Some(Key::Copy),
                        PhysicalKey::Code(KeyCode::KeyO) if !self.ctrl => Some(Key::Open),
                        PhysicalKey::Code(KeyCode::KeyS) if !self.ctrl => Some(Key::Share),
                        _ => None,
                    };
                    if let Some(k) = k {
                        self.emit(ChromeEv::Key(k));
                    }
                }
            }
            WindowEvent::RedrawRequested => self.present(),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                // the DIB stays at creation resolution — logical geometry
                // follows the new factor so hit tests keep matching pixels
                self.scale = *scale_factor;
                self.lw = self.dib.w as f64 / self.scale;
                self.lh = self.dib.h as f64 / self.scale;
                self.present();
            }
            _ => {}
        }
        true
    }

    /// Animation tick — returns the next wake while a shot is in flight
    /// or, at rest, the idle-hide deadline so the stack slides away.
    /// `UpdateLayeredWindow` has no vsync of its own, so `DwmFlush`
    /// blocks until the compositor presents the queued frame — that
    /// makes each pump land one render per display refresh (the
    /// deprecated-but-live API is the same clock D3D Present waits on).
    pub fn pump(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let was = self.stack.animating();
        self.stack.settle(now); // may kick off the idle-hide tween
        if was || self.stack.animating() {
            // flush BEFORE presenting so this frame lands inside the
            // composite it just unblocked from; a failure (DWM off)
            // degrades to the old 16ms free-run
            let _ = unsafe { DwmFlush() };
            self.stack.settle(Instant::now()); // resample after the wait
            self.present();
        }
        if self.stack.animating() {
            Some(Instant::now() + Duration::from_millis(16))
        } else {
            self.stack.idle_deadline()
        }
    }

    /// Context menus on Windows go through muda's window handle, not a
    /// native view — nothing to hand out (host cfg-gates use to macOS).
    pub fn view_ptr(&self) -> *const c_void {
        std::ptr::null()
    }
}

/// No direct "reduce motion" switch exists on Windows; the closest is
/// "Animate controls and elements inside windows" (Performance Options),
/// reported by SPI_GETCLIENTAREAANIMATION — matching the macOS backend's
/// accessibilityDisplayShouldReduceMotion read.
fn animations_disabled() -> bool {
    let mut b = BOOL(0);
    unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut b as *mut BOOL as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .map(|()| b.0 == 0)
    .unwrap_or(false)
}
