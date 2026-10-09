//! macOS chrome backend: one titled-but-chromeless NSWindow stretched over
//! the monitor (Borderless fullscreen would transition into a new Space —
//! same reason the overlay avoids it), rendered by a layer-backed
//! `QSChromeView` whose `CALayer.contents` is a premultiplied
//! NSBitmapImageRep updated per frame.
//!
//! Click-through comes from two halves that share `ChromeView::cursor_at`:
//! while the cursor is outside the live input rects the window is
//! `ignoresMouseEvents`, and a global NSEvent monitor watches moves bound
//! for *other* apps (global monitors never see our own events — exactly
//! the ignored case); inside a rect the window wakes and the view's own
//! mouse methods take over until the cursor leaves again.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBitmapFormat, NSBitmapImageRep, NSColor, NSEvent, NSEventMask,
    NSScreen, NSStatusWindowLevel, NSView, NSWindowButton, NSWindowCollectionBehavior,
    NSWindowStyleMask, NSWindowTitleVisibility, NSWorkspace,
};
use objc2_foundation::{NSPoint, NSRect, NSString};
use objc2_quartz_core::CATransaction;
use qs_overlay::sdf::Canvas;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::monitor::MonitorHandle;
use winit::platform::macos::MonitorHandleExtMacOS;
use winit::window::{Window, WindowId};

use super::{Chrome, ChromeCmd, ChromeEv, Corner, FRect, Key, Land, ShotKind, Stack};

/* ==================== NSView subclass ==================== */

#[derive(Default)]
struct Ivars {
    /// live input rects in view-local logical coords (top-left origin —
    /// the view is flipped)
    rects: RefCell<Vec<FRect>>,
    /// event sink wired to the host's EventLoopProxy
    handler: RefCell<Option<Rc<dyn Fn(ChromeEv)>>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "QSChromeView"]
    #[ivars = Ivars]
    struct ChromeView;

    unsafe impl NSObjectProtocol for ChromeView {}

    impl ChromeView {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Option<Retained<Self>> {
            let this = this.set_ivars(Ivars::default());
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(isOpaque))]
        fn opaque(&self) -> bool {
            false
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        // let clicks land on a thumbs stack even while the app is inactive
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _e: Option<&NSEvent>) -> bool {
            true
        }

        /// nil outside the live rects — AppKit skips us for hit-testing,
        /// so clicks pass to the windows below without a click-through
        /// flash.
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, p: NSPoint) -> *mut NSView {
            let local = self.convertPoint_fromView(p, unsafe { self.superview() }.as_deref());
            let inside = self
                .ivars()
                .rects
                .borrow()
                .iter()
                .any(|r| local.x >= r.x && local.x <= r.x + r.w && local.y >= r.y && local.y <= r.y + r.h);
            if inside {
                self as *const Self as *mut NSView
            } else {
                std::ptr::null_mut()
            }
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, e: &NSEvent) {
            self.cursor_event(e);
        }

        #[unsafe(method(leftMouseDragged:))]
        fn left_dragged(&self, e: &NSEvent) {
            self.cursor_event(e);
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_dragged(&self, e: &NSEvent) {
            self.cursor_event(e);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, e: &NSEvent) {
            let (x, y) = self.point(e);
            // key the window so the stack's keyboard nav works
            if let Some(w) = self.window() {
                w.makeFirstResponder(Some(self));
            }
            self.emit(ChromeEv::Down { x, y });
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, e: &NSEvent) {
            let (x, y) = self.point(e);
            self.emit(ChromeEv::RightDown { x, y });
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, e: &NSEvent) {
            let k = match e.keyCode() {
                53 => Key::Esc,
                123 => Key::Left,
                124 => Key::Right,
                36 | 76 => Key::Enter,
                51 => Key::Back,
                _ => return,
            };
            self.emit(ChromeEv::Key(k));
        }
    }
);

