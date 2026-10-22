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
use winit::dpi::{PhysicalPosition, PhysicalSize};
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
/// Fallback accent — `Prefs::default` and the pixelate preview outline.
const ACCENT: u32 = 0x0A84FF;

/// User-tunable overlay behaviour — the host maps its config into this at
/// `spawn` so a session never reads files itself.
#[derive(Clone)]
pub struct Prefs {
    /// Selection frame, caret and highlight colour (0xRRGGBB).
    pub accent: u32,
    pub keys: Keys,
}

/// ⌘+key triggers for the toolbar-phase actions. Case is ignored; the undo
/// key doubles as redo with ⇧ (⌘key / ⌘⇧key).
#[derive(Clone)]
pub struct Keys {
    pub save: char,
    pub copy: char,
    pub ocr: char,
    pub share: char,
    /// scrolling capture — repeated grabs of the selection stitched tall
    pub scroll: char,
    /// screen recording — live frames → ffmpeg until stopped
    pub record: char,
    pub undo: char,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            accent: ACCENT,
            keys: Keys {
                save: 's',
                copy: 'c',
                ocr: 't',
                share: 'l',
                scroll: 'g',
                record: 'r',
                undo: 'z',
            },
        }
    }
}

/// Largest char boundary ≤ `i` — guards stale byte indexes after edits.
fn floor_boundary(text: &str, i: usize) -> usize {
    let mut b = i.min(text.len());
    while !text.is_char_boundary(b) {
        b -= 1;
    }
    b
}

fn prev_boundary(text: &str, i: usize) -> usize {
    let i = floor_boundary(text, i);
    text[..i]
        .char_indices()
        .next_back()
        .map(|(j, _)| j)
        .unwrap_or(0)
}

fn next_boundary(text: &str, i: usize) -> usize {
    let i = floor_boundary(text, i);
    text[i..]
        .chars()
        .next()
        .map(|c| i + c.len_utf8())
        .unwrap_or(i)
}

/// Stroke a `thick`-px rect outline into the physical buffer (clipped).
#[allow(clippy::too_many_arguments)]
fn frame_rect(
    buf: &mut [u32],
    sw: usize,
    sh: usize,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    thick: i32,
    rgb: u32,
) {
    for i in 0..thick {
        for x in x0..x1 {
            for yy in [y0 + i, y1 - 1 - i] {
                if x >= 0 && yy >= 0 && (x as usize) < sw && (yy as usize) < sh {
                    buf[yy as usize * sw + x as usize] = rgb;
                }
            }
        }
        for y in y0..y1 {
            for xx in [x0 + i, x1 - 1 - i] {
                if xx >= 0 && y >= 0 && (xx as usize) < sw && (y as usize) < sh {
                    buf[y as usize * sw + xx as usize] = rgb;
                }
            }
        }
    }
}

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
    pub fn spawn(
        el: &ActiveEventLoop,
        frames: &[CapturedFrame],
        bounds: Rect,
        prefs: &Prefs,
    ) -> Result<Self> {
        let (bright, dim, cscale) = composite(frames, bounds);
        let mut s = Session {
            bounds,
            cscale,
            bright,
            dim,
            prefs: prefs.clone(),
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
            self.s.redraw_dirty();
        }
    }
}

/// Show the overlay one-shot: own event loop, blocks until the session ends.
/// Used by `--capture` CLI mode. `bounds` = virtual screen bounds.
pub fn select_region(
    frames: &[CapturedFrame],
    bounds: Rect,
    prefs: &Prefs,
) -> Result<Option<(Rect, Action, Vec<Annotation>)>> {
    struct CliRunner<'a> {
        frames: &'a [CapturedFrame],
        bounds: Rect,
        prefs: &'a Prefs,
        ov: Option<Overlay>,
        out: Option<Option<(Rect, Action, Vec<Annotation>)>>,
    }
    impl ApplicationHandler for CliRunner<'_> {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            match Overlay::spawn(el, self.frames, self.bounds, self.prefs) {
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
        prefs,
        ov: None,
        out: None,
    };
    el.run_app(&mut runner)?;
    Ok(runner.out.flatten())
}

/// Blit all frames into one virtual-space canvas (0xRRGGBB) at the densest
/// monitor's pixel scale — `canvas = logical * cscale`, so a Retina backdrop
/// stays sharp on the physical-res surface instead of upscaling a logical
/// buffer. Returns (bright, dimmed, cscale).
fn composite(frames: &[CapturedFrame], bounds: Rect) -> (Vec<u32>, Vec<u32>, f64) {
    let cscale = frames
        .iter()
        .map(|f| f.geometry.scale)
        .fold(1.0_f64, f64::max);
    let (w, h) = (
        (bounds.w as f64 * cscale).round() as usize,
        (bounds.h as f64 * cscale).round() as usize,
    );
    let mut canvas = vec![0u32; w * h];
    for f in frames {
        let mon = f.geometry.logical_rect();
        // this frame's region in canvas px
        let (dx0, dy0) = (
            ((mon.x - bounds.x) as f64 * cscale).round() as i64,
            ((mon.y - bounds.y) as f64 * cscale).round() as i64,
        );
        let (dw, dh) = (
            (mon.w as f64 * cscale).round() as u32,
            (mon.h as f64 * cscale).round() as u32,
        );
        let resized;
        let img = if f.pixels.width() == dw && f.pixels.height() == dh {
            &f.pixels
        } else {
            resized = imageops::resize(&f.pixels, dw, dh, imageops::FilterType::Triangle);
            &resized
        };
        for row in 0..img.height() {
            let dy = dy0 + row as i64;
            if dy < 0 || dy >= h as i64 {
                continue;
            }
            for col in 0..img.width() {
                let dx = dx0 + col as i64;
                if dx < 0 || dx >= w as i64 {
                    continue;
                }
                let p = img.get_pixel(col, row);
                canvas[dy as usize * w + dx as usize] =
                    ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32;
            }
        }
    }
    let dim = canvas.iter().map(|&p| (p & 0xFEFEFE) >> 1).collect();
    (canvas, dim, cscale)
}

