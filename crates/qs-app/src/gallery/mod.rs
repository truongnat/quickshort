//! Capture history gallery — a floating dark window with the recent
//! captures as a thumbnail grid. Opens with a cascade (window content
//! rises, cells follow one by one), thumbnails lift on hover, and each
//! cell carries Copy/Pin/Delete actions. Esc, the corner ×, or focus
//! loss slide it back down.
//!
//! Thumbnails load on a worker thread (`load`) and arrive through
//! `HostEvent::GalleryData` so the event loop never blocks on decode.

use anyhow::Result;
use qs_overlay::sdf::{ellipse_sdf, fill_cov, rrect_sdf, seg_sdf, stroke_cov, Canvas};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::WindowId;

use crate::widgets::{card_chrome, pill, Txt};

const COLS: usize = 3;
const W: f64 = 664.0;
const H: f64 = 500.0;
const PAD: f64 = 16.0;
const GAP: f64 = 14.0;
const HEADER: f64 = 52.0;
const CELL_W: f64 = (W - PAD * 2.0 - GAP * (COLS - 1) as f64) / COLS as f64;
const THUMB_H: f64 = 132.0;
const CELL_H: f64 = 178.0;
const CLOSE_R: f64 = 9.0;

const FRAME: Duration = Duration::from_millis(16);
const STAGGER: f64 = 0.045; // seconds between cells entering
const ENTER: f64 = 0.26; // per-cell rise duration
const ENTER_DY: f64 = 26.0; // px each cell climbs while fading in
const OPEN_SLIDE: f64 = 0.24;
const OPEN_DY: f64 = 34.0; // whole content group rises this much
const CLOSE_SLIDE: f64 = 0.18;
const CLOSE_DY: f64 = 30.0;
const ARM_TTL: f64 = 2.0; // delete confirm auto-cancels after this

const BG: u32 = 0xFBFBFD;
const EDGE: u32 = 0xDADADE;
const FRINGE: u32 = 0x8E8E96;
const FG: u32 = 0x1E1F24;
const DIM: u32 = 0x6B7078;
const CHIP: u32 = 0xEFEFF3;
const HOT: u32 = 0xE2E2E8;
const X_BG: u32 = 0xE2E2E8;
const DANGER: u32 = 0xD92D20;
const EMPTY: u32 = 0xA2A5AD;

/// Everything `load` produces for one cell — plain data, no windowing.
pub struct CellData {
    pub path: PathBuf,
    /// fitted thumbnail, ARGB — already at display size
    pub px: Vec<u32>,
    pub tw: usize,
    pub th: usize,
    /// `"1920×1080"` of the source image
    pub dims: String,
    /// relative mtime — "just now", "5m ago", "3d ago"
    pub age: String,
}

/// Newest `n` captures decoded + thumbnailed. Runs off the UI thread.
pub fn load(dir: &Path, n: usize) -> Vec<CellData> {
    crate::actions::recent_captures(dir, n)
        .into_iter()
        .filter_map(|p| {
            let img = image::open(&p).ok()?.to_rgba8();
            let (iw, ih) = (img.width(), img.height());
            let s = (CELL_W / iw as f64).min(THUMB_H / ih as f64).min(1.0);
            let (tw, th) = (
                (iw as f64 * s).round().max(1.0) as usize,
                (ih as f64 * s).round().max(1.0) as usize,
            );
            let t = image::imageops::resize(&img, tw as u32, th as u32, image::imageops::Triangle);
            let px = t
                .pixels()
                .map(|c| ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32)
                .collect();
            let age = std::fs::metadata(&p)
                .and_then(|m| m.modified())
                .map(rel_age)
                .unwrap_or_default();
            Some(CellData {
                path: p,
                px,
                tw,
                th,
                dims: format!("{iw}×{ih}"),
                age,
            })
        })
        .collect()
}

/// "5m ago" / "2h ago" / "3d ago" — no chrono, durations only.
fn rel_age(t: SystemTime) -> String {
    let d = SystemTime::now()
        .duration_since(t)
        .unwrap_or_default()
        .as_secs();
    if d < 60 {
        "just now".into()
    } else if d < 3600 {
        format!("{}m ago", d / 60)
    } else if d < 86400 {
        format!("{}h ago", d / 3600)
    } else {
        format!("{}d ago", d / 86400)
    }
}

