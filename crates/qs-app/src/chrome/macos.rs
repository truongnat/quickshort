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
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBitmapFormat, NSBitmapImageRep, NSColor, NSDragOperation,
    NSDraggingContext, NSDraggingItem, NSDraggingSession, NSDraggingSource, NSEvent, NSEventMask,
    NSEventModifierFlags, NSFilePromiseProvider, NSFilePromiseProviderDelegate, NSImage, NSScreen,
    NSStatusWindowLevel, NSView, NSWindowButton, NSWindowCollectionBehavior, NSWindowStyleMask,
    NSWindowTitleVisibility, NSWorkspace,
};
use objc2_foundation::{
    NSArray, NSDictionary, NSError, NSLocalizedDescriptionKey, NSNull, NSObject, NSPoint, NSRect,
    NSSize, NSString, NSURL,
};
use objc2_quartz_core::CAAction;
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
    /// keeps the mouse monitors alive — removed in Drop
    mon_token: Option<Retained<AnyObject>>,
    local_mon_token: Option<Retained<AnyObject>>,
    pub stack: Stack,
    rep: Retained<NSBitmapImageRep>,
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
        if let Some(layer) = view.layer() {
            layer.setContentsScale(sf);
            // contents swaps must never animate — the implicit ~0.25s
            // crossfade ghosts every frame mid-animation. NSNull in the
            // actions dict disables it once, instead of wrapping every
            // setContents in a CATransaction (whose commit() blocks ~20ms
            // on WindowServer — that stall was the animation jank).
            let k = NSString::from_str("contents");
            let acts = NSDictionary::from_retained_objects(
                &[&*k],
                &[ProtocolObject::<dyn CAAction>::from_retained(NSNull::null())],
            );
            layer.setActions(Some(&acts));
        }
        content.addSubview(&view);
        // sink binds the WindowId — set later via set_sink() from land()

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
        Ok(Self {
            win,
            view,
            mon_token,
            local_mon_token: local_token,
            stack: Stack::new(corner),
            rep,
            buf: vec![0; pw * ph],
            mon_key: super::mon_key(mon),
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
            self.stack
                .render(&mut cv, &font, self.accent, Instant::now());
        }
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.buf.as_ptr() as *const u8,
                self.rep.bitmapData(),
                self.buf.len() * 4,
            );
        }
        if let Some(layer) = self.view.layer() {
            // CGImage straight off the rep — the actions dict disables
            // implicit animations so setContents is non-blocking
            if let Some(cg) = self.rep.CGImage() {
                let obj: &AnyObject = unsafe { &*(Retained::as_ptr(&cg) as *const AnyObject) };
                unsafe { layer.setContents(Some(obj)) };
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

    /// Animation tick — returns the next wake while a shot is in flight
    /// or, at rest, the idle-hide deadline so the stack slides away.
    pub fn pump(&mut self) -> Option<Instant> {
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
}

impl Drop for ChromeWin {
    fn drop(&mut self) {
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
