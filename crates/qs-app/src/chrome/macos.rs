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
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBitmapFormat, NSBitmapImageRep, NSColor, NSDragOperation,
    NSDraggingContext, NSDraggingItem, NSDraggingSession, NSDraggingSource, NSEvent, NSEventMask,
    NSEventModifierFlags, NSFilePromiseProvider, NSFilePromiseProviderDelegate, NSImage, NSScreen,
    NSStatusWindowLevel, NSView, NSWindowButton, NSWindowCollectionBehavior, NSWindowStyleMask,
    NSWindowTitleVisibility, NSWorkspace,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColor, CGImage};
use objc2_foundation::{
    NSArray, NSDictionary, NSError, NSLocalizedDescriptionKey, NSNull, NSObject, NSPoint, NSRect,
    NSRunLoop, NSRunLoopCommonModes, NSSize, NSString, NSValue, NSURL,
};
use objc2_quartz_core::{
    kCAGravityResizeAspectFill, CAAction, CADisplayLink, CALayer, CAMediaTiming, CASpringAnimation,
};
use qs_overlay::sdf::Canvas;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::monitor::MonitorHandle;
use winit::platform::macos::MonitorHandleExtMacOS;
use winit::window::{Window, WindowId};

use super::{ChromeCmd, ChromeEv, Corner, FRect, Key, Land, Stack};

/* ==================== NSView subclass ==================== */

