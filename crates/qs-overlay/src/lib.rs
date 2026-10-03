//! Overlay: one borderless-fullscreen window per monitor covering the whole
//! virtual screen (works on X11 *and* Wayland — Wayland has no global window
//! positioning). Renders the captured frames dimmed, tracks a drag selection
//! in virtual coords, returns it on mouse release. Pointer grab semantics keep
//! events flowing to the grab window when the cursor crosses monitors.

use anyhow::Result;
use image::imageops;
use qs_core::{CapturedFrame, Point, Rect};
use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::rc::Rc;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::monitor::MonitorHandle;
#[cfg(not(target_os = "macos"))]
use winit::window::Fullscreen;
use winit::window::{Window, WindowId, WindowLevel};

mod annotate;
#[cfg(target_os = "macos")]
mod macos;
pub mod sdf;
mod toolbar;

pub use annotate::{annotate_image, Annotation};
pub use toolbar::{Action, Item, Tool};

const MIN_SELECT: u32 = 3;

/// Outcome of a finished overlay session.
pub enum OverlayEvent {
    /// User picked an action on `sel`; annotations ride along for baking.
    Done(Rect, Action, Vec<Annotation>),
    /// Esc / click-without-drag / window close.
    Cancelled,
}

/// A live overlay session: one borderless window per monitor, created on
/// demand inside the host's event loop. Frame pixels are composited into
/// `Session` canvases at `spawn`, so `frames` may be dropped afterwards.
/// Dropping the session destroys its windows.
pub struct Overlay {
    s: Session,
}

impl Overlay {
    /// Composite `frames` and spawn one overlay window per monitor.
    /// `bounds` = virtual screen bounds (see `qs_core::virtual_bounds`).
    pub fn spawn(el: &ActiveEventLoop, frames: &[CapturedFrame], bounds: Rect) -> Result<Self> {
        let (bright, dim) = composite(frames, bounds);
        let mut s = Session {
            bounds,
            bright,
            dim,
            ..Session::default_state()
        };
        s.spawn_windows(el)?;
        Ok(Self { s })
    }

    /// Route a winit `WindowEvent` into the session. Returns `Some` exactly
    /// once, when the session ends — the host should then drop the overlay.
    pub fn window_event(&mut self, id: WindowId, event: WindowEvent) -> Option<OverlayEvent> {
        self.s.window_event(id, event);
        self.s.outcome()
    }

    /// Coalesce redraws — call from the host's `about_to_wait`.
    pub fn about_to_wait(&mut self) {
        if self.s.dirty && !self.s.done {
            self.s.dirty = false;
            self.s.redraw_all();
        }
    }
}

/// Show the overlay one-shot: own event loop, blocks until the session ends.
/// Used by `--capture` CLI mode. `bounds` = virtual screen bounds.
pub fn select_region(
    frames: &[CapturedFrame],
    bounds: Rect,
) -> Result<Option<(Rect, Action, Vec<Annotation>)>> {
    struct CliRunner<'a> {
        frames: &'a [CapturedFrame],
        bounds: Rect,
        ov: Option<Overlay>,
        out: Option<Option<(Rect, Action, Vec<Annotation>)>>,
    }
    impl ApplicationHandler for CliRunner<'_> {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            match Overlay::spawn(el, self.frames, self.bounds) {
                Ok(ov) => self.ov = Some(ov),
                Err(e) => {
                    eprintln!("overlay failed: {e:#}");
                    el.exit();
                }
            }
        }
        fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
            if let Some(ov) = &mut self.ov {
                if let Some(ev) = ov.window_event(id, event) {
                    self.out = Some(match ev {
                        OverlayEvent::Done(r, a, an) => Some((r, a, an)),
                        OverlayEvent::Cancelled => None,
                    });
                    el.exit();
                }
            }
        }
        fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
            if let Some(ov) = &mut self.ov {
                ov.about_to_wait();
            }
        }
    }

    let el = EventLoop::new()?;
    el.set_control_flow(ControlFlow::Wait);
    let mut runner = CliRunner {
        frames,
        bounds,
        ov: None,
        out: None,
    };
    el.run_app(&mut runner)?;
    Ok(runner.out.flatten())
}