impl ChromeView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        unsafe { msg_send![mtm.alloc::<Self>(), initWithFrame: frame] }
    }

    fn emit(&self, ev: ChromeEv) {
        if let Some(h) = self.ivars().handler.borrow().as_ref() {
            h(ev);
        }
    }

    fn point(&self, e: &NSEvent) -> (f64, f64) {
        let p = self.convertPoint_fromView(e.locationInWindow(), None);
        (p.x, p.y)
    }

    /// One funnel for cursor bookkeeping — updates `ignoresMouseEvents`
    /// from the live rects and forwards the move. Called by the view's
    /// own events (window awake) and by the global monitor (ignored).
    fn cursor_at(&self, local: (f64, f64)) {
        let (x, y) = local;
        let inside = self
            .ivars()
            .rects
            .borrow()
            .iter()
            .any(|r| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h);
        if let Some(w) = self.window() {
            if w.ignoresMouseEvents() == inside {
                w.setIgnoresMouseEvents(!inside);
            }
        }
        self.emit(ChromeEv::Move { x, y });
    }

    fn cursor_event(&self, e: &NSEvent) {
        self.cursor_at(self.point(e));
    }

    /// From the global monitor: AppKit screen point → view-local.
    fn cursor_at_screen(&self, p: NSPoint) {
        let Some(w) = self.window() else { return };
        let local = self.convertPoint_fromView(w.convertPointFromScreen(p), None);
        self.cursor_at((local.x, local.y));
    }

    /// Re-evaluate under the current cursor after the rects changed.
    fn sync_input(&self, rects: Vec<FRect>) {
        *self.ivars().rects.borrow_mut() = rects;
        self.cursor_at_screen(NSEvent::mouseLocation());
    }
}

/* ==================== window ==================== */

/// One chrome surface per monitor.
pub struct ChromeWin {
    win: Window,
    view: Retained<ChromeView>,
    /// keeps the global mouse monitor alive — removed in Drop
    mon_token: Option<Retained<AnyObject>>,
    pub stack: Stack,
    rep: Retained<NSBitmapImageRep>,
    buf: Vec<u32>,
    mon_id: u32,
    /// logical size/origin of this monitor
    lw: f64,
    lh: f64,
    mx: f64,
    my: f64,
    scale: f64,
    font: Rc<fontdue::Font>,
    accent: u32,
    /// accessibilityDisplayShouldReduceMotion sampled at land-time
    rm: bool,
}