/// What the host should do — every variant carries the cell's file.
#[derive(Clone)]
pub enum GalleryEvent {
    Open(PathBuf),
    Pin(PathBuf),
    Copy(PathBuf),
    /// user confirmed the two-step delete — remove the file for real
    Delete(PathBuf),
    Closed,
}

/// `about_to_wait` contract: Wake polls again, Idle sleeps, Dead drops.
pub enum Pump {
    Wake(Instant),
    Idle,
    Dead,
}

#[derive(Clone, Copy, PartialEq)]
enum Pill {
    Copy,
    Pin,
    Del,
}

struct Cell {
    d: CellData,
    /// eased hover progress 0..1 — drives the lift + pills
    hot: f32,
    /// first Del click armed; second within ARM_TTL confirms
    armed: Option<Instant>,
    /// dying cells shrink away before removal
    dying: Option<Instant>,
}

#[derive(Clone, Copy)]
enum Phase {
    Open(Instant),
    Live,
    /// `from` keeps the exit continuous when dismissed mid-entrance.
    Close { t0: Instant, from: f64 },
}

/// Logical rect of cell `i` in grid space (scroll applied).
fn cell_rect(scroll: f64, i: usize) -> (f64, f64) {
    let (c, r) = (i % COLS, i / COLS);
    (
        PAD + c as f64 * (CELL_W + GAP),
        HEADER + PAD + r as f64 * (CELL_H + GAP) - scroll,
    )
}

/// Per-cell action pills along the thumb's bottom edge — Copy, Pin,
/// Delete right-to-left. Rects are (x, y, w) in window coords.
fn pills(cx: f64, cy: f64) -> [(Pill, f64, f64, f64); 3] {
    let y = cy + THUMB_H - 26.0;
    let mut x = cx + CELL_W - 8.0;
    let mut out = [(Pill::Copy, 0.0, y, 0.0); 3];
    for (n, (p, w)) in [(Pill::Del, 34.0), (Pill::Pin, 32.0), (Pill::Copy, 46.0)]
        .into_iter()
        .enumerate()
    {
        x -= w;
        out[2 - n] = (p, x, y, w);
        x -= 6.0;
    }
    out
}

/// Read-only snapshot of the animating grid — `draw` builds it before
/// `buffer_mut` takes an exclusive borrow of the surface.
struct Grid<'a> {
    cells: &'a [Cell],
    scroll: f64,
    open_t: Instant,
    accent: u32,
    pill_hot: Option<Pill>,
    hover_close: bool,
    closing: bool,
}

fn content_h(n: usize) -> f64 {
    let rows = n.div_ceil(COLS) as f64;
    PAD + rows * CELL_H + (rows - 1.0).max(0.0) * GAP + PAD
}

fn max_scroll(n: usize) -> f64 {
    (content_h(n) - (H - HEADER)).max(0.0)
}

#[cfg(target_os = "linux")]
mod wayland;
mod winit_win;

/// The gallery surface the host owns. On Wayland it's a layer-shell
/// palette anchored under the top bar (self-driven on its own dispatch
/// thread); everywhere else it's the plain winit window the host pumps.
pub enum Gallery {
    #[cfg(target_os = "linux")]
    Wl(wayland::WlGallery),
    Win(winit_win::WinGallery),
}