/// Blit all frames into one virtual-space canvas (0xRRGGBB), downscaling
/// hi-DPI frames to logical size for display.
fn composite(frames: &[CapturedFrame], bounds: Rect) -> (Vec<u32>, Vec<u32>) {
    let (w, h) = (bounds.w as usize, bounds.h as usize);
    let mut canvas = vec![0u32; w * h];
    for f in frames {
        let mon = f.geometry.logical_rect();
        let img = if (f.geometry.scale - 1.0).abs() > f64::EPSILON {
            imageops::resize(&f.pixels, mon.w, mon.h, imageops::FilterType::Triangle)
        } else {
            f.pixels.clone()
        };
        for row in 0..img.height() {
            let dy = mon.y - bounds.y + row as i32;
            if dy < 0 || dy >= bounds.h as i32 {
                continue;
            }
            for col in 0..img.width() {
                let dx = mon.x - bounds.x + col as i32;
                if dx < 0 || dx >= bounds.w as i32 {
                    continue;
                }
                let p = img.get_pixel(col, row);
                canvas[dy as usize * w + dx as usize] =
                    ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32;
            }
        }
    }
    let dim = canvas.iter().map(|&p| (p & 0xFEFEFE) >> 1).collect();
    (canvas, dim)
}

struct WinCtx {
    win: Rc<Window>,
    surf: Surface<Rc<Window>, Rc<Window>>,
    /// this monitor's rect in virtual coords
    vrect: Rect,
    /// physical-res ARGB layer of committed strokes (lazy — allocated on
    /// the first commit so zero-annotation runs cost nothing)
    layer: Option<Vec<u32>>,
}

/// Pixelate undo record: canvas-space rect + the pixels it overwrote.
type PixBackup = (Rect, Vec<u32>);

struct Session {
    bounds: Rect,
    bright: Vec<u32>,
    dim: Vec<u32>,
    anchor: Option<Point>, // drag start, virtual coords
    cursor: Point,         // virtual coords (inside the grabbing window)
    sel: Option<Rect>,     // confirmed selection (toolbar phase)
    toolbar: Option<toolbar::Toolbar>,
    tool: Option<Tool>,         // armed drawing tool
    annots: Vec<Annotation>,    // committed strokes
    active: Option<Annotation>, // stroke in progress
    shape_anchor: Point,        // press point for rect/ellipse/arrow/pixelate
    /// bright-canvas backups for undoable pixelate ops
    backups: Vec<PixBackup>,
    /// (annotation, pixelate backup) entries popped by Undo
    redo: Vec<(Annotation, Option<PixBackup>)>,
    /// current stroke size level / palette index
    size: u8,
    color: usize,
    mods: winit::keyboard::ModifiersState,
    result: Option<(Rect, Action, Vec<Annotation>)>,
    done: bool,
    dirty: bool,
    wins: Vec<WinCtx>,
}

impl Session {
    fn default_state() -> Self {
        Self {
            bounds: Rect::default(),
            bright: Vec::new(),
            dim: Vec::new(),
            anchor: None,
            cursor: Point::default(),
            sel: None,
            toolbar: None,
            tool: None,
            annots: Vec::new(),
            active: None,
            shape_anchor: Point::default(),
            backups: Vec::new(),
            redo: Vec::new(),
            size: 1,
            color: 0,
            mods: winit::keyboard::ModifiersState::empty(),
            result: None,
            done: false,
            dirty: false,
            wins: Vec::new(),
        }
    }

    /// Monitor logical rect holding the selection's bottom-right corner —
    /// where the toolbar anchors. Falls back to the first monitor.
    fn sel_monitor(&self, sel: Rect) -> Rect {
        let p = Point::new(sel.right() - 1, sel.bottom() - 1);
        self.wins
            .iter()
            .map(|w| w.vrect)
            .find(|r| p.x >= r.x && p.x < r.right() && p.y >= r.y && p.y < r.bottom())
            .or_else(|| self.wins.first().map(|w| w.vrect))
            .unwrap_or(self.bounds)
    }