impl ChromeWin {
    fn new(
        el: &ActiveEventLoop,
        mon: &MonitorHandle,
        corner: Corner,
        font: Rc<fontdue::Font>,
        accent: u32,
        sink: Rc<dyn Fn(ChromeEv)>,
    ) -> Result<Self> {
        let sf = mon.scale_factor();
        let (mw, mh) = {
            let s = mon.size();
            (s.width as f64 / sf, s.height as f64 / sf)
        };
        let mpos = mon.position();
        let (mx, my) = (mpos.x as f64 / sf, mpos.y as f64 / sf);
        let (pw, ph) = ((mw * sf).round() as usize, (mh * sf).round() as usize);

        let attrs = Window::default_attributes()
            .with_title("quickshort chrome")
            .with_visible(false)
            .with_resizable(false)
            .with_transparent(true);
        let win = el.create_window(attrs).context("chrome window")?;

        let RawWindowHandle::AppKit(h) = win
            .window_handle()
            .map_err(|e| anyhow!("chrome handle: {e}"))?
            .as_raw()
        else {
            anyhow::bail!("not an AppKit window");
        };
        let content: &NSView = unsafe { &*(h.ns_view.as_ptr() as *const NSView) };
        let ns_win = content.window().context("no NSWindow")?;
        let mtm = MainThreadMarker::new().context("off the main thread")?;

        // chromeless but still keyable (borderless windows can't take key
        // status, which would kill the stack's keyboard nav)
        ns_win.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        ns_win.setTitlebarAppearsTransparent(true);
        ns_win.setStyleMask(ns_win.styleMask() | NSWindowStyleMask::FullSizeContentView);
        for b in [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ] {
            if let Some(b) = ns_win.standardWindowButton(b) {
                b.setHidden(true);
            }
        }
        ns_win.setOpaque(false);
        ns_win.setBackgroundColor(Some(&NSColor::clearColor()));
        ns_win.setHasShadow(false);
        // above apps' windows, still under the capture overlay (screensaver
        // level) so a new capture isn't drawn on top of the stack
        ns_win.setLevel(NSStatusWindowLevel);
        ns_win.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        ns_win.setIgnoresMouseEvents(true);

        // stretch over the matching NSScreen — AppKit's bottom-left frame
        let mut found = false;
        for screen in NSScreen::screens(mtm).iter() {
            let desc = screen.deviceDescription();
            let key = NSString::from_str("NSScreenNumber");
            let id = desc
                .objectForKey(&key)
                .and_then(|o| o.downcast_ref::<objc2_foundation::NSNumber>().map(|n| n.as_u32()));
            if id == Some(mon.native_id()) {
                ns_win.setFrame_display(screen.frame(), true);
                found = true;
                break;
            }
        }
        if !found {
            anyhow::bail!("no NSScreen for monitor {}", mon.native_id());
        }

        // our drawing surface replaces none of winit's view — it sits on
        // top as a subview and answers hitTest only inside live rects
        let view = ChromeView::new(mtm, content.bounds());
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        view.setWantsLayer(true);
        if let Some(layer) = view.layer() {
            layer.setContentsScale(sf);
        }
        content.addSubview(&view);
        *view.ivars().handler.borrow_mut() = Some(sink);

        // 32LE without AlphaFirst = memory bytes R,G,B,A — the buffer's
        // A<<24|B<<16|G<<8|R u32 packing. (AlphaFirst would put alpha at
        // byte0 — the little-endian "first" — and eat the R channel.)
        let rep = unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
                mtm.alloc::<NSBitmapImageRep>(),
                std::ptr::null_mut(),
                pw as isize,
                ph as isize,
                8,
                4,
                true,
                false,
                objc2_app_kit::NSCalibratedRGBColorSpace,
                NSBitmapFormat::ThirtyTwoBitLittleEndian,
                (pw * 4) as isize,
                32,
            )
        }
        .context("bitmap rep")?;

        // global monitor: fires only for events bound for *other* apps —
        // while the window ignores mouse events that's every move
        let v = view.clone();
        let block = RcBlock::new(move |_e: NonNull<NSEvent>| {
            let p = NSEvent::mouseLocation();
            v.cursor_at_screen(p);
        });
        let mask = NSEventMask::MouseMoved
            | NSEventMask::LeftMouseDragged
            | NSEventMask::RightMouseDragged
            | NSEventMask::OtherMouseDragged;
        let mon_token = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &block);

        win.set_visible(true);
        Ok(Self {
            win,
            view,
            mon_token,
            stack: Stack::new(corner),
            rep,
            buf: vec![0; pw * ph],
            mon_id: mon.native_id(),
            lw: mw,
            lh: mh,
            mx,
            my,
            scale: sf,
            font,
            accent,
            rm: NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion(),
        })
    }

    pub fn id(&self) -> WindowId {
        self.win.id()
    }
    fn mon_id(&self) -> u32 {
        self.mon_id
    }
    pub fn stack(&self) -> &Stack {
        &self.stack
    }
    pub fn stack_mut(&mut self) -> &mut Stack {
        &mut self.stack
    }

    /// Render the stack into the buffer and push it to the layer.
    fn present(&mut self) {
        self.buf.fill(0);
        {
            let mut cv = Canvas {
                buf: &mut self.buf,
                w: (self.lw * self.scale).round() as usize,
                h: (self.lh * self.scale).round() as usize,
                scale: self.scale,
            };
            let font = self.font.clone();
            self.stack.render(&mut cv, &font, self.accent, Instant::now());
        }
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.buf.as_ptr() as *const u8,
                self.rep.bitmapData(),
                self.buf.len() * 4,
            );
        }
        if let Some(layer) = self.view.layer() {
            // hand CA a CGImage straight off the rep — routing through
            // NSImage colorspace-converts and bleeds alpha
            if let Some(cg) = self.rep.CGImage() {
                let obj: &AnyObject =
                    unsafe { &*(Retained::as_ptr(&cg) as *const AnyObject) };
                // contents swaps implicitly crossfade ~0.25s — at animation
                // framerates that ghosts every frame into a translucent smear
                CATransaction::begin();
                CATransaction::setDisableActions(true);
                unsafe { layer.setContents(Some(obj)) };
                CATransaction::commit();
            }
        }
        self.view.sync_input(
            self.stack
                .input_rects(self.lw, self.lh)
                .into_iter()
                .map(|(r, _)| r)
                .collect(),
        );
    }

    /// Fly a finished shot into this monitor's stack.
    fn land(&mut self, l: Land) {
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
        self.stack.land(l.img, l.kind, l.path, from, rm, scale);
        self.present();
    }

    /// Platform event → stack → optional host command. Redraw is handled
    /// by the caller (Chrome::event) via `redraw`.
    pub fn event(&mut self, ev: ChromeEv) -> Option<ChromeCmd> {
        match ev {
            ChromeEv::Key(k) => self.stack.key(k),
            e => self.stack.pointer(e, self.lw, self.lh),
        }
    }

    pub fn redraw(&mut self) {
        self.present();
    }

    /// winit window event — repaint requests and scale changes.
    pub fn window_event(&mut self, ev: &WindowEvent) -> bool {
        match ev {
            WindowEvent::RedrawRequested => {
                self.present();
                true
            }
            _ => true, // the window exists; nothing else needs handling
        }
    }

    /// Animation tick — returns the next wake while a shot is in flight.
    pub fn pump(&mut self) -> Option<Instant> {
        if !self.stack.animating() {
            return None;
        }
        let now = Instant::now();
        self.stack.settle(now);
        self.present();
        if self.stack.animating() {
            Some(now + Duration::from_millis(16))
        } else {
            None
        }
    }

    /// muda needs the NSView for popUpMenuPositioningItem.
    pub fn view_ptr(&self) -> *const std::ffi::c_void {
        Retained::as_ptr(&self.view) as *const std::ffi::c_void
    }
}