impl Gallery {
    /// `px` carries gallery actions back as `HostEvent::Gallery` —
    /// the layer thread needs it; the winit path reports synchronously.
    pub fn open(
        el: &ActiveEventLoop,
        cells: Vec<CellData>,
        accent: u32,
        px: EventLoopProxy<crate::host::HostEvent>,
    ) -> Result<Self> {
        #[cfg(target_os = "linux")]
        match wayland::WlGallery::spawn(cells, accent, px) {
            Ok(g) => return Ok(Self::Wl(g)),
            // cells come back on failure so the fallback still has them
            Err((e, cells)) => {
                eprintln!("layer-shell unavailable ({e:#}) — window fallback");
                return winit_win::WinGallery::open(el, cells, accent).map(Self::Win);
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = px;
            winit_win::WinGallery::open(el, cells, accent).map(Self::Win)
        }
    }

    /// `None` for surfaces winit never sees (layer-shell thread).
    pub fn win_id(&self) -> Option<WindowId> {
        match self {
            Self::Win(g) => Some(g.win_id()),
            #[cfg(target_os = "linux")]
            Self::Wl(_) => None,
        }
    }

    pub fn window_event(&mut self, ev: WindowEvent) -> Option<GalleryEvent> {
        match self {
            Self::Win(g) => g.window_event(ev),
            #[cfg(target_os = "linux")]
            Self::Wl(_) => None,
        }
    }

    pub fn pump(&mut self) -> Pump {
        match self {
            Self::Win(g) => g.pump(),
            #[cfg(target_os = "linux")]
            Self::Wl(g) => g.pump(),
        }
    }
}

/// What a primary-button click resolved to.
enum Hit {
    /// × clicked — the close animation is already running.
    Closed,
    /// Cell or pill action — the host performs it.
    Event(GalleryEvent),
    /// Empty header strip — the winit window drags itself from here.
    Header,
    /// Nothing actionable (armed a delete, empty space) — still wants a frame.
    Miss,
}

/// `State::tick` outcome — each backend maps it to its own frame request.
enum Tick {
    Busy,
    Idle,
    Dead,
}

/// Interaction state shared by every backend — pointer/keyboard events
/// feed it, `tick` advances animation, `grid` yields the render view.
/// No windowing here on purpose: layer.rs and winit_win.rs own that.
struct State {
    cells: Vec<Cell>,
    accent: u32,
    cursor: (f64, f64),
    /// hovered cell index; pill hit-testing derives from it
    hover: Option<usize>,
    /// hovered pill inside the hovered cell
    pill_hot: Option<Pill>,
    /// grid scroll — `to` is the wheel target, eased toward in tick
    scroll: f64,
    scroll_to: f64,
    open_t: Instant,
    phase: Phase,
    hover_close: bool,
}

impl State {
    fn new(cells: Vec<CellData>, accent: u32) -> Self {
        let now = Instant::now();
        Self {
            cells: cells
                .into_iter()
                .map(|d| Cell {
                    d,
                    hot: 0.0,
                    armed: None,
                    dying: None,
                })
                .collect(),
            accent,
            cursor: (-1.0, -1.0),
            hover: None,
            pill_hot: None,
            scroll: 0.0,
            scroll_to: 0.0,
            open_t: now,
            phase: Phase::Open(now),
            hover_close: false,
        }
    }

    /// Read-only view for the render pass.
    fn grid(&self) -> Grid<'_> {
        Grid {
            cells: &self.cells,
            scroll: self.scroll,
            open_t: self.open_t,
            accent: self.accent,
            pill_hot: self.pill_hot,
            hover_close: self.hover_close,
            closing: matches!(self.phase, Phase::Close { .. }),
        }
    }

    /// Current content offset — render and begin_close share it so the
    /// exit continues from wherever the entrance got to. Close retracts
    /// *up*: the palette anchored under the top bar slides back under it.
    fn gdy(&self, now: Instant) -> f64 {
        match self.phase {
            Phase::Open(t0) => {
                let t = (now.duration_since(t0).as_secs_f64() / OPEN_SLIDE).min(1.0);
                -OPEN_DY * (1.0 - t).powi(3)
            }
            Phase::Close { t0, from } => {
                let t = (now.duration_since(t0).as_secs_f64() / CLOSE_SLIDE).min(1.0);
                from - t.powi(2) * CLOSE_DY // accelerate away — retracts, not slides
            }
            Phase::Live => 0.0,
        }
    }

    /// Restart the entrance from the moment the surface actually
    /// becomes visible — the state is built during backend setup, so
    /// without this the slide/cascade finishes before first frame.
    fn reopen(&mut self) {
        if matches!(self.phase, Phase::Close { .. }) {
            return;
        }
        let now = Instant::now();
        self.phase = Phase::Open(now);
        self.open_t = now;
    }

    fn begin_close(&mut self) {
        if matches!(self.phase, Phase::Close { .. }) {
            return;
        }
        let now = Instant::now();
        self.phase = Phase::Close {
            t0: now,
            from: self.gdy(now),
        };
    }

    fn escape(&mut self) {
        self.begin_close();
    }

    fn max_scroll(&self) -> f64 {
        max_scroll(self.cells.len())
    }

    fn pill_at(&self, x: f64, y: f64) -> Option<Pill> {
        let i = self.hover?;
        let (cx, cy) = cell_rect(self.scroll, i);
        pills(cx, cy)
            .iter()
            .find(|(_, px, py, pw)| x >= *px && x < px + pw && y >= *py && y < py + 20.0)
            .map(|(p, ..)| *p)
    }