struct WinCtx {
    win: Rc<Window>,
    surf: Surface<Rc<Window>, Rc<Window>>,
    /// this monitor's rect in virtual coords
    vrect: Rect,
    /// physical-res ARGB layer of committed strokes (lazy — allocated on
    /// the first commit so zero-annotation runs cost nothing)
    layer: Option<Vec<u32>>,
    /// dimmed frame for this monitor — rebuilt when the surface resizes;
    /// selection drags only patch the bright rect on top
    base_dim: Option<Vec<u32>>,
}

/// Pixelate undo record: canvas-space rect + the pixels it overwrote.
type PixBackup = (Rect, Vec<u32>);

/// A committed edit, in order — the undo log. Pixelate carries its pixel
/// backup here instead of a parallel stack.
enum Op {
    Add {
        pix: Option<PixBackup>,
    },
    Move {
        idx: usize,
        before: Annotation,
        after: Annotation,
    },
    Remove {
        idx: usize,
        annot: Annotation,
    },
}

/// Inverse data needed to re-apply an undone op.
enum RedoOp {
    Add(Annotation),
    Move { idx: usize, after: Annotation },
    Remove { idx: usize },
}

/// Annotation mid-move: pulled out of `annots` so it rides the preview
/// path; `before` is the pristine copy for cancel + the undo record.
struct Grab {
    idx: usize,
    annot: Annotation,
    before: Annotation,
    start: Point,
    /// last cursor position — the incremental-translate baseline
    last: Point,
}

struct Session {
    bounds: Rect,
    /// canvas px per logical px (= densest monitor's scale)
    cscale: f64,
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
    /// committed edits, in order — the undo log. Ops are applied and undone
    /// strictly LIFO, which keeps their `idx` fields valid indexes into
    /// `annots`
    ops: Vec<Op>,
    /// ops popped by Undo, holding what redo needs to re-apply them
    redo: Vec<RedoOp>,
    /// annotation index selected via the Select tool
    selected: Option<usize>,
    /// annotation being dragged — pulled out of `annots` until release
    grabbed: Option<Grab>,
    /// in-flight IME composition, rendered after the draft text
    preedit: String,
    /// editing caret / selection anchor — byte indexes into the draft
    /// text; `caret_anchor` set means [anchor..caret] is selected
    caret: usize,
    caret_anchor: Option<usize>,
    /// whether a space key-down was seen — a release without one means
    /// the IME ate the press as its commit trigger (winit#4526)
    space_down: bool,
    /// any IME event seen while this draft is live — gates the
    /// swallowed-space payback so stray releases don't insert text
    ime_seen: bool,
    /// current stroke size level / palette index / text weight+size
    size: u8,
    color: usize,
    bold: bool,
    /// index into annotate::TEXT_SIZES for the text tool
    font_size: usize,
    mods: winit::keyboard::ModifiersState,
    /// accent + action keys from app config
    prefs: Prefs,
    result: Option<(Rect, Action, Vec<Annotation>)>,
    done: bool,
    dirty: bool,
    wins: Vec<WinCtx>,
}

impl Session {
    fn default_state() -> Self {
        Self {
            bounds: Rect::default(),
            cscale: 1.0,
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
            ops: Vec::new(),
            redo: Vec::new(),
            selected: None,
            grabbed: None,
            preedit: String::new(),
            caret: 0,
            caret_anchor: None,
            space_down: false,
            ime_seen: false,
            size: 1,
            color: 0,
            bold: false,
            font_size: annotate::TEXT_SIZE_DEFAULT,
            mods: winit::keyboard::ModifiersState::empty(),
            prefs: Prefs::default(),
            result: None,
            done: false,
            dirty: false,
            wins: Vec::new(),
        }
    }

    /// Logical virtual rect -> canvas px rect (canvas = logical * cscale).
    fn canvas_rect(&self, r: Rect) -> Rect {
        Rect::new(
            ((r.x - self.bounds.x) as f64 * self.cscale).round() as i32,
            ((r.y - self.bounds.y) as f64 * self.cscale).round() as i32,
            (r.w as f64 * self.cscale).round().max(1.0) as u32,
            (r.h as f64 * self.cscale).round().max(1.0) as u32,
        )
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
        // annotations live inside the captured region only
        let p = self.sel_clamp(p);
        self.shape_anchor = p;
        self.caret = 0;
        self.caret_anchor = None;
        self.ime_seen = false;
        let rgb = annotate::PALETTE[self.color];
        let w = annotate::SIZES[self.size as usize];
        self.active = Some(match t {
            Tool::Select => return, // armed select grabs, never strokes
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
            Tool::Text => Annotation::Text {
                pos: p,
                text: String::new(),
                px: annotate::TEXT_SIZES[self.font_size].0,
                bold: self.bold,
                rgb,
            },
        });
        self.dirty = true;
    }