#[derive(Default)]
struct Ivars {
    /// live input rects in view-local logical coords (top-left origin —
    /// the view is flipped)
    rects: RefCell<Vec<FRect>>,
    /// event sink wired to the host's EventLoopProxy
    handler: RefCell<Option<Rc<dyn Fn(ChromeEv)>>>,
    /// the press event that became a drag — `beginDraggingSession` needs
    /// the original NSEvent, which is gone by the time the host-side
    /// DragBegin command arrives
    drag_ev: RefCell<Option<Retained<NSEvent>>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "QSChromeView"]
    #[ivars = Ivars]
    struct ChromeView;

    unsafe impl NSObjectProtocol for ChromeView {}

    /// Drag source: file-promises only, copy-only, left-hold-drag.
    unsafe impl NSDraggingSource for ChromeView {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn drag_source_mask(
            &self,
            _s: &NSDraggingSession,
            _ctx: NSDraggingContext,
        ) -> NSDragOperation {
            NSDragOperation::Copy
        }
    }

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
            // stash the press event — beginDraggingSession needs it
            // after the DragBegin command round-trips through the host
            if let Some(ev) = unsafe { Retained::retain(e as *const NSEvent as *mut NSEvent) } {
                *self.ivars().drag_ev.borrow_mut() = Some(ev);
            }
            let (x, y) = self.point(e);
            self.emit(ChromeEv::Dragged { x, y });
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, e: &NSEvent) {
            *self.ivars().drag_ev.borrow_mut() = None;
            let (x, y) = self.point(e);
            self.emit(ChromeEv::Up { x, y });
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
            let cmd = e.modifierFlags().contains(NSEventModifierFlags::Command);
            let k = match e.keyCode() {
                53 => Key::Esc,
                123 => Key::Left,
                124 => Key::Right,
                36 | 76 => Key::Enter,
                51 => Key::Back,
                49 => Key::Space,
                8 if cmd => Key::Copy,
                31 if !cmd => Key::Open,
                1 if !cmd => Key::Share,
                _ => return,
            };
            self.emit(ChromeEv::Key(k));
        }

        /// CADisplayLink callback — one ChromeEv::Tick per display
        /// refresh, the compositor's own animation clock.
        #[unsafe(method(frameTick:))]
        fn frame_tick(&self, _link: &CADisplayLink) {
            self.emit(ChromeEv::Tick);
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

/// An in-flight landing carried by Core Animation: WindowServer owns
/// the spring interpolation, so the main thread never re-rasterizes the
/// capture while it travels to the shelf.
struct NativeFly {
    layer: Retained<CALayer>,
    /// the rep wraps `_img`'s RGBA bytes directly — both must outlive
    /// the layer's contents (CGImage reads the same memory)
    _rep: Retained<NSBitmapImageRep>,
    _img: Rc<image::RgbaImage>,
    /// when the spring has settled + a frame of slack; the thumb is
    /// revealed then
    end: Instant,
}

/// One chrome surface per monitor.
pub struct ChromeWin {
    win: Window,
    view: Retained<ChromeView>,
    /// CADisplayLink bound to this view — the animation clock; paused
    /// at rest so it costs nothing outside fly/fan/hide tweens.
    link: Retained<CADisplayLink>,
    /// keeps the mouse monitors alive — removed in Drop
    mon_token: Option<Retained<AnyObject>>,
    local_mon_token: Option<Retained<AnyObject>>,
    pub stack: Stack,
    /// our pixel carrier, a foreign sublayer of the window's content
    /// layer — AppKit owns the view's backing layer and resyncs its
    /// frame on display, which stretched the cropped contents
    canvas: Retained<CALayer>,
    buf: Vec<u32>,
    mon_key: String,
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
    /// native landing in flight, if any
    native: Option<NativeFly>,
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
            let id = desc.objectForKey(&key).and_then(|o| {
                o.downcast_ref::<objc2_foundation::NSNumber>()
                    .map(|n| n.as_u32())
            });
            if id == Some(mon.native_id()) {
                ns_win.setFrame_display(screen.frame(), true);
                found = true;
                break;
            }
        }
        if !found {
            anyhow::bail!("no NSScreen for monitor {}", mon.native_id());
        }
        // macOS clamps a titled window's frame to the screen's
        // visibleFrame — the menu bar strip is unreachable. Read the
        // clamped frame back and lay out inside it, so hit-tests match
        // pixels and bottom-corner stacks aren't clipped.
        let f = ns_win.frame();
        // primary screen height converts the AppKit bottom-left frame
        // into the winit virtual top-left space `sel` lives in
        let pri_h = NSScreen::screens(mtm)
            .iter()
            .next()
            .map(|s| s.frame().size.height)
            .unwrap_or(0.0);
        let (mx, my) = (f.origin.x, pri_h - f.origin.y - f.size.height);
        let (mw, mh) = (f.size.width, f.size.height);
        let (pw, ph) = ((mw * sf).round() as usize, (mh * sf).round() as usize);

        // our drawing surface replaces none of winit's view — it sits on
        // top as a subview and answers hitTest only inside live rects
        let view = ChromeView::new(mtm, content.bounds());
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        view.setWantsLayer(true);
        content.addSubview(&view);
        // Pixel carrier: a foreign sublayer of the content view's
        // backing layer. It must NOT be the view's own backing layer —
        // AppKit resyncs that one's frame to the full view on display,
        // which stretched our cropped bitmap when no present ran during
        // a CA flight.
        let host = content.layer().context("content layer")?;
        let canvas = CALayer::new();
        canvas.setContentsScale(sf);
        // contents/frame swaps must never animate — the implicit ~0.25s
        // crossfade ghosts every frame mid-animation. NSNull in the
        // actions dict disables it once, instead of wrapping every
        // setContents in a CATransaction (whose commit() blocks ~20ms
        // on WindowServer — that stall was the animation jank).
        let (k1, k2, k3) = (
            NSString::from_str("contents"),
            NSString::from_str("position"),
            NSString::from_str("bounds"),
        );
        let null = || ProtocolObject::<dyn CAAction>::from_retained(NSNull::null());
        let acts =
            NSDictionary::from_retained_objects(&[&*k1, &*k2, &*k3], &[null(), null(), null()]);
        canvas.setActions(Some(&acts));
        host.addSublayer(&canvas);
        // sink binds the WindowId — set later via set_sink() from land()

        // 32LE without AlphaFirst = memory bytes R,G,B,A — the buffer's
        // A<<24|B<<16|G<<8|R u32 packing. (AlphaFirst would put alpha at
        // byte0 — the little-endian "first" — and eat the R channel.)
        // present() wraps this Vec in a fresh rep per frame: a cached
        // NSBitmapImageRep hands back one immutable CGImage, so every
        // frame after the first would show the initial pixels.
        let buf = vec![0u32; pw * ph];

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

        ns_win.setAcceptsMouseMovedEvents(true);

        // local monitor: while the window is NOT ignoring (cursor was
        // inside a rect) move events bound for *us* — including points
        // where hitTest returns nil — land on the window, unseen by both
        // the view and the global monitor. Without this the hover never
        // clears when the cursor leaves a rect (stuck hover → no hide).
        let v2 = view.clone();
        let local_block = RcBlock::new(move |e: NonNull<NSEvent>| {
            let p = NSEvent::mouseLocation();
            v2.cursor_at_screen(p);
            e.as_ptr()
        });
        let local_token =
            unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local_block) };

        win.set_visible(true);

        // The display link is the animation clock: ticks land on the
        // compositor's vsync (ProMotion-aware), paused while idle. It
        // must be bound to the view — a link on a detached target still
        // ticks, but scoping it to the view keeps teardown trivial.
        let link = unsafe {
            let l = CADisplayLink::displayLinkWithTarget_selector(
                &*view as &AnyObject,
                objc2::sel!(frameTick:),
            );
            l.setPaused(true);
            l.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes);
            l
        };

        Ok(Self {
            win,
            view,
            link,
            mon_token,
            local_mon_token: local_token,
            stack: Stack::new(corner),
            canvas,
            buf,
            mon_key: super::mon_key(mon),
            lw: mw,
            lh: mh,
            mx,
            my,
            scale: sf,
            font,
            accent,
            rm: NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion(),
            native: None,
        })
    }

    pub fn id(&self) -> WindowId {
        self.win.id()
    }
    pub(super) fn mon_key(&self) -> &str {
        &self.mon_key
    }
    /// Rebind the event sink once the real WindowId exists — the generic
    /// `land` calls this right after `new`.
    pub(super) fn set_sink(&self, sink: Rc<dyn Fn(ChromeEv)>) {
        *self.view.ivars().handler.borrow_mut() = Some(sink);
    }
    pub fn stack(&self) -> &Stack {
        &self.stack
    }
    pub fn stack_mut(&mut self) -> &mut Stack {
        &mut self.stack
    }

    /// Render the stack into the buffer and push it to the layer.
    /// Only the stack's paint bounds become the layer's frame + a
    /// cropped CGImage — fullscreen-layer contents meant WindowServer
    /// re-composited the whole monitor every frame (~20MB @60Hz),
    /// which was the residual jank even on vsync.
    fn present(&mut self) {
        let now = Instant::now();
        let bounds = self.stack.paint_bounds(self.lw, self.lh, now);
        let Some(b) = bounds else {
            unsafe { self.canvas.setContents(None) };
            self.view.sync_input(Vec::new());
            return;
        };
        // dirty region (physical px) — zero just this band before render
        let s = self.scale;
        let pw = (self.lw * s).round() as usize;
        let ph = (self.lh * s).round() as usize;
        let x0 = ((b.x * s).floor() as usize).min(pw);
        let y0 = ((b.y * s).floor() as usize).min(ph);
        let x1 = (((b.x + b.w) * s).ceil() as usize).min(pw);
        let y1 = (((b.y + b.h) * s).ceil() as usize).min(ph);
        for row in &mut self.buf[y0.min(y1) * pw..y1 * pw] {
            *row = 0;
        }
        {
            let mut cv = Canvas {
                buf: &mut self.buf,
                w: pw,
                h: ph,
                scale: s,
            };
            let font = self.font.clone();
            self.stack.render(&mut cv, &font, self.accent, now);
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let mut planes = [self.buf.as_mut_ptr() as *mut u8];
        let Some(rep) = (unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
                mtm.alloc::<NSBitmapImageRep>(),
                planes.as_mut_ptr(),
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
        }) else {
            return;
        };
        let Some(full) = rep.CGImage() else {
            return;
        };
        let Some(cg) = CGImage::with_image_in_rect(
            Some(&full),
            CGRect::new(
                objc2_core_foundation::CGPoint::new(x0 as f64, y0 as f64),
                objc2_core_foundation::CGSize::new((x1 - x0) as f64, (y1 - y0) as f64),
            ),
        ) else {
            return;
        };
        let obj: &AnyObject = unsafe { &*(CFRetained::as_ptr(&cg).cast::<AnyObject>().as_ptr()) };
        unsafe { self.canvas.setContents(Some(obj)) };
        // frame lives in the SUPERlayer's coords: the content
        // layer backs winit's flipped view, so top-left y maps
        // 1:1 there.
        let parent = self.canvas.superlayer();
        let py = match parent.as_ref() {
            Some(p) if !p.isGeometryFlipped() => p.bounds().size.height - b.y - b.h,
            _ => b.y,
        };
        self.canvas.setFrame(CGRect::new(
            objc2_core_foundation::CGPoint::new(b.x, py),
            objc2_core_foundation::CGSize::new(b.w, b.h),
        ));
        self.view.sync_input(
            self.stack
                .input_rects(self.lw, self.lh)
                .into_iter()
                .map(|(r, _)| r)
                .collect(),
        );
    }

    /// Fly a finished shot into this monitor's stack.
    pub(super) fn land(&mut self, l: Land) {
        // a newer landing supersedes a layer that is still travelling
        self.finish_native();
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
        // the stack always lands settled on macOS — Core Animation
        // carries the pixels; the software fly would re-blit the
        // full-res capture on the main thread every frame
        self.stack.land(
            l.img, l.kind, l.path, from, true, self.scale, l.dur_s, l.shared,
        );
        if rm {
            self.present();
            return;
        }
        let Some(img) = self.stack.shot(0).map(|s| s.img.clone()) else {
            self.present();
            return;
        };
        let to = self.stack.landing_rect(self.lw, self.lh);
        self.stack.hold_newest(true);
        self.present();
        if !self.start_native(from, to, img) {
            // no backing layer or CGImage — an instant landing is
            // better than a thumb-shaped hole
            self.stack.hold_newest(false);
            self.present();
        }
    }

    /// Hand a landing to Core Animation. Returns false when the window
    /// has no usable backing layer or the capture can't become a
    /// CGImage; the caller then reveals the thumb immediately.
    fn start_native(&mut self, from: FRect, to: FRect, img: Rc<image::RgbaImage>) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        // the canvas's superlayer is the window's content layer:
        // full-size, stable, and geometry-flipped like winit's view, so
        // top-left logical coords map 1:1
        let Some(parent) = self.canvas.superlayer() else {
            return false;
        };
        let pb = parent.bounds();
        let ph = pb.size.height;
        if ph <= 0.0 {
            return false;
        }

        let (iw, ih) = (img.width() as isize, img.height() as isize);
        if iw <= 0 || ih <= 0 {
            return false;
        }
        // rep wraps the capture's RGBA bytes directly — `NativeFly`
        // keeps both alive for as long as the layer displays them
        let mut planes = [img.as_raw().as_ptr() as *mut u8];
        let Some(rep) = (unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
                mtm.alloc::<NSBitmapImageRep>(),
                planes.as_mut_ptr(),
                iw,
                ih,
                8,
                4,
                true,
                false,
                objc2_app_kit::NSCalibratedRGBColorSpace,
                NSBitmapFormat::ThirtyTwoBitLittleEndian,
                iw * 4,
                32,
            )
        }) else {
            return false;
        };
        let Some(cg) = rep.CGImage() else {
            return false;
        };
        let content: &AnyObject = unsafe { &*((Retained::as_ptr(&cg)) as *const AnyObject) };

        let layer = CALayer::new();
        layer.setContentsScale(self.scale);
        unsafe { layer.setContents(Some(content)) };
        layer.setContentsGravity(unsafe { kCAGravityResizeAspectFill });
        layer.setMasksToBounds(true);
        layer.setCornerRadius(6.0);
        layer.setZPosition(1000.0);
        let shadow = CGColor::new_srgb(0.0, 0.0, 0.0, 1.0);
        layer.setShadowColor(Some(&shadow));
        layer.setShadowOpacity(0.3);
        layer.setShadowRadius(12.0);
        layer.setShadowOffset(CGSize::new(0.0, 0.0));
        // explicit spring below; the property sets must not animate
        let (k1, k2, k3) = (
            NSString::from_str("contents"),
            NSString::from_str("position"),
            NSString::from_str("bounds"),
        );
        let null = || ProtocolObject::<dyn CAAction>::from_retained(NSNull::null());
        let acts =
            NSDictionary::from_retained_objects(&[&*k1, &*k2, &*k3], &[null(), null(), null()]);
        layer.setActions(Some(&acts));

        // top-left logical rect → parent-local coords (bottom-left when
        // the parent is not geometry-flipped)
        let flipped = parent.isGeometryFlipped();
        let geom = |r: FRect| {
            let (x, y) = (r.x - pb.origin.x, r.y - pb.origin.y);
            CGRect::new(
                CGPoint::new(x, if flipped { y } else { ph - y - r.h }),
                CGSize::new(r.w, r.h),
            )
        };
        parent.addSublayer(&layer);
        layer.setFrame(geom(to));

        // Animate position and bounds — the real properties behind
        // `frame`. Animating the derived "frame" key path makes the
        // presentation layer report interpolated frames while the
        // render server draws nothing (black-hole layer).
        let spring = |key: &str, from_v: Retained<NSValue>, to_v: Retained<NSValue>| {
            let a = CASpringAnimation::initWithPerceptualDuration_bounce(
                CASpringAnimation::alloc(),
                0.36,
                0.08,
            );
            a.setKeyPath(Some(&NSString::from_str(key)));
            let f: &AnyObject = unsafe { &*(Retained::as_ptr(&from_v) as *const AnyObject) };
            let t: &AnyObject = unsafe { &*(Retained::as_ptr(&to_v) as *const AnyObject) };
            unsafe {
                a.setFromValue(Some(f));
                a.setToValue(Some(t));
            }
            let d = a.settlingDuration();
            a.setDuration(d);
            layer.addAnimation_forKey(&a, Some(&NSString::from_str(key)));
            d
        };
        let gf = geom(from);
        let gt = geom(to);
        let center = |r: CGRect| {
            CGPoint::new(
                r.origin.x + r.size.width / 2.0,
                r.origin.y + r.size.height / 2.0,
            )
        };
        let settle = spring(
            "position",
            unsafe { NSValue::valueWithPoint(center(gf)) },
            unsafe { NSValue::valueWithPoint(center(gt)) },
        );
        spring(
            "bounds",
            unsafe { NSValue::valueWithRect(CGRect::new(CGPoint::new(0.0, 0.0), gf.size)) },
            unsafe { NSValue::valueWithRect(CGRect::new(CGPoint::new(0.0, 0.0), gt.size)) },
        );

        let end =
            Instant::now() + Duration::from_secs_f64(settle.max(0.05)) + Duration::from_millis(80);
        self.native = Some(NativeFly {
            layer,
            _rep: rep,
            _img: img,
            end,
        });
        true
    }

    /// Spring done: drop the layer and reveal the thumb it was standing
    /// in for. Also the cancel path for a newer landing.
    fn finish_native(&mut self) {
        if let Some(n) = self.native.take() {
            n.layer.removeFromSuperlayer();
            self.stack.hold_newest(false);
            self.present();
        }
    }

    /// Abort without repainting — the caller clears/redraws the stack
    /// itself (remove / clear-all paths).
    pub(super) fn cancel_native(&mut self) {
        if let Some(n) = self.native.take() {
            n.layer.removeFromSuperlayer();
            self.stack.hold_newest(false);
        }
    }

    /// A thumb got dragged past the threshold — start a real drag
    /// session. Saved or not, the shot goes out as an
    /// `NSFilePromiseProvider` so the PNG materializes wherever the
    /// drop lands (Finder, an editor, a composer) — no save-first step.
    fn begin_drag(&mut self, idx: usize) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some((img, name)) = self.stack.shot(idx).map(|s| {
            let name = s
                .path
                .as_ref()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| format!("quickshort-{}.png", unix_ts()));
            (s.img.clone(), name)
        }) else {
            return;
        };
        // drag image: the thumbnail the cursor grabbed, rebuilt as an
        // NSImage (de-premultiplied into a fresh rep)
        let (pixels, tw, th) = match self.stack.shot(idx).map(|s| s.thumb()) {
            Some(t) => t,
            None => return,
        };
        let rep = unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
                mtm.alloc::<NSBitmapImageRep>(),
                std::ptr::null_mut(),
                tw as isize,
                th as isize,
                8,
                4,
                true,
                false,
                objc2_app_kit::NSCalibratedRGBColorSpace,
                NSBitmapFormat::ThirtyTwoBitLittleEndian,
                (tw * 4) as isize,
                32,
            )
        };
        let Some(rep) = rep else { return };
        // packed thumb is premul A<<24|B<<16|G<<8|R → de-premultiply into
        // the rep's RGBA bytes so the drag image matches what's on screen
        unsafe {
            let dst = rep.bitmapData();
            for (i, px) in pixels.iter().enumerate() {
                let (a, b, g, r) = (px >> 24, (px >> 16) & 0xff, (px >> 8) & 0xff, px & 0xff);
                let (r, g, b) = if a > 0 {
                    (r * 255 / a, g * 255 / a, b * 255 / a)
                } else {
                    (0, 0, 0)
                };
                *dst.add(i * 4) = r as u8;
                *dst.add(i * 4 + 1) = g as u8;
                *dst.add(i * 4 + 2) = b as u8;
                *dst.add(i * 4 + 3) = a as u8;
            }
        }
        let size = NSSize::new(tw as f64 / self.scale, th as f64 / self.scale);
        let nsimg = NSImage::initWithSize(mtm.alloc::<NSImage>(), size);
        nsimg.addRepresentation(&rep);

        let delegate = PromiseWriter::new(mtm, name, encode_png(&img));
        let provider = NSFilePromiseProvider::initWithFileType_delegate(
            mtm.alloc::<NSFilePromiseProvider>(),
            &NSString::from_str("public.png"),
            ProtocolObject::from_ref(&*delegate),
        );
        let item = NSDraggingItem::initWithPasteboardWriter(
            mtm.alloc::<NSDraggingItem>(),
            ProtocolObject::from_ref(&*provider),
        );
        // anchor the drag image under the cursor — a small offset past
        // the press point reads as "the thumb came along"
        let f = NSRect::new(NSPoint::new(-size.width / 2.0, -size.height / 2.0), size);
        unsafe { item.setDraggingFrame_contents(f, Some(&*nsimg)) };
        let stashed = self.view.ivars().drag_ev.borrow().clone();
        let ev = match stashed {
            Some(e) => e,
            // scriptable tools (cliclick & friends) deliver down/up but
            // never leftMouseDragged — synthesize one so the session
            // still starts
            None => {
                let p = self
                    .view
                    .window()
                    .map(|w| w.convertPointToScreen(NSPoint::new(0.0, 0.0)))
                    .unwrap_or(NSPoint::new(0.0, 0.0));
                let wnum = self.view.window().map(|w| w.windowNumber()).unwrap_or(0);
                let wnum = wnum as isize;
                let ev: Option<Retained<NSEvent>> = unsafe {
                    msg_send![
                        objc2::class!(NSEvent),
                        mouseEventWithType: objc2_app_kit::NSEventType::LeftMouseDragged,
                        location: NSPoint::new(p.x + self.lw - 60.0, p.y + 60.0),
                        modifierFlags: NSEventModifierFlags::empty(),
                        timestamp: 0.0_f64,
                        windowNumber: wnum,
                        context: std::ptr::null::<AnyObject>(),
                        eventNumber: 0_isize,
                        clickCount: 1_isize,
                        pressure: 0.0_f32
                    ]
                };
                match ev {
                    Some(e) => e,
                    None => return, // can't even build the event — skip
                }
            }
        };
        let items = NSArray::from_retained_slice(&[item]);
        self.view.beginDraggingSessionWithItems_event_source(
            &items,
            &ev,
            ProtocolObject::from_ref(&*self.view),
        );
    }

    /// Platform event → stack → optional host command. Redraw is handled
    /// by the caller (Chrome::event) via `redraw`.
    pub fn event(&mut self, ev: ChromeEv) -> Option<ChromeCmd> {
        if let ChromeEv::Tick = ev {
            // one frame per display refresh — settle + present, then
            // park the link once the tween ends
            let now = Instant::now();
            self.stack.settle(now);
            if self.native.as_ref().is_some_and(|n| now >= n.end) {
                self.finish_native();
            } else {
                self.present();
            }
            if !self.stack.animating() {
                self.link.setPaused(true);
            }
            return None;
        }
        let cmd = match ev {
            ChromeEv::Key(k) => self.stack.key(k),
            e => self.stack.pointer(e, self.lw, self.lh),
        };
        // a drag-out starts the OS session here — the command never
        // reaches the host
        if let Some(ChromeCmd::DragBegin { idx, .. }) = cmd {
            self.begin_drag(idx);
            return None;
        }
        cmd
    }

    pub fn redraw(&mut self) {
        self.present();
    }

    /// winit window event — repaint requests only; animation frames
    /// come from the CADisplayLink, not RedrawRequested.
    pub fn window_event(&mut self, ev: &WindowEvent) -> bool {
        match ev {
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                self.stack.settle(now);
                if self.native.as_ref().is_some_and(|n| now >= n.end) {
                    self.finish_native();
                } else {
                    self.present();
                }
                true
            }
            _ => true, // the window exists; nothing else needs handling
        }
    }

    /// `about_to_wait` hook — arms the display link while animating and
    /// returns the idle-hide deadline at rest. The 16ms wake stays as a
    /// backstop: if the link's screen is occluded or asleep its ticks
    /// stop, and the host-side settle still advances the tween
    /// wall-clock so nothing hangs mid-flight.
    pub fn pump(&mut self) -> Option<Instant> {
        let now = Instant::now();
        if self.native.as_ref().is_some_and(|n| now >= n.end) {
            self.finish_native();
        }
        self.stack.settle(now); // may kick off the idle-hide tween
        let native_end = self.native.as_ref().map(|n| n.end);
        if self.stack.animating() {
            self.link.setPaused(false);
            let t = now + Duration::from_millis(16);
            return Some(match native_end {
                Some(n) => t.min(n),
                None => t,
            });
        }
        if native_end.is_some() {
            // CA interpolates in WindowServer — one wake when the
            // spring settles is all the app needs
            return native_end;
        }
        self.stack.idle_deadline()
    }
}