    fn cell_at(&self, x: f64, y: f64) -> Option<usize> {
        if y < HEADER + 4.0 || x < PAD || x >= W - PAD {
            return None;
        }
        let (r, c) = (
            ((y + self.scroll - HEADER - PAD) / (CELL_H + GAP)) as i64,
            ((x - PAD) / (CELL_W + GAP)) as i64,
        );
        if r < 0 || c < 0 || c >= COLS as i64 {
            return None;
        }
        let (lx, ly) = (x - PAD - c as f64 * (CELL_W + GAP), y + self.scroll - HEADER - PAD - r as f64 * (CELL_H + GAP));
        if lx >= CELL_W || ly >= CELL_H {
            return None; // inside a gap
        }
        let i = (r * COLS as i64 + c) as usize;
        (i < self.cells.len()).then_some(i)
    }

    /// Cursor moved — returns true when any hover state changed.
    fn moved(&mut self, x: f64, y: f64) -> bool {
        self.cursor = (x, y);
        let mut dirty = false;
        let hc = dist_to((x, y), W - CLOSE_R - 10.0, CLOSE_R + 10.0) <= CLOSE_R + 4.0;
        if hc != self.hover_close {
            self.hover_close = hc;
            dirty = true;
        }
        let i = self.cell_at(x, y);
        if i != self.hover {
            self.hover = i;
            self.pill_hot = None;
            dirty = true;
        }
        let ph = self.pill_at(x, y);
        if ph != self.pill_hot {
            self.pill_hot = ph;
            dirty = true;
        }
        dirty
    }

    /// Pointer left the surface — unlight everything.
    fn pointer_left(&mut self) -> bool {
        let was = self.hover.take().is_some()
            || self.pill_hot.take().is_some()
            || self.hover_close;
        self.hover_close = false;
        was
    }

    fn wheel(&mut self, d: f64) {
        self.scroll_to = (self.scroll_to - d).clamp(0.0, self.max_scroll());
    }

    /// Primary click — resolves against the hover/cursor state the
    /// backend last fed through `moved`.
    fn clicked(&mut self) -> Hit {
        let (x, y) = self.cursor;
        if self.hover_close {
            self.begin_close();
            return Hit::Closed;
        }
        if let (Some(i), Some(p)) = (self.hover, self.pill_at(x, y)) {
            let path = self.cells[i].d.path.clone();
            return Hit::Event(match p {
                Pill::Copy => GalleryEvent::Copy(path),
                Pill::Pin => GalleryEvent::Pin(path),
                Pill::Del => {
                    // two-step delete — first click arms the red
                    // pill, second within ARM_TTL deletes for real
                    if self.cells[i].armed.is_some() {
                        self.cells[i].dying = Some(Instant::now());
                        GalleryEvent::Delete(path)
                    } else {
                        self.cells[i].armed = Some(Instant::now());
                        return Hit::Miss;
                    }
                }
            });
        }
        if let Some(i) = self.hover {
            if let Some(c) = self.cells.get_mut(i) {
                c.armed = None; // clicking elsewhere disarms
            }
            return Hit::Event(GalleryEvent::Open(self.cells[i].d.path.clone()));
        }
        if y < HEADER {
            return Hit::Header;
        }
        Hit::Miss
    }

    /// Advance every animation — Busy means another frame is needed.
    fn tick(&mut self, now: Instant) -> Tick {
        match self.phase {
            Phase::Open(t0) => {
                if now.duration_since(t0).as_secs_f64() >= OPEN_SLIDE {
                    self.phase = Phase::Live;
                }
            }
            Phase::Close { t0, .. } => {
                if now.duration_since(t0).as_secs_f64() >= CLOSE_SLIDE {
                    return Tick::Dead;
                }
            }
            Phase::Live => {}
        }
        let mut busy = matches!(self.phase, Phase::Open(_) | Phase::Close { .. });
        // entrance cascade stays "busy" until the last cell lands
        if !self.cells.is_empty()
            && now
                .duration_since(self.open_t)
                .as_secs_f64()
                < (self.cells.len() - 1) as f64 * STAGGER + ENTER
        {
            busy = true;
        }
        // scroll easing + per-cell hover/dying state
        let sd = self.scroll_to - self.scroll;
        if sd.abs() > 0.5 {
            self.scroll += sd * 0.25;
            busy = true;
        } else if self.scroll != self.scroll_to {
            self.scroll = self.scroll_to;
            busy = true;
        }
        let mut any_dead = false;
        for i in 0..self.cells.len() {
            let c = &mut self.cells[i];
            let target = if self.hover == Some(i) { 1.0f32 } else { 0.0 };
            if (c.hot - target).abs() > 0.02 {
                c.hot += (target - c.hot) * 0.35;
                busy = true;
            } else if c.hot != target {
                c.hot = target;
            }
            if let Some(t) = c.armed {
                if now.duration_since(t).as_secs_f64() > ARM_TTL {
                    c.armed = None;
                    busy = true;
                }
            }
            if let Some(t) = c.dying {
                if now.duration_since(t).as_secs_f64() >= 0.18 {
                    any_dead = true;
                } else {
                    busy = true;
                }
            }
        }
        if any_dead {
            self.cells
                .retain(|c| c.dying.is_none_or(|t| now.duration_since(t).as_secs_f64() < 0.18));
            // indexes shifted — drop stale hover state, draw the gap
            self.hover = None;
            self.pill_hot = None;
            busy = true;
        }
        if busy {
            Tick::Busy
        } else {
            Tick::Idle
        }
    }
}