    /// Commit the in-progress stroke into the annotation layer / bright canvas.
    fn commit_annot(&mut self) {
        self.preedit.clear();
        self.caret_anchor = None;
        self.ime_seen = false;
        let Some(an) = self.active.take() else { return };
        let ok = match &an {
            Annotation::Pen { pts, .. } | Annotation::Marker { pts, .. } => pts.len() >= 2,
            Annotation::Arrow { a, b, .. } => a != b,
            Annotation::Rect { r, .. }
            | Annotation::Ellipse { r, .. }
            | Annotation::Pixelate(r) => r.w >= MIN_SELECT && r.h >= MIN_SELECT,
            Annotation::Text { text, .. } => !text.trim().is_empty(),
        };
        if !ok {
            return;
        }
        let op = if let Annotation::Pixelate(r) = an {
            let rc = self.canvas_rect(r);
            let (cw, ch) = self.canvas_dims();
            let backup = annotate::pixelate_canvas(
                &mut self.bright,
                cw,
                ch,
                rc,
                (annotate::PIX_BLOCK as f64 * self.cscale).round().max(2.0) as i32,
            );
            Op::Add {
                pix: Some((rc, backup)),
            }
        } else {
            self.blit_to_layers(&an);
            Op::Add { pix: None }
        };
        self.annots.push(an);
        self.ops.push(op);
        self.redo.clear(); // a fresh edit invalidates the redo stack
        self.sync_tb_flags();
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
            // pixelate lives in `bright`, not the stroke layer — rendering it
            // here would bake its preview border into the committed layer
            if a.movable() {
                self.blit_to_layers(a);
            }
        }
        self.annots = annots;
    }

    /// `annots` changed: layers and toolbar catch up, selection invalidates.
    fn after_edit(&mut self) {
        self.selected = None;
        self.rerender_layers();
        self.sync_tb_flags();
        self.dirty = true;
    }

    /// Index of the topmost annotation under `p` (last-to-first), if any.
    fn hit_top(&self, p: Point) -> Option<usize> {
        self.annots.iter().rposition(|a| a.hit(p))
    }

    /// Point clamped into the confirmed selection — drawing, writing
    /// and moving are confined to the captured region.
    fn sel_clamp(&self, p: Point) -> Point {
        match self.sel {
            Some(s) => Point::new(p.x.clamp(s.x, s.right()), p.y.clamp(s.y, s.bottom())),
            None => p,
        }
    }

    /// Ordered byte range of the draft-text selection, if non-empty.
    fn edit_range(&self) -> Option<(usize, usize)> {
        let a = self.caret_anchor?;
        (a != self.caret).then_some((a.min(self.caret), a.max(self.caret)))
    }

    /// Insert `s` at the caret, replacing any selected text.
    fn edit_insert(&mut self, s: &str) {
        let range = self.edit_range();
        if let Some(Annotation::Text { text, .. }) = &mut self.active {
            if let Some((a, b)) = range {
                text.drain(a..b);
                self.caret = a;
            }
            let c = floor_boundary(text, self.caret);
            text.insert_str(c, s);
            self.caret = c + s.len();
        }
        self.caret_anchor = None;
        self.dirty = true;
    }

    /// Backspace: delete the selection, else the char before the caret.
    fn edit_backspace(&mut self) {
        let range = self.edit_range();
        if let Some(Annotation::Text { text, .. }) = &mut self.active {
            if let Some((a, b)) = range {
                text.drain(a..b);
                self.caret = a;
            } else {
                let c = floor_boundary(text, self.caret);
                let prev = prev_boundary(text, c);
                text.drain(prev..c);
                self.caret = prev;
            }
        }
        self.caret_anchor = None;
        self.dirty = true;
    }

    /// Forward-delete: same, for the char after the caret.
    fn edit_delete(&mut self) {
        let range = self.edit_range();
        if let Some(Annotation::Text { text, .. }) = &mut self.active {
            if let Some((a, b)) = range {
                text.drain(a..b);
                self.caret = a;
            } else {
                let c = floor_boundary(text, self.caret);
                let next = next_boundary(text, c);
                text.drain(c..next);
            }
        }
        self.caret_anchor = None;
        self.dirty = true;
    }

    /// Arrow key: shift extends the selection, ⌘/Ctrl jumps to an end,
    /// plain arrows collapse the selection or step one char.
    fn edit_move(&mut self, left: bool) {
        let Some(Annotation::Text { text, .. }) = &self.active else {
            return;
        };
        if self.mods.shift_key() {
            self.caret_anchor.get_or_insert(self.caret);
        } else if let Some((a, b)) = self.edit_range() {
            self.caret = if left { a } else { b };
            self.caret_anchor = None;
            self.dirty = true;
            return;
        } else {
            self.caret_anchor = None;
        }
        let c = floor_boundary(text, self.caret);
        self.caret = if self.mods.super_key() || self.mods.control_key() {
            if left {
                0
            } else {
                text.len()
            }
        } else if left {
            prev_boundary(text, c)
        } else {
            next_boundary(text, c)
        };
        self.dirty = true;
    }

    /// ⌘A: select the whole draft.
    fn edit_select_all(&mut self) {
        if let Some(Annotation::Text { text, .. }) = &self.active {
            self.caret_anchor = Some(0);
            self.caret = text.len();
            self.dirty = true;
        }
    }

    /// Physical dims of the `bright` canvas, for pixel-edit ops.
    fn canvas_dims(&self) -> (usize, usize) {
        (
            (self.bounds.w as f64 * self.cscale).round() as usize,
            (self.bounds.h as f64 * self.cscale).round() as usize,
        )
    }

    fn sync_tb_flags(&mut self) {
        if let Some(tb) = &mut self.toolbar {
            tb.can_undo = !self.ops.is_empty();
            tb.has_redo = !self.redo.is_empty();
        }
    }

    fn undo(&mut self) {
        // while an annot is detached in `grabbed`, op indexes into `annots`
        // would desync — no undo until the drag settles
        if self.grabbed.is_some() {
            return;
        }
        let Some(op) = self.ops.pop() else { return };
        match op {
            Op::Add { pix } => {
                let an = self.annots.pop().expect("Add op without annotation");
                if let Some((rc, back)) = pix {
                    let (cw, ch) = self.canvas_dims();
                    annotate::restore_canvas(&mut self.bright, cw, ch, rc, &back);
                }
                self.redo.push(RedoOp::Add(an));
            }
            Op::Move { idx, before, after } => {
                self.annots[idx] = before;
                self.redo.push(RedoOp::Move { idx, after });
            }
            Op::Remove { idx, annot } => {
                self.annots.insert(idx, annot);
                self.redo.push(RedoOp::Remove { idx });
            }
        }
        self.after_edit();
    }

    fn redo(&mut self) {
        if self.grabbed.is_some() {
            return;
        }
        let Some(op) = self.redo.pop() else {
            return;
        };
        match op {
            RedoOp::Add(an) => {
                let pix = if let Annotation::Pixelate(r) = an {
                    // deterministic: re-pixelating the restored pixels
                    // reproduces both the edit and the backup it had
                    let rc = self.canvas_rect(r);
                    let (cw, ch) = self.canvas_dims();
                    let nb = annotate::pixelate_canvas(
                        &mut self.bright,
                        cw,
                        ch,
                        rc,
                        (annotate::PIX_BLOCK as f64 * self.cscale).round().max(2.0) as i32,
                    );
                    Some((rc, nb))
                } else {
                    None
                };
                self.annots.push(an);
                self.ops.push(Op::Add { pix });
            }
            RedoOp::Move { idx, after } => {
                let before = std::mem::replace(&mut self.annots[idx], after.clone());
                self.ops.push(Op::Move { idx, before, after });
            }
            RedoOp::Remove { idx } => {
                let annot = self.annots.remove(idx);
                self.ops.push(Op::Remove { idx, annot });
            }
        }
        self.after_edit();
    }

    /// Reset annotation state when the user starts a fresh selection.
    fn clear_annots(&mut self) {
        self.annots.clear();
        self.redo.clear();
        self.active = None;
        self.tool = None;
        self.selected = None;
        self.grabbed = None;
        for w in &mut self.wins {
            w.layer = None;
        }
        // newest-first: overlapping pixelate backups must peel back in LIFO
        // order or the newer backup re-stamps scrambled pixels over the
        // older one's originals
        let (cw, ch) = self.canvas_dims();
        for op in self.ops.drain(..).rev() {
            if let Op::Add {
                pix: Some((rc, back)),
            } = op
            {
                annotate::restore_canvas(&mut self.bright, cw, ch, rc, &back);
            }
        }
        self.preedit.clear();
        self.ime_seen = false;
    }

    fn redraw_all(&self) {
        for w in &self.wins {
            w.win.request_redraw();
        }
    }

    /// Coalesced dirty pass — during a selection drag only the monitor(s)
    /// touched by the preview rect need a full present.
    fn redraw_dirty(&self) {
        let preview = self
            .anchor
            .map(|a| Rect::from_drag(a, self.cursor))
            .or(self.sel);
        if let Some(r) = preview {
            let mut any = false;
            for w in &self.wins {
                if r.intersection(&w.vrect).is_some() {
                    w.win.request_redraw();
                    any = true;
                }
            }
            if !any {
                self.redraw_all();
            }
        } else {
            self.redraw_all();
        }
    }

    fn fill_base_dim(
        vrect: Rect,
        bounds: Rect,
        dim: &[u32],
        cscale: f64,
        sw: usize,
        sh: usize,
        scale: f64,
    ) -> Vec<u32> {
        let k = cscale / scale;
        let (oxc, oyc) = (
            (vrect.x - bounds.x) as f64 * cscale,
            (vrect.y - bounds.y) as f64 * cscale,
        );
        let (cw, ch) = (
            (bounds.w as f64 * cscale).round() as i32,
            (bounds.h as f64 * cscale).round() as i32,
        );
        let colmap: Vec<i32> = (0..sw).map(|x| (oxc + x as f64 * k) as i32).collect();
        let rowmap: Vec<i32> = (0..sh).map(|y| (oyc + y as f64 * k) as i32).collect();
        let mut buf = vec![0u32; sw * sh];
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
                        dim[src + cx as usize]
                    } else {
                        0
                    });
                }
                rowbuf_cy = cy;
            }
            buf[y * sw..y * sw + sw].copy_from_slice(&rowbuf);
        }
        buf
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
        // resolved before the `wins` borrow below — caret state is a
        // plain field, the range helper needs `&self`
        let sel_range = self.edit_range();
        let wctx = &mut self.wins[i];
        let size = wctx.win.inner_size();
        let (sw, sh) = match (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            (Some(w), Some(h)) => (w, h),
            _ => return,
        };
        if wctx.surf.resize(sw, sh).is_err() {
            return;
        }
        let (sw, sh) = (sw.get() as usize, sh.get() as usize);
        // Surface coords and canvas coords are both *physical* px — the
        // canvas is `logical * cscale`; the window surface is `logical *
        // scale_factor` (2.0 on Retina). `k` converts between them.
        let scale = wctx.win.scale_factor();
        let k = self.cscale / scale;
        // canvas offset of this monitor (canvas px)
        let (oxc, oyc) = (
            (wctx.vrect.x - self.bounds.x) as f64 * self.cscale,
            (wctx.vrect.y - self.bounds.y) as f64 * self.cscale,
        );
        let (cw, ch) = (
            (self.bounds.w as f64 * self.cscale).round() as i32,
            (self.bounds.h as f64 * self.cscale).round() as i32,
        );
        // surface (x,y) -> canvas (cx, cy); precompute per-row/col mapping
        let colmap: Vec<i32> = (0..sw).map(|x| (oxc + x as f64 * k) as i32).collect();
        let rowmap: Vec<i32> = (0..sh).map(|y| (oyc + y as f64 * k) as i32).collect();
        let vrect = wctx.vrect;
        if wctx.base_dim.as_ref().is_none_or(|b| b.len() != sw * sh) {
            wctx.base_dim = Some(Self::fill_base_dim(
                vrect,
                self.bounds,
                &self.dim,
                self.cscale,
                sw,
                sh,
                scale,
            ));
        }
        // softbuffer's buffer_mut() borrows `surf`; clone so we don't also
        // hold an imm borrow of `base_dim` on the same WinCtx.
        let base_dim = wctx.base_dim.clone().unwrap_or_default();
        let Ok(mut buf) = wctx.surf.buffer_mut() else {
            return;
        };
        if base_dim.len() == sw * sh {
            buf.copy_from_slice(&base_dim);
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
                let b = (1.5 * scale).round().max(1.0) as i32;
                let (lx0, ly0) = (
                    (local.x as f64 * scale) as i32,
                    (local.y as f64 * scale) as i32,
                );
                let (lx1, ly1) = (
                    (local.right() as f64 * scale) as i32,
                    (local.bottom() as f64 * scale) as i32,
                );
                frame_rect(&mut buf, sw, sh, lx0, ly0, lx1, ly1, b, self.prefs.accent);
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
                // same filter as rerender_layers: pixelate lives in `bright`
                for a in self.annots.iter().filter(|a| a.movable()) {
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
        // in-flight IME composition spliced at the caret — built once,
        // shared by the preview render and the caret position below
        let composed = match &self.active {
            Some(Annotation::Text {
                pos,
                text,
                px,
                bold,
                rgb,
            }) if !self.preedit.is_empty() => {
                let c = floor_boundary(text, self.caret);
                Some(Annotation::Text {
                    pos: *pos,
                    text: format!("{}{}{}", &text[..c], self.preedit, &text[c..]),
                    px: *px,
                    bold: *bold,
                    rgb: *rgb,
                })
            }
            _ => None,
        };
        // in-progress stroke preview at physical res
        if let Some(an) = &self.active {
            composed.as_ref().unwrap_or(an).render_phys(
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
        // selection highlight + caret at the editing position — the only
        // visible marker while the draft text is still empty
        if let Some(Annotation::Text {
            pos,
            text,
            px,
            bold,
            rgb,
        }) = &self.active
        {
            if let Some((a, b)) = sel_range {
                let (wa, _) = annotate::text_extent(&text[..a], *px, *bold);
                let (wb, _) = annotate::text_extent(&text[..b], *px, *bold);
                let x0 = ((pos.x - wctx.vrect.x) as f64 * scale + wa * scale) as i32;
                let x1 = ((pos.x - wctx.vrect.x) as f64 * scale + wb * scale) as i32;
                let y0 = ((pos.y - wctx.vrect.y) as f64 * scale) as i32;
                let y1 = y0 + (*px * scale) as i32;
                for x in x0..x1 {
                    for y in y0..y1 {
                        if x >= 0 && y >= 0 && (x as usize) < sw && (y as usize) < sh {
                            let i = y as usize * sw + x as usize;
                            let d = buf[i];
                            let f = 0.35;
                            let mix = |s: u32, d: u32| {
                                (s as f64 * f + d as f64 * (1.0 - f)).round() as u32
                            };
                            let acc = self.prefs.accent;
                            buf[i] = mix((acc >> 16) & 0xFF, (d >> 16) & 0xFF) << 16
                                | mix((acc >> 8) & 0xFF, (d >> 8) & 0xFF) << 8
                                | mix(acc & 0xFF, d & 0xFF);
                        }
                    }
                }
            }
            let c = floor_boundary(text, self.caret);
            let (wc, _) = annotate::text_extent(&text[..c], *px, *bold);
            let (wp, _) = annotate::text_extent(&self.preedit, *px, *bold);
            let x0 = ((pos.x - wctx.vrect.x) as f64 * scale + (wc + wp) * scale + scale) as i32;
            let y0 = ((pos.y - wctx.vrect.y) as f64 * scale) as i32;
            let y1 = y0 + (*px * scale) as i32;
            let cw = (1.6 * scale).round().max(1.0) as i32;
            for x in x0..x0 + cw {
                for y in y0..y1 {
                    if x >= 0 && y >= 0 && (x as usize) < sw && (y as usize) < sh {
                        buf[y as usize * sw + x as usize] = *rgb;
                    }
                }
            }
            // keep the IME candidate popup anchored under the caret
            wctx.win.set_ime_cursor_area(
                PhysicalPosition::new(x0 + cw, y0),
                PhysicalSize::new(cw as u32, (y1 - y0) as u32),
            );
        }
        // grabbed annotation rides the preview path — it left `annots` and
        // the window layers on grab, so it needs live rendering
        if let Some(g) = &self.grabbed {
            g.annot.render_phys(
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
        // accent outline on the selected or grabbed annotation
        let outlined = self
            .grabbed
            .as_ref()
            .map(|g| &g.annot)
            .or_else(|| self.selected.and_then(|i| self.annots.get(i)));
        if let Some(an) = outlined {
            if let Some(l) = an
                .bbox()
                .intersection(&wctx.vrect)
                .map(|r| r.translated(-wctx.vrect.x, -wctx.vrect.y))
            {
                let b = (1.2 * scale).round().max(1.0) as i32;
                let (x0, y0) = ((l.x as f64 * scale) as i32, (l.y as f64 * scale) as i32);
                let (x1, y1) = (
                    (l.right() as f64 * scale) as i32,
                    (l.bottom() as f64 * scale) as i32,
                );
                frame_rect(&mut buf, sw, sh, x0, y0, x1, y1, b, self.prefs.accent);
            }
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
            // without this winit never routes keys through the input context,
            // so IME-composed text (Vietnamese Telex, CJK) cannot arrive
            win.set_ime_allowed(true);
            let surf = Context::new(win.clone()).and_then(|c| Surface::new(&c, win.clone()));
            match surf {
                Ok(surf) => self.wins.push(WinCtx {
                    win,
                    surf,
                    vrect: Self::vrect_of(&m),
                    layer: None,
                    base_dim: None,
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
                if let Some(g) = &mut self.grabbed {
                    // incremental integer delta — no drift, no per-frame
                    // clone of `before`
                    let dx = self.cursor.x - g.last.x;
                    let dy = self.cursor.y - g.last.y;
                    if dx != 0 || dy != 0 {
                        g.annot.translate(dx, dy);
                        // blocks stay inside the captured region
                        if let Some(s) = self.sel {
                            let b = g.annot.bbox();
                            let fx = if b.x < s.x {
                                s.x - b.x
                            } else if b.right() > s.right() {
                                s.right() - b.right()
                            } else {
                                0
                            };
                            let fy = if b.y < s.y {
                                s.y - b.y
                            } else if b.bottom() > s.bottom() {
                                s.bottom() - b.bottom()
                            } else {
                                0
                            };
                            g.annot.translate(fx, fy);
                        }
                        g.last = self.cursor;
                        self.dirty = true;
                    }
                } else if self.anchor.is_some() || self.active.is_some() {
                    let p = self.sel_clamp(self.cursor);
                    if let Some(an) = &mut self.active {
                        match an {
                            Annotation::Pen { pts, .. } | Annotation::Marker { pts, .. } => {
                                pts.push(p)
                            }
                            Annotation::Arrow { b, .. } => *b = p,
                            Annotation::Rect { r, .. }
                            | Annotation::Ellipse { r, .. }
                            | Annotation::Pixelate(r) => *r = Rect::from_drag(self.shape_anchor, p),
                            Annotation::Text { .. } => {}
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
                        } else if self.tool == Some(Tool::Select) {
                            if self.hit_top(self.cursor).is_some() {
                                winit::window::CursorIcon::Move
                            } else {
                                winit::window::CursorIcon::Default
                            }
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
                        // actions and tool switches end a text edit so it
                        // bakes into the result; option clicks (size, color,
                        // bold) live-update the draft and keep it open
                        if matches!(item, Item::Act(_) | Item::Tool(_)) {
                            self.commit_annot();
                        }
                        match item {
                            Item::Act(action) => {
                                eprintln!("overlay action: {action:?} (toolbar click)");
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
                            Item::FontSize(s) => {
                                self.font_size = s as usize;
                                if let Some(tb) = &mut self.toolbar {
                                    tb.font_size = s;
                                }
                                if let Some(Annotation::Text { px, .. }) = &mut self.active {
                                    *px = annotate::TEXT_SIZES[s as usize].0;
                                }
                                self.dirty = true;
                            }
                            Item::Color(c) => {
                                self.color = c;
                                if let Some(tb) = &mut self.toolbar {
                                    tb.color = c;
                                }
                                if let Some(Annotation::Text { rgb, .. }) = &mut self.active {
                                    *rgb = annotate::PALETTE[c];
                                }
                                self.dirty = true;
                            }
                            Item::Bold => {
                                self.bold = !self.bold;
                                if let Some(tb) = &mut self.toolbar {
                                    tb.bold = self.bold;
                                }
                                if let Some(Annotation::Text { bold, .. }) = &mut self.active {
                                    *bold = self.bold;
                                }
                                self.dirty = true;
                            }
                            Item::Tool(t) => {
                                let nt = if self.tool == Some(t) { None } else { Some(t) };
                                self.tool = nt;
                                if nt != Some(Tool::Select) {
                                    self.selected = None;
                                }
                                let mr = self.sel.map(|s| self.sel_monitor(s));
                                if let Some(tb) = &mut self.toolbar {
                                    tb.tool = nt;
                                    // options row toggles -> reflow the bar
                                    // (Select has no stroke options to show)
                                    if let (Some(sel), Some(mr)) = (self.sel, mr) {
                                        tb.layout(
                                            sel,
                                            mr,
                                            nt.is_some() && nt != Some(Tool::Select),
                                        );
                                    }
                                    tb.hover = None;
                                }
                                self.dirty = true;
                                self.wins[i].win.set_cursor(match self.tool {
                                    Some(Tool::Text) => winit::window::CursorIcon::Text,
                                    Some(Tool::Select) => winit::window::CursorIcon::Default,
                                    Some(_) => winit::window::CursorIcon::Crosshair,
                                    None => winit::window::CursorIcon::Pointer,
                                });
                            }
                        }
                        return;
                    }
                    if let Some(tb) = &self.toolbar {
                        if tb.contains(self.cursor) {
                            return; // padding area: swallow, don't start a drag
                        }
                        // armed tool: a click outside the toolbar starts a
                        // stroke — or moves the text caret when typing
                        if let Some(t) = self.tool {
                            if t == Tool::Select {
                                // grab the topmost picked annotation: it
                                // leaves `annots` and rides the preview path
                                if let Some(i) = self.hit_top(self.cursor) {
                                    let an = self.annots.remove(i);
                                    self.grabbed = Some(Grab {
                                        idx: i,
                                        before: an.clone(),
                                        annot: an,
                                        start: self.cursor,
                                        last: self.cursor,
                                    });
                                    self.selected = None;
                                    self.rerender_layers();
                                } else {
                                    self.selected = None;
                                }
                                self.dirty = true;
                                return;
                            }
                            // click inside the live draft moves the caret
                            // instead of ending the edit
                            if t == Tool::Text
                                && self.active.as_ref().is_some_and(|a| a.hit(self.cursor))
                            {
                                if let Some(i) = self
                                    .active
                                    .as_ref()
                                    .and_then(|a| a.text_index_at(self.cursor))
                                {
                                    self.caret = i;
                                    self.caret_anchor = None;
                                    self.dirty = true;
                                }
                                return;
                            }
                            self.commit_annot();
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
                    if let Some(g) = self.grabbed.take() {
                        // a drag is a Move op; a bare click just selects
                        if self.cursor != g.start {
                            self.ops.push(Op::Move {
                                idx: g.idx,
                                before: g.before,
                                after: g.annot.clone(),
                            });
                            self.redo.clear();
                        }
                        self.annots.insert(g.idx, g.annot);
                        self.after_edit();
                        if self.cursor == g.start {
                            self.selected = Some(g.idx);
                        }
                        return;
                    }
                    if self.active.is_some() {
                        // text stays live for keyboard input until Enter /
                        // the next click — strokes commit on release
                        if !matches!(self.active, Some(Annotation::Text { .. })) {
                            self.commit_annot();
                        }
                        return;
                    }
                    let Some(a) = self.anchor.take() else { return };
                    let sel = Rect::from_drag(a, self.cursor);
                    if sel.w >= MIN_SELECT && sel.h >= MIN_SELECT {
                        let mr = self.sel_monitor(sel);
                        self.sel = Some(sel);
                        let mut tb = toolbar::Toolbar::new(sel, mr, false);
                        tb.accent = self.prefs.accent;
                        self.toolbar = Some(tb);
                        self.dirty = true;
                    } else {
                        self.finish(None);
                    }
                }
            },
            // IME-composed text (Vietnamese Telex/VNI, dead keys) arrives as
            // Commit strings, not KeyboardInput; Preedit is the in-flight
            // composition — shown so the user can see what is being typed
            WindowEvent::Ime(winit::event::Ime::Preedit(s, _)) => {
                if matches!(self.active, Some(Annotation::Text { .. })) {
                    self.preedit = s;
                    self.ime_seen = true;
                    self.dirty = true;
                }
            }
            WindowEvent::Ime(winit::event::Ime::Commit(s)) => {
                self.preedit.clear();
                if matches!(self.active, Some(Annotation::Text { .. })) {
                    self.ime_seen = true;
                    self.edit_insert(&s);
                }
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            // a space the IME ate as its commit trigger leaks its release
            // without a press — pay it back, once (winit#4526)
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Released => {
                let swallowed =
                    matches!(event.logical_key, Key::Named(NamedKey::Space)) && !self.space_down;
                self.space_down = false;
                if swallowed
                    && self.ime_seen
                    && self.preedit.is_empty()
                    && !self.mods.super_key()
                    && !self.mods.control_key()
                {
                    if let Some(Annotation::Text { text, .. }) = &mut self.active {
                        // a commit that already carried the space isn't
                        // owed a second one
                        let c = floor_boundary(text, self.caret);
                        if !text[..c].ends_with(' ') {
                            text.insert_str(c, " ");
                            self.caret = c + 1;
                            self.dirty = true;
                        }
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if matches!(event.logical_key, Key::Named(NamedKey::Space)) {
                    self.space_down = true;
                }
                // text tool owns the keyboard while a Text annotation is
                // live: a real editing box — caret, selection, arrows,
                // ⌘A; Enter commits, Esc drops the draft
                if matches!(self.active, Some(Annotation::Text { .. })) {
                    match &event.logical_key {
                        Key::Named(NamedKey::Escape) => {
                            self.active = None;
                            self.preedit.clear();
                            self.ime_seen = false;
                        }
                        Key::Named(NamedKey::Enter) => self.commit_annot(),
                        Key::Named(NamedKey::Backspace) => self.edit_backspace(),
                        Key::Named(NamedKey::Delete) => self.edit_delete(),
                        Key::Named(NamedKey::ArrowLeft) => self.edit_move(true),
                        Key::Named(NamedKey::ArrowRight) => self.edit_move(false),
                        Key::Named(NamedKey::Space) => self.edit_insert(" "),
                        Key::Character(c)
                            if (self.mods.super_key() || self.mods.control_key())
                                && c.as_str() == "a" =>
                        {
                            self.edit_select_all()
                        }
                        Key::Character(s) if !self.mods.super_key() && !self.mods.control_key() => {
                            self.edit_insert(s.as_str());
                        }
                        _ => {}
                    }
                    self.dirty = true;
                    return;
                }
                // Esc mid-grab: cancel the move, keep the annotation
                if matches!(event.logical_key, Key::Named(NamedKey::Escape))
                    && self.grabbed.is_some()
                {
                    if let Some(g) = self.grabbed.take() {
                        self.annots.insert(g.idx, g.before);
                        self.after_edit();
                    }
                    return;
                }
                // Select tool: Esc deselects, Delete/Backspace removes it
                if let Some(i) = self.selected {
                    match &event.logical_key {
                        Key::Named(NamedKey::Escape) => {
                            self.selected = None;
                            self.dirty = true;
                            return;
                        }
                        Key::Named(NamedKey::Backspace) | Key::Named(NamedKey::Delete) => {
                            let annot = self.annots.remove(i);
                            self.ops.push(Op::Remove { idx: i, annot });
                            self.redo.clear();
                            self.after_edit();
                            return;
                        }
                        _ => {}
                    }
                }
                if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
                    self.finish(None);
                    return;
                }
                // Toolbar actions — same keys as the buttons. Ctrl/⌘+key is
                // the documented path; plain keys (S/C/…) work too so Linux
                // users aren't stuck when they skip the modifier.
                if self.sel.is_some() && self.toolbar.is_some() {
                    if let Key::Character(c) = &event.logical_key {
                        let Some(ch) = c.chars().next().map(|c| c.to_ascii_lowercase()) else {
                            return;
                        };
                        let modded = self.mods.super_key() || self.mods.control_key();
                        if modded && ch == self.prefs.keys.undo {
                            if self.mods.shift_key() {
                                self.redo();
                            } else {
                                self.undo();
                            }
                            return;
                        }
                        // save/copy: plain S/C or Ctrl/⌘+key; rest need modifier
                        let k = &self.prefs.keys;
                        let act = if ch == k.save {
                            Some(Action::Save)
                        } else if ch == k.copy {
                            Some(Action::Copy)
                        } else if modded {
                            match ch {
                                x if x == k.ocr => Some(Action::Ocr),
                                x if x == k.share => Some(Action::Share),
                                x if x == k.scroll => Some(Action::Scroll),
                                x if x == k.record => Some(Action::Record),
                                _ => None,
                            }
                        } else {
                            None
                        };
                        if let Some(a) = act {
                            eprintln!("overlay action: {a:?} (key '{ch}')");
                            self.finish(Some(a));
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_boundaries_are_unicode_safe() {
        // 'ệ' is 3 UTF-8 bytes — index arithmetic must never split it
        let s = "việt"; // v=0, i=1, ệ=2..5, t=5
        assert_eq!(s.len(), 6);
        // inside the 'ệ': floor snaps back to its first byte
        assert_eq!(floor_boundary(s, 3), 2);
        assert_eq!(floor_boundary(s, 99), s.len());
        // prev/next walk whole chars, not bytes
        assert_eq!(prev_boundary(s, 4), 1);
        assert_eq!(prev_boundary(s, 5), 2);
        assert_eq!(next_boundary(s, 1), 2);
        assert_eq!(next_boundary(s, 2), 5);
        assert_eq!(next_boundary(s, s.len()), s.len());
        assert_eq!(prev_boundary(s, 0), 0);
    }
}