impl Drop for ChromeWin {
    fn drop(&mut self) {
        self.cancel_native();
        self.link.invalidate();
        for m in [&self.mon_token, &self.local_mon_token]
            .into_iter()
            .flatten()
        {
            unsafe { NSEvent::removeMonitor(m) };
        }
    }
}

/* ==================== file-promise delegate ==================== */

/// Writes the promised PNG when the drop destination asks for it.
/// Bytes are pre-encoded (the promise callback can run off-main on
/// whatever queue AppKit picks).
struct WriterIvars {
    name: String,
    png: Vec<u8>,
    /// pre-built write-failure error — AppKit may call the delegate
    /// off-main, where NSError isn't allocatable
    err: Option<Retained<NSError>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "QSPromiseWriter"]
    #[ivars = WriterIvars]
    struct PromiseWriter;

    unsafe impl NSObjectProtocol for PromiseWriter {}

    unsafe impl NSFilePromiseProviderDelegate for PromiseWriter {
        #[unsafe(method_id(filePromiseProvider:fileNameForType:))]
        fn file_name_for_type(
            &self,
            _p: &NSFilePromiseProvider,
            _t: &NSString,
        ) -> Retained<NSString> {
            NSString::from_str(&self.ivars().name)
        }

        #[unsafe(method(filePromiseProvider:writePromiseToURL:completionHandler:))]
        fn write_promise(
            &self,
            _p: &NSFilePromiseProvider,
            url: &NSURL,
            done: &block2::Block<dyn Fn(*mut NSError)>,
        ) {
            let wrote = url
                .path()
                .map(|p| std::fs::write(p.to_string(), &self.ivars().png))
                .is_some_and(|r| r.is_ok());
            let err = if wrote {
                std::ptr::null_mut()
            } else {
                self.ivars()
                    .err
                    .as_ref()
                    .map(|e| Retained::as_ptr(e) as *mut NSError)
                    .unwrap_or(std::ptr::null_mut())
            };
            done.call((err,));
        }
    }
);

impl PromiseWriter {
    fn new(mtm: MainThreadMarker, name: String, png: Vec<u8>) -> Retained<Self> {
        // the generic write-failure error is built now — alloc needs
        // the main thread, which `new` always runs on
        let desc = NSString::from_str("Couldn't write the promised file");
        let dict = NSDictionary::from_slices(
            &[unsafe { NSLocalizedDescriptionKey }],
            &[unsafe { &*(Retained::as_ptr(&desc) as *const AnyObject) }],
        );
        let err = unsafe {
            NSError::initWithDomain_code_userInfo(
                mtm.alloc::<NSError>(),
                &NSString::from_str("QSQuickshortError"),
                1,
                Some(&dict),
            )
        };
        let this = mtm.alloc::<Self>().set_ivars(WriterIvars {
            name,
            png,
            err: Some(err),
        });
        unsafe { msg_send![super(this), init] }
    }
}

/// PNG-encode for the promise — same encoder `actions` uses.
fn encode_png(img: &image::RgbaImage) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    let _ = img.write_to(&mut out, image::ImageFormat::Png);
    out.into_inner()
}

fn unix_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