impl Grid<'_> {
    /// Full window paint — chrome, header, cells, scrollbar.
    fn render(&self, cv: &mut Canvas, t: &Txt, now: Instant, gdy: f64) {
        card_chrome(cv, W, H, 14.0, BG, EDGE, FRINGE);
        // header — title left, count chip, × right
        t.draw(cv, PAD, 34.0, 15.0, "Recent Captures", FG);
        let n = self.cells.len().to_string();
        let nw = t.width(10.5, &n) + 14.0;
        let title_w = t.width(15.0, "Recent Captures");
        pill(cv, (PAD + title_w + 8.0, 19.0, nw, 17.0), 8.0, CHIP, EDGE, 1.0);
        t.draw(cv, PAD + title_w + 8.0 + nw / 2.0 - t.width(10.5, &n) / 2.0, 31.5, 10.5, &n, DIM);
        self.draw_close(cv);
        // grid — clipped to below the header via per-pixel check
        if self.cells.is_empty() {
            t.draw(cv, PAD, H / 2.0 - 20.0, 13.0, "No captures yet", DIM);
            t.draw(cv, PAD, H / 2.0, 10.5, "Ctrl+Alt+Q to grab a region", EMPTY);
        }
        for i in 0..self.cells.len() {
            self.draw_cell(cv, t, i, now, gdy);
        }
        // scrollbar hint — a thin notch on the right edge while scrollable
        let ms = max_scroll(self.cells.len());
        if ms > 0.0 {
            let scale = cv.scale;
            let track = H - HEADER - 12.0;
            let thumb = (track * (H - HEADER) / content_h(self.cells.len())).max(24.0);
            let sy = HEADER + 6.0 + (track - thumb) * self.scroll / ms;
            for py in (sy * scale) as i32..((sy + thumb) * scale) as i32 {
                for px in ((W - 4.0) * scale) as i32..((W - 1.5) * scale) as i32 {
                    let d = rrect_sdf(px as f64 / scale, py as f64 / scale, W - 2.75, sy + thumb / 2.0, 1.2, thumb / 2.0, 1.2);
                    let a = fill_cov(d, scale);
                    if a > 0.0 {
                        cv.blend(px, py, DIM, a * 0.5);
                    }
                }
            }
        }
    }

    fn draw_cell(&self, cv: &mut Canvas, t: &Txt, i: usize, now: Instant, gdy: f64) {
        let (cx, cy0) = cell_rect(self.scroll, i);
        let cy = cy0 + gdy;
        // fully outside the viewport — header covers the top
        if cy + CELL_H < HEADER || cy > H {
            return;
        }
        let c = &self.cells[i];
        // entrance: each cell rises ENTER_DY while fading in, staggered.
        // back-out easing overshoots ~8% — cells float up past their
        // slot then settle, a little spring instead of a dead stop
        let st = (now.duration_since(self.open_t).as_secs_f64() - i as f64 * STAGGER) / ENTER;
        let u = st.clamp(0.0, 1.0);
        let e = if self.closing {
            1.0
        } else {
            1.0 + 2.2 * (u - 1.0).powi(3) + 1.2 * (u - 1.0).powi(2)
        };
        // hover lift — the hot cell floats a few px above the grid
        let (dy, mut a) = ((1.0 - e) * ENTER_DY - c.hot as f64 * 4.0, e.min(1.0));
        // delete-out: shrink to a point while fading
        let mut zoom = 1.0 + 0.05 * c.hot as f64;
        if let Some(t0) = c.dying {
            let k = (now.duration_since(t0).as_secs_f64() / 0.18).min(1.0);
            zoom *= 1.0 - 0.15 * k;
            a *= 1.0 - k;
        }
        if a <= 0.0 {
            return;
        }
        // thumb blit into an rrect clip, centred in the cell's thumb box
        let (tw, th) = (c.d.tw as f64, c.d.th as f64);
        let (dw, dh) = (tw * zoom, th * zoom);
        let (dx0, dy0) = (cx + (CELL_W - dw) / 2.0, cy + dy + (THUMB_H - dh) / 2.0);
        let scale = cv.scale;
        for py in (dy0 * scale) as i32..((dy0 + dh) * scale) as i32 {
            for px in (dx0 * scale) as i32..((dx0 + dw) * scale) as i32 {
                let (lx, ly) = (px as f64 / scale, py as f64 / scale);
                // clip to the *unzoomed* thumb box — the zoom samples a
                // tighter window of the image instead of growing it
                let d = rrect_sdf(lx, ly, cx + CELL_W / 2.0, cy + dy + THUMB_H / 2.0, CELL_W / 2.0 - 2.0, THUMB_H / 2.0 - 2.0, 8.0);
                if d > 0.0 || ly < HEADER + 2.0 {
                    continue;
                }
                let (sx, sy) = (
                    (((lx - dx0) / zoom) as usize).min(c.d.tw - 1),
                    (((ly - dy0) / zoom) as usize).min(c.d.th - 1),
                );
                cv.blend(px, py, c.d.px[sy * c.d.tw + sx], a);
            }
        }
        // hover ring + pills, both eased by c.hot
        let h = c.hot as f64;
        if h > 0.02 {
            for py in (cy * scale) as i32..((cy + THUMB_H) * scale) as i32 {
                for px in (cx * scale) as i32..((cx + CELL_W) * scale) as i32 {
                    let (lx, ly) = (px as f64 / scale, py as f64 / scale);
                    let d = rrect_sdf(lx, ly, cx + CELL_W / 2.0, cy + dy + THUMB_H / 2.0, CELL_W / 2.0 - 2.0, THUMB_H / 2.0 - 2.0, 8.0);
                    let rim = stroke_cov(d, 1.4, scale) * h;
                    if rim > 0.0 && ly >= HEADER + 2.0 {
                        cv.blend(px, py, self.accent, rim);
                    }
                }
            }
            for (p, px0, py0, pw) in pills(cx, cy + dy) {
                let armed = matches!(p, Pill::Del) && c.armed.is_some();
                let (bg, fg) = if armed {
                    (DANGER, 0xFFFFFF)
                } else if self.pill_hot == Some(p) {
                    (HOT, FG)
                } else {
                    (CHIP, FG)
                };
                let label = match (p, armed) {
                    (Pill::Copy, _) => "Copy",
                    (Pill::Pin, _) => "Pin",
                    (Pill::Del, false) => "Del",
                    (Pill::Del, true) => "Sure?",
                };
                let pw2 = if armed { 44.0 } else { pw };
                pill(cv, (px0, py0, pw2, 20.0), 6.0, bg, EDGE, 0.92 * h);
                let tw2 = t.width(9.5, label);
                t.draw(cv, px0 + (pw2 - tw2) / 2.0, py0 + 14.0, 9.5, label, fg);
            }
        }
        // captions under the thumb
        if e > 0.55 {
            let ty = cy + dy + THUMB_H + 8.0;
            t.draw(cv, cx + 2.0, ty + 8.0, 10.5, &c.d.age, FG);
            t.draw(cv, cx + 2.0, ty + 22.0, 9.5, &c.d.dims, DIM);
            // filename, right-aligned — the stem is what differentiates
            let name = c.d.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let stem = name.trim_end_matches(".png");
            let tail = stem.chars().rev().take(10).collect::<String>().chars().rev().collect::<String>();
            t.draw(cv, cx + CELL_W - t.width(9.5, &tail) - 2.0, ty + 22.0, 9.5, &tail, EMPTY);
        }
    }

    fn draw_close(&self, cv: &mut Canvas) {
        let (cx, cy) = (W - CLOSE_R - 10.0, CLOSE_R + 10.0);
        let scale = cv.scale;
        for py in ((cy - CLOSE_R) * scale) as i32..((cy + CLOSE_R) * scale) as i32 {
            for px in ((cx - CLOSE_R) * scale) as i32..((cx + CLOSE_R) * scale) as i32 {
                let d = ellipse_sdf(px as f64 / scale, py as f64 / scale, cx, cy, CLOSE_R, CLOSE_R);
                let a = fill_cov(d, scale);
                if a > 0.0 {
                    cv.blend(px, py, if self.hover_close { X_BG } else { CHIP }, a);
                }
            }
        }
        for py in ((cy - 4.0) * scale) as i32..((cy + 4.0) * scale) as i32 {
            for px in ((cx - 4.0) * scale) as i32..((cx + 4.0) * scale) as i32 {
                let d = seg_sdf(px as f64 / scale, py as f64 / scale, cx - 3.0, cy - 3.0, cx + 3.0, cy + 3.0)
                    .min(seg_sdf(px as f64 / scale, py as f64 / scale, cx - 3.0, cy + 3.0, cx + 3.0, cy - 3.0));
                let a = stroke_cov(d, 1.3, scale);
                if a > 0.0 {
                    cv.blend(px, py, FG, a);
                }
            }
        }
    }
}