    fn finish(&mut self, action: Option<Action>) {
        self.result =
            action.and_then(|a| self.sel.map(|s| (s, a, std::mem::take(&mut self.annots))));
        self.done = true;
    }

    /// `Some` exactly once when the session has ended (see `OverlayEvent`).
    fn outcome(&mut self) -> Option<OverlayEvent> {
        if !self.done {
            return None;
        }
        Some(match self.result.take() {
            Some((r, a, an)) => OverlayEvent::Done(r, a, an),
            None => OverlayEvent::Cancelled,
        })
    }

    /// Begin a stroke with the armed tool at the cursor.
    fn begin_annot(&mut self, t: Tool, p: Point) {
        self.shape_anchor = p;
        let rgb = annotate::PALETTE[self.color];
        let w = annotate::SIZES[self.size as usize];
        self.active = Some(match t {
            Tool::Pen => Annotation::Pen {
                pts: vec![p],
                w,
                rgb,
            },
            Tool::Marker => Annotation::Marker {
                pts: vec![p],
                w: annotate::MARKER_SIZES[self.size as usize],
                rgb,
            },
            Tool::Arrow => Annotation::Arrow { a: p, b: p, w, rgb },
            Tool::Rect => Annotation::Rect {
                r: Rect::new(p.x, p.y, 0, 0),
                w,
                rgb,
            },
            Tool::Ellipse => Annotation::Ellipse {
                r: Rect::new(p.x, p.y, 0, 0),
                w,
                rgb,
            },
            Tool::Pixelate => Annotation::Pixelate(Rect::new(p.x, p.y, 0, 0)),
        });
        self.dirty = true;
    }

    /// Commit the in-progress stroke into the annotation layer / bright canvas.
    fn commit_annot(&mut self) {
        let Some(an) = self.active.take() else { return };
        let ok = match &an {
            Annotation::Pen { pts, .. } | Annotation::Marker { pts, .. } => pts.len() >= 2,
            Annotation::Arrow { a, b, .. } => a != b,
            Annotation::Rect { r, .. }
            | Annotation::Ellipse { r, .. }
            | Annotation::Pixelate(r) => r.w >= MIN_SELECT && r.h >= MIN_SELECT,
        };
        if !ok {
            return;
        }
        if let Annotation::Pixelate(r) = an {
            let rc = r.translated(-self.bounds.x, -self.bounds.y);
            let backup = annotate::pixelate_canvas(
                &mut self.bright,
                self.bounds.w as usize,
                self.bounds.h as usize,
                rc,
            );
            self.backups.push((rc, backup));
        } else {
            self.blit_to_layers(&an);
        }
        self.annots.push(an);
        self.redo.clear(); // a fresh stroke invalidates the redo stack
        if let Some(tb) = &mut self.toolbar {
            tb.has_annots = true;
            tb.has_redo = false;
        }
        self.dirty = true;
    }

    /// Rasterize a committed stroke into every window's physical-res layer —
    /// the same code path as the drag preview, so what you see while drawing
    /// is exactly what gets committed.
    fn blit_to_layers(&mut self, an: &Annotation) {
        for w in &mut self.wins {
            let s = w.win.inner_size();
            let layer = w
                .layer
                .get_or_insert_with(|| vec![0u32; s.width as usize * s.height as usize]);
            an.render_phys(
                &mut annotate::ArgbLayer {
                    buf: layer,
                    w: s.width as usize,
                    h: s.height as usize,
                },
                w.vrect,
                w.win.scale_factor(),
            );
        }
    }

    /// Rebuild the layer in every window from the annotation list — needed
    /// when Undo removes an overlapping stroke.
    fn rerender_layers(&mut self) {
        for w in &mut self.wins {
            if let Some(l) = &mut w.layer {
                l.fill(0);
            }
        }
        let annots = std::mem::take(&mut self.annots);
        for a in &annots {
            self.blit_to_layers(a);
        }
        self.annots = annots;
    }

    fn sync_tb_flags(&mut self) {
        if let Some(tb) = &mut self.toolbar {
            tb.has_annots = !self.annots.is_empty();
            tb.has_redo = !self.redo.is_empty();
        }
    }