impl Drop for ChromeWin {
    fn drop(&mut self) {
        if let Some(mon) = &self.mon_token {
            unsafe { NSEvent::removeMonitor(mon) };
        }
    }
}

/* ==================== entry point ==================== */

/// Monitor containing the selection's centre, else the primary.
fn monitor_for(el: &ActiveEventLoop, sel: Option<qs_core::Rect>) -> Option<MonitorHandle> {
    let mut mons = el.available_monitors();
    let primary = el.primary_monitor();
    match sel {
        Some(r) => {
            let (cx, cy) = (r.x as f64 + r.w as f64 / 2.0, r.y as f64 + r.h as f64 / 2.0);
            mons.find(|m| {
                let sf = m.scale_factor();
                let p = m.position();
                let s = m.size();
                let (mx, my) = (p.x as f64 / sf, p.y as f64 / sf);
                let (mw, mh) = (s.width as f64 / sf, s.height as f64 / sf);
                cx >= mx && cx < mx + mw && cy >= my && cy < my + mh
            })
            .or(primary)
        }
        None => primary,
    }
}

/// `Chrome::land` — the shared entry. Err hands the `Land` back intact
/// so the host can fall back to the quick-access card.
pub fn land(chrome: &mut Chrome, el: &ActiveEventLoop, l: Land) -> Result<(), Land> {
    let Some(mon) = monitor_for(el, l.sel) else {
        return Err(l);
    };
    let mid = mon.native_id();
    if chrome.wins.iter().all(|w| w.mon_id() != mid) {
        let font = Rc::new(match crate::settings::system_font() {
            Ok(f) => f,
            Err(_) => return Err(l),
        });
        let corner = Corner::from_popup_pos(chrome.pos());
        // the sink binds the window id, which only exists after creation —
        // give the view a placeholder first and rebind to the real id below
        let w = match ChromeWin::new(
            el,
            &mon,
            corner,
            font,
            chrome.accent(),
            (chrome.sink)(WindowId::dummy()),
        ) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("chrome win: {e:#}");
                return Err(l);
            }
        };
        *w.view.ivars().handler.borrow_mut() = Some((chrome.sink)(w.id()));
        chrome.wins.push(w);
    }
    let w = chrome.wins.iter_mut().find(|w| w.mon_id() == mid).unwrap();
    w.land(l);
    Ok(())
}