fn dist_to(p: (f64, f64), x: f64, y: f64) -> f64 {
    ((p.0 - x).powi(2) + (p.1 - y).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::system_font;

    fn demo_cell(i: usize, w: usize, h: usize, age: &str) -> Cell {
        let px = (0..w * h)
            .map(|p| {
                let (x, y) = (p % w, p / w);
                ((x * 255 / w.max(1)) as u32) << 16
                    | ((y * 255 / h.max(1)) as u32) << 8
                    | (0x40 + i as u32 * 30)
            })
            .collect();
        Cell {
            d: CellData {
                path: format!("/tmp/quickshort-20250101-1200{i:02}.png").into(),
                px,
                tw: w,
                th: h,
                dims: format!("{}×{}", w * 10, h * 10),
                age: age.into(),
            },
            hot: 0.0,
            armed: None,
            dying: None,
        }
    }

    #[test]
    fn grid_math() {
        assert_eq!(max_scroll(0), 0.0);
        assert_eq!(max_scroll(3), 0.0); // a single row always fits
        assert!(max_scroll(9) > 0.0);
        let (x, y) = cell_rect(0.0, 4); // row 1, col 1
        assert_eq!(x, PAD + CELL_W + GAP);
        assert_eq!(y, HEADER + PAD + CELL_H + GAP);
        // scroll shifts cells up, nothing else
        let (_, y2) = cell_rect(30.0, 4);
        assert_eq!(y2, y - 30.0);
    }

    /// `cargo test -p qs-app gallery_png -- --ignored` renders the card
    /// to /tmp for eyeballing — same pattern as card_png.
    #[test]
    #[ignore]
    fn gallery_png() {
        let mut cells: Vec<Cell> = vec![
            demo_cell(0, 200, 110, "just now"),
            demo_cell(1, 180, 132, "5m ago"),
            demo_cell(2, 200, 90, "1h ago"),
            demo_cell(3, 150, 132, "3h ago"),
            demo_cell(4, 200, 132, "2d ago"),
        ];
        cells[1].hot = 1.0; // hover state → ring + action pills
        let g = Grid {
            cells: &cells,
            scroll: 0.0,
            // entrance finished — deterministic frame
            open_t: Instant::now() - Duration::from_secs(2),
            accent: 0x1677DF,
            pill_hot: Some(Pill::Copy),
            hover_close: false,
            closing: false,
        };
        let (w, h) = (W as usize, H as usize);
        let mut buf = vec![0u32; w * h];
        let font = system_font().unwrap();
        {
            let mut cv = Canvas {
                buf: &mut buf,
                w,
                h,
                scale: 1.0,
            };
            let t = Txt {
                font: &font,
                scale: 1.0,
            };
            g.render(&mut cv, &t, Instant::now(), 0.0);
        }
        let mut out = image::RgbaImage::new(w as u32, h as u32);
        for (i, px) in buf.iter().enumerate() {
            out.put_pixel(
                (i % w) as u32,
                (i / w) as u32,
                image::Rgba([(px >> 16) as u8, (px >> 8) as u8, *px as u8, 255]),
            );
        }
        out.save("/tmp/quickshort-gallery.png").unwrap();
    }
}