    fn undo(&mut self) {
        let Some(an) = self.annots.pop() else { return };
        let backup = if matches!(an, Annotation::Pixelate(_)) {
            self.backups.pop().inspect(|(rc, back)| {
                annotate::restore_canvas(
                    &mut self.bright,
                    self.bounds.w as usize,
                    self.bounds.h as usize,
                    *rc,
                    back,
                );
            })
        } else {
            None
        };
        self.redo.push((an, backup));
        self.rerender_layers();
        self.sync_tb_flags();
        self.dirty = true;
    }

    fn redo(&mut self) {
        let Some((an, backup)) = self.redo.pop() else {
            return;
        };
        if let Annotation::Pixelate(_) = an {
            // deterministic: re-pixelating the restored pixels reproduces both
            // the edit and the backup it had
            if let Some((rc, _)) = backup {
                let nb = annotate::pixelate_canvas(
                    &mut self.bright,
                    self.bounds.w as usize,
                    self.bounds.h as usize,
                    rc,
                );
                self.backups.push((rc, nb));
            }
        } else {
            self.blit_to_layers(&an);
        }
        self.annots.push(an);
        self.sync_tb_flags();
        self.dirty = true;
    }

    /// Reset annotation state when the user starts a fresh selection.
    fn clear_annots(&mut self) {
        self.annots.clear();
        self.redo.clear();
        self.active = None;
        self.tool = None;
        for w in &mut self.wins {
            w.layer = None;
        }
        for (rc, back) in self.backups.drain(..) {
            annotate::restore_canvas(
                &mut self.bright,
                self.bounds.w as usize,
                self.bounds.h as usize,
                rc,
                &back,
            );
        }
    }

    fn redraw_all(&self) {
        for w in &self.wins {
            w.win.request_redraw();
        }
    }

    /// MonitorHandle -> monitor rect in virtual coords.
    /// winit reports position/size in physical px; divide by scale_factor.
    fn vrect_of(m: &MonitorHandle) -> Rect {
        let s = m.scale_factor();
        let p = m.position();
        let sz = m.size();
        Rect::new(
            (p.x as f64 / s).round() as i32,
            (p.y as f64 / s).round() as i32,
            (sz.width as f64 / s).round() as u32,
            (sz.height as f64 / s).round() as u32,
        )
    }