/* ==================== `--chrome` demo ==================== */

/// Bare event loop hosting one stack — drops three synthetic shots a
/// second apart, then leaves the window live for hover/click/menu poking.
pub fn standalone(pos: &str, accent: u32) -> Result<()> {
    use winit::application::ApplicationHandler;
    use winit::event_loop::{ControlFlow, EventLoop, EventLoopProxy};

    enum Ev {
        View(WindowId, ChromeEv),
    }

    fn shot(i: usize) -> Land {
        let img = image::RgbaImage::from_fn(1200, 800, |x, y| {
            image::Rgba([
                ((x * 200 / 1200) as u8).saturating_add(i as u8 * 40),
                (y * 200 / 800) as u8,
                120u8.saturating_add(i as u8 * 30),
                255,
            ])
        });
        Land {
            img,
            kind: [ShotKind::Saved, ShotKind::Shared, ShotKind::Record][i % 3],
            path: None,
            sel: Some(qs_core::Rect::new(200, 200, 400, 300)),
            instant: false,
        }
    }

    struct Demo {
        chrome: Chrome,
        drops: usize,
        next_drop: Instant,
    }
    impl Demo {
        fn drop(&mut self, el: &ActiveEventLoop) {
            if self.chrome.land(el, shot(self.drops)).is_ok() {
                self.drops += 1;
            }
            self.next_drop = Instant::now() + Duration::from_secs(1);
        }
    }
    impl ApplicationHandler<Ev> for Demo {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            self.drop(el);
        }
        fn window_event(&mut self, _el: &ActiveEventLoop, id: WindowId, ev: WindowEvent) {
            self.chrome.window_event(id, &ev);
        }
        fn user_event(&mut self, _el: &ActiveEventLoop, ev: Ev) {
            let Ev::View(id, e) = ev;
            if let Some((wid, cmd)) = self.chrome.event(id, e) {
                match cmd {
                    ChromeCmd::Remove(i) => {
                        self.chrome.remove(wid, i);
                        self.chrome.sweep();
                    }
                    ChromeCmd::Menu { idx, x, y } => eprintln!("menu: thumb {idx} @ {x},{y}"),
                    ChromeCmd::Redraw => {}
                }
            }
        }
        fn about_to_wait(&mut self, el: &ActiveEventLoop) {
            if self.drops < 3 && Instant::now() >= self.next_drop {
                self.drop(el);
            }
            let mut wake = self.chrome.pump();
            if self.drops < 3 {
                wake = Some(wake.map_or(self.next_drop, |w| w.min(self.next_drop)));
            }
            el.set_control_flow(match wake {
                Some(w) => ControlFlow::WaitUntil(w),
                None => ControlFlow::Wait,
            });
        }
    }

    let el = EventLoop::<Ev>::with_user_event().build()?;
    let proxy: EventLoopProxy<Ev> = el.create_proxy();
    let px = proxy.clone();
    let chrome = Chrome::new(
        accent,
        pos,
        Box::new(move |wid| {
            let px = px.clone();
            Rc::new(move |e| {
                let _ = px.send_event(Ev::View(wid, e));
            })
        }),
    );
    let mut demo = Demo {
        chrome,
        drops: 0,
        next_drop: Instant::now(),
    };
    el.run_app(&mut demo)?;
    Ok(())
}