    fn draw(&mut self, id: WindowId) {
        let Some(i) = self.wins.iter().position(|w| w.win.id() == id) else {
            return;
        };
        let wctx = &mut self.wins[i];
        let size = wctx.win.inner_size();
        let (sw, sh) = match (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            (Some(w), Some(h)) => (w, h),
            _ => return,
        };
        if wctx.surf.resize(sw, sh).is_err() {
            return;
        }
        let Ok(mut buf) = wctx.surf.buffer_mut() else {
            return;
        };
        let (sw, sh) = (sw.get() as usize, sh.get() as usize);
        // Surface coords are *physical* px; canvas/virtual coords are *logical*
        // px — scale_factor converts between them (2.0 on Retina).
        let scale = wctx.win.scale_factor();
        // canvas offset of this monitor; canvas coords vs window coords
        let (ox, oy) = (wctx.vrect.x - self.bounds.x, wctx.vrect.y - self.bounds.y);
        let (cw, ch) = (self.bounds.w as i32, self.bounds.h as i32);
        // physical (x,y) -> canvas (cx, cy); precompute per-row/col mapping
        let colmap: Vec<i32> = (0..sw).map(|x| ox + (x as f64 / scale) as i32).collect();
        let rowmap: Vec<i32> = (0..sh).map(|y| oy + (y as f64 / scale) as i32).collect();
        // Expand each canvas row to physical width once, then memcpy it into
        // every physical row that maps to it (consecutive rows share cy).
        let mut rowbuf: Vec<u32> = Vec::new();
        let mut rowbuf_cy = -1i32;
        for (y, &cy) in rowmap.iter().enumerate() {
            if !(0..ch).contains(&cy) {
                continue;
            }
            if cy != rowbuf_cy {
                rowbuf.clear();
                rowbuf.reserve(sw);
                let src = cy as usize * cw as usize;
                for &cx in &colmap {
                    rowbuf.push(if (0..cw).contains(&cx) {
                        self.dim[src + cx as usize]
                    } else {
                        0
                    });
                }
                rowbuf_cy = cy;
            }
            buf[y * sw..y * sw + sw].copy_from_slice(&rowbuf);
        }
        // undim the part of the selection inside this monitor + border;
        // dragging preview takes priority over the confirmed selection
        let sel = self
            .anchor
            .map(|a| Rect::from_drag(a, self.cursor))
            .or(self.sel);
        if let Some(sel) = sel {
            if let Some(local) = sel
                .intersection(&wctx.vrect)
                .map(|r| r.translated(-wctx.vrect.x, -wctx.vrect.y))
            {
                // selection rect is logical; scale to physical px for drawing
                let x0 = (local.x as f64 * scale).max(0.0) as usize;
                let y0 = (local.y as f64 * scale).max(0.0) as usize;
                let x1 = ((local.right() as f64 * scale) as usize).min(sw);
                let y1 = ((local.bottom() as f64 * scale) as usize).min(sh);
                // same trick: expand the canvas slice once per canvas row
                let mut srow: Vec<u32> = Vec::new();
                let mut srow_cy = -1i32;
                for (y, &cy) in rowmap.iter().enumerate().take(y1).skip(y0) {
                    if !(0..ch).contains(&cy) {
                        continue;
                    }
                    if cy != srow_cy {
                        srow.clear();
                        srow.reserve(x1 - x0);
                        let src = cy as usize * cw as usize;
                        for &cx in &colmap[x0..x1] {
                            srow.push(if (0..cw).contains(&cx) {
                                self.bright[src + cx as usize]
                            } else {
                                0
                            });
                        }
                        srow_cy = cy;
                    }
                    buf[y * sw + x0..y * sw + x1].copy_from_slice(&srow);
                }
                // accent border around the local rect (1.5 logical px)
                const ACCENT: u32 = 0x0A84FF;
                let b = (1.5 * scale).round().max(1.0) as i32;
                let (lx0, ly0) = (
                    (local.x as f64 * scale) as i32,
                    (local.y as f64 * scale) as i32,
                );
                let (lx1, ly1) = (
                    (local.right() as f64 * scale) as i32,
                    (local.bottom() as f64 * scale) as i32,
                );
                for i in 0..b {
                    for x in lx0..lx1 {
                        for yy in [ly0 + i, ly1 - 1 - i] {
                            if x >= 0 && yy >= 0 && (x as usize) < sw && (yy as usize) < sh {
                                buf[yy as usize * sw + x as usize] = ACCENT;
                            }
                        }
                    }
                    for y in ly0..ly1 {
                        for xx in [lx0 + i, lx1 - 1 - i] {
                            if xx >= 0 && y >= 0 && (xx as usize) < sw && (y as usize) < sh {
                                buf[y as usize * sw + xx as usize] = ACCENT;
                            }
                        }
                    }
                }
            }
        }
        // committed annotation layer (physical-res ARGB, 1:1 with buf) —
        // clip compositing to the strokes' union bbox
        if let Some(layer) = &mut wctx.layer {
            // window resized / layer from a stale surface size: rebuild it
            if layer.len() != sw * sh {
                *layer = vec![0u32; sw * sh];
                let mut al = annotate::ArgbLayer {
                    buf: layer,
                    w: sw,
                    h: sh,
                };
                for a in &self.annots {
                    a.render_phys(&mut al, wctx.vrect, scale);
                }
            }
        }
        if let (Some(layer), Some(bb)) = (&wctx.layer, annotate::annots_bbox(&self.annots)) {
            if let Some(l) = bb
                .intersection(&wctx.vrect)
                .map(|r| r.translated(-wctx.vrect.x, -wctx.vrect.y))
            {
                let x0 = (l.x as f64 * scale).max(0.0) as usize;
                let y0 = (l.y as f64 * scale).max(0.0) as usize;
                let x1 = ((l.right() as f64 * scale) as usize).min(sw);
                let y1 = ((l.bottom() as f64 * scale) as usize).min(sh);
                for y in y0..y1 {
                    for x in x0..x1 {
                        let lp = layer[y * sw + x];
                        let a = (lp >> 24) & 0xFF;
                        if a == 0 {
                            continue;
                        }
                        let i = y * sw + x;
                        let d = buf[i];
                        let fa = a as f64 / 255.0;
                        let mix =
                            |s: u32, d: u32| (s as f64 * fa + d as f64 * (1.0 - fa)).round() as u32;
                        buf[i] = mix((lp >> 16) & 0xFF, (d >> 16) & 0xFF) << 16
                            | mix((lp >> 8) & 0xFF, (d >> 8) & 0xFF) << 8
                            | mix(lp & 0xFF, d & 0xFF);
                    }
                }
            }
        }
        // in-progress stroke preview at physical res
        if let Some(an) = &self.active {
            an.render_phys(
                &mut sdf::Canvas {
                    buf: &mut buf,
                    w: sw,
                    h: sh,
                    scale,
                },
                wctx.vrect,
                scale,
            );
        }
        if let Some(tb) = &self.toolbar {
            tb.paint(&mut buf, sw, sh, wctx.vrect, scale);
        }
        let _ = buf.present();
    }
}

impl Session {
    /// Create one borderless window per monitor (was `ApplicationHandler::resumed`).
    fn spawn_windows(&mut self, el: &ActiveEventLoop) -> Result<()> {
        for m in el.available_monitors() {
            let attrs = Window::default_attributes()
                .with_title("quickshort")
                .with_decorations(false)
                .with_resizable(false)
                .with_active(true)
                .with_window_level(WindowLevel::AlwaysOnTop);
            // macOS: Borderless fullscreen enters a *new Space* — wrong for an
            // overlay. Cover the monitor with a plain borderless window whose
            // level/frame are set post-creation instead.
            #[cfg(target_os = "macos")]
            let attrs = attrs.with_position(m.position()).with_inner_size(m.size());
            #[cfg(not(target_os = "macos"))]
            let attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(Some(m.clone()))));
            let win = match el.create_window(attrs) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("overlay window creation failed: {e}");
                    continue;
                }
            };
            let win = Rc::new(win);
            #[cfg(target_os = "macos")]
            macos::prepare_overlay_window(&win, &m);
            // launched via global hotkey -> no activation token; nudge the
            // compositor so keyboard focus lands on us (Esc must reach us)
            win.request_user_attention(Some(winit::window::UserAttentionType::Critical));
            let surf = Context::new(win.clone()).and_then(|c| Surface::new(&c, win.clone()));
            match surf {
                Ok(surf) => self.wins.push(WinCtx {
                    win,
                    surf,
                    vrect: Self::vrect_of(&m),
                    layer: None,
                }),
                Err(e) => eprintln!("overlay surface creation failed: {e}"),
            }
        }
        if self.wins.is_empty() {
            anyhow::bail!("could not create any overlay window");
        }
        self.redraw_all();
        Ok(())
    }

    fn window_event(&mut self, id: WindowId, event: WindowEvent) {
        if self.done {
            return;
        }
        let Some(i) = self.wins.iter().position(|w| w.win.id() == id) else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => self.finish(None),
            WindowEvent::RedrawRequested => self.draw(id),
            WindowEvent::CursorMoved { position, .. } => {
                // position is physical px; virtual coords are logical
                let s = self.wins[i].win.scale_factor();
                let v = self.wins[i].vrect;
                self.cursor = Point::new(
                    v.x + (position.x / s).round() as i32,
                    v.y + (position.y / s).round() as i32,
                );
                if self.anchor.is_some() || self.active.is_some() {
                    if let Some(an) = &mut self.active {
                        match an {
                            Annotation::Pen { pts, .. } | Annotation::Marker { pts, .. } => {
                                pts.push(self.cursor)
                            }
                            Annotation::Arrow { b, .. } => *b = self.cursor,
                            Annotation::Rect { r, .. }
                            | Annotation::Ellipse { r, .. }
                            | Annotation::Pixelate(r) => {
                                *r = Rect::from_drag(self.shape_anchor, self.cursor)
                            }
                        }
                    }
                    self.dirty = true;
                } else if let Some(tb) = &mut self.toolbar {
                    let h = tb.hit(self.cursor);
                    if h != tb.hover {
                        tb.hover = h;
                        self.dirty = true;
                        self.wins[i].win.set_cursor(if h.is_some() {
                            winit::window::CursorIcon::Pointer
                        } else if self.tool.is_some() {
                            winit::window::CursorIcon::Crosshair
                        } else {
                            winit::window::CursorIcon::Default
                        });
                    }
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => {
                    // resolve the toolbar hit first so the borrows below are
                    // free to mutate the whole App
                    let hit_item = self
                        .toolbar
                        .as_ref()
                        .and_then(|tb| tb.hit(self.cursor).map(|bi| tb.buttons[bi].0));
                    if let Some(item) = hit_item {
                        match item {
                            Item::Act(action) => {
                                self.finish(Some(action));
                            }
                            Item::Undo => self.undo(),
                            Item::Redo => self.redo(),
                            Item::Size(s) => {
                                self.size = s;
                                if let Some(tb) = &mut self.toolbar {
                                    tb.size = s;
                                }
                                self.dirty = true;
                            }
                            Item::Color(c) => {
                                self.color = c;
                                if let Some(tb) = &mut self.toolbar {
                                    tb.color = c;
                                }
                                self.dirty = true;
                            }
                            Item::Tool(t) => {
                                let nt = if self.tool == Some(t) { None } else { Some(t) };
                                self.tool = nt;
                                let mr = self.sel.map(|s| self.sel_monitor(s));
                                if let Some(tb) = &mut self.toolbar {
                                    tb.tool = nt;
                                    // options row toggles -> reflow the bar
                                    if let (Some(sel), Some(mr)) = (self.sel, mr) {
                                        tb.layout(sel, mr, nt.is_some());
                                    }
                                    tb.hover = None;
                                }
                                self.dirty = true;
                                self.wins[i].win.set_cursor(if self.tool.is_some() {
                                    winit::window::CursorIcon::Crosshair
                                } else {
                                    winit::window::CursorIcon::Pointer
                                });
                            }
                        }
                        return;
                    }
                    if let Some(tb) = &self.toolbar {
                        if tb.contains(self.cursor) {
                            return; // padding area: swallow, don't start a drag
                        }
                        // armed tool: a click outside the toolbar starts a stroke
                        if let Some(t) = self.tool {
                            self.begin_annot(t, self.cursor);
                            return;
                        }
                        // clicked outside the toolbar: start a fresh selection
                        self.toolbar = None;
                        self.sel = None;
                        self.clear_annots();
                        self.dirty = true;
                    }
                    // anchor-only change paints nothing yet; the first
                    // motion event will redraw — skip the sync fullscreen
                    // redraw here (it stalls the drag start)
                    self.anchor = Some(self.cursor);
                }
                ElementState::Released => {
                    if self.active.is_some() {
                        self.commit_annot();
                        return;
                    }
                    let Some(a) = self.anchor.take() else { return };
                    let sel = Rect::from_drag(a, self.cursor);
                    if sel.w >= MIN_SELECT && sel.h >= MIN_SELECT {
                        let mr = self.sel_monitor(sel);
                        self.sel = Some(sel);
                        self.toolbar = Some(toolbar::Toolbar::new(sel, mr, false));
                        self.dirty = true;
                    } else {
                        self.finish(None);
                    }
                }
            },
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
                    self.finish(None);
                    return;
                }
                // ⌘/Ctrl shortcuts in the toolbar phase — same actions as
                // the buttons; T = translate to clipboard, X = raw OCR text
                if self.sel.is_some() && (self.mods.super_key() || self.mods.control_key()) {
                    if let Key::Character(c) = &event.logical_key {
                        let act = match c.as_str() {
                            "s" | "S" => Some(Action::Save),
                            "c" | "C" => Some(Action::Copy),
                            "t" | "T" => Some(Action::Translate),
                            "x" | "X" => Some(Action::Ocr),
                            _ => None,
                        };
                        if let Some(a) = act {
                            self.finish(Some(a));
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
