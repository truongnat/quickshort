//! Chrome layer — the post-capture album stack, fly-to-corner animation
//! and lightbox preview, drawn into one per-pixel-alpha surface per
//! monitor (see `chrome/macos.rs` for the AppKit presentation; other
//! platforms get a stub that declines `land` so the host falls back to
//! the quick-access card).
//!
//! Buffers are premultiplied `A<<24 | B<<16 | G<<8 | R` u32s — in memory
//! that is R,G,B,A bytes, what NSBitmapImageRep and wl_shm ARGB8888 take.
//! All drawing goes through `Canvas::blend_over`.

use image::RgbaImage;
use qs_overlay::sdf::{ellipse_sdf, fill_cov, rrect_sdf, stroke_cov, Canvas};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
#[path = "chrome/macos.rs"]
mod imp;
#[cfg(not(target_os = "macos"))]
#[path = "chrome/stub.rs"]
mod imp;

pub use imp::ChromeWin;

use crate::widgets::Txt;

/* ---- layout constants (logical px) ---- */
const TW: f64 = 118.0;
const TH: f64 = 74.0;
const MARGIN: f64 = 20.0;
const STACK_DX: f64 = 6.0; // depth offset per older shot
const STACK_DY: f64 = 8.0;
const FAN: f64 = 128.0; // hover/pinned fan spacing
const MAX_VIS: usize = 5;
const MAX_VIS_PINNED: usize = 8;
const FLY_MS: f64 = 420.0;
const FAN_OPEN_MS: f64 = 280.0;
const FAN_CLOSE_MS: f64 = 180.0;
const BOX_MS: f64 = 200.0;
/// idle before the stack slides into the edge
const HIDE_AFTER_MS: f64 = 3000.0;
const HIDE_MS: f64 = 280.0;   // slide-away
const UNHIDE_MS: f64 = 200.0; // slide-back
// thumbs slide fully off-screen when hidden — the edge-docked chip
// is the grab handle (a sliver + chip read as two disjoint artifacts)

/* ---- ui-2026 tokens ---- */
const CARD: u32 = 0x0A0A0A;
const LINE: u32 = 0x262626;
const MUTED: u32 = 0xA1A1A1;
const GRN: u32 = 0x30D158;
const RED: u32 = 0xFF5F57;

/// What produced the shot — drives the corner badge.
#[derive(Clone, Copy, PartialEq)]
pub enum ShotKind {
    Saved,
    Copied,
    Ocr,
    Shared,
    Scroll,
    Record,
}

pub struct Shot {
    /// full-resolution capture, kept for Copy/Pin/Reveal actions
    pub img: Rc<RgbaImage>,
    /// premul-packed thumbnail pixels (physical px)
    thumb: Vec<u32>,
    /// thumbnail buffer dims in physical px
    tw: usize,
    th: usize,
    /// on disk already → Reveal; None → unsaved (Copy-only provenance)
    pub path: Option<PathBuf>,
    kind: ShotKind,
}

#[derive(Clone, Copy)]
pub struct FRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

struct Fly {
    /// premul-packed full-res pixels — packed once at land-time,
    /// blitted scaled each frame (packing per frame was the jank)
    packed: Vec<u32>,
    iw: usize,
    ih: usize,
    from: FRect,
    t0: Instant,
}

/// Hit-test roles; `rects` maps these to screen areas each frame.
#[derive(Clone, Copy, PartialEq)]
pub enum Hit {
    /// stack thumbnail `i` (0 = newest)
    Thumb(usize),
    /// the × button on thumb `i`
    Del(usize),
    /// "N SHOTS" count chip — toggles the pinned fan
    Chip,
    /// lightbox backdrop (click = close)
    Backdrop,
}

/// What the host must run in response to a chrome click/key.
pub enum ChromeCmd {
    /// state changed — repaint the window
    Redraw,
    /// right-click menu over thumb `idx` at logical (x, y)
    Menu { idx: usize, x: f64, y: f64 },
    Remove(usize),
}

/// muda ids for the thumb right-click menu (host matches on them).
pub const MENU_REVEAL: &str = "qs-chrome.reveal";
pub const MENU_COPY: &str = "qs-chrome.copy";
pub const MENU_SAVE: &str = "qs-chrome.save";
pub const MENU_PIN: &str = "qs-chrome.pin";
pub const MENU_DEL: &str = "qs-chrome.del";

/// Which screen corner the stack docks to.
#[derive(Clone, Copy, PartialEq)]
pub enum Corner {
    TL,
    TC,
    TR,
    BL,
    BC,
    BR,
}

impl Corner {
    pub fn from_popup_pos(pos: &str) -> Self {
        match pos {
            "top-left" => Self::TL,
            "top-center" => Self::TC,
            "bottom-left" => Self::BL,
            "bottom-center" => Self::BC,
            "bottom-right" => Self::BR,
            _ => Self::TR,
        }
    }
    fn right(self) -> bool {
        matches!(self, Self::TR | Self::BR)
    }
    fn bottom(self) -> bool {
        matches!(self, Self::BL | Self::BC | Self::BR)
    }
}

/// Per-window stack state — pure logic, platform code only forwards
/// events and asks `render` for pixels.
pub struct Stack {
    pub shots: Vec<Shot>, // newest LAST
    fly: Option<Fly>,
    hover: Option<Hit>,
    /// stacked↔fanned blend — time-based tween toward `fan_to`
    fan_t: f64,
    fan_from: f64,
    fan_to: f64,
    fan_t0: Instant,
    fan_anim: bool,
    /// shown↔edge-hidden blend — after idle the stack slides into the
    /// screen edge — the docked chip is the only visible handle
    hide_t: f64,
    hide_from: f64,
    hide_to: f64,
    hide_t0: Instant,
    hide_anim: bool,
    /// last user activity — the idle countdown for edge-hiding
    idle_since: Instant,
    /// keyboard selection ring (macshot model — hover syncs it)
    ring: Option<usize>,
    pub pinned: bool,
    lightbox: Option<usize>,
    /// lightbox fade/scale-in start + its packed pixels (cached at open)
    box_t0: Option<Instant>,
    box_img: Vec<u32>,
    box_iw: usize,
    box_ih: usize,
    corner: Corner,
}

impl Stack {
    pub fn new(corner: Corner) -> Self {
        Self {
            shots: Vec::new(),
            fly: None,
            hover: None,
            fan_t: 0.0,
            fan_from: 0.0,
            fan_to: 0.0,
            fan_t0: Instant::now(),
            fan_anim: false,
            hide_t: 0.0,
            hide_from: 0.0,
            hide_to: 0.0,
            hide_t0: Instant::now(),
            hide_anim: false,
            idle_since: Instant::now(),
            ring: None,
            pinned: false,
            lightbox: None,
            box_t0: None,
            box_img: Vec::new(),
            box_iw: 0,
            box_ih: 0,
            corner,
        }
    }

    /// Queue the fly-in; `from` is the selection rect in window-local
    /// logical coords. `rm` (reduce motion) lands instantly.
    pub fn land(&mut self, img: RgbaImage, kind: ShotKind, path: Option<PathBuf>, from: FRect, rm: bool, scale: f64) {
        self.touch(); // a landing shot is activity — reveal the stack
        let img = Rc::new(img);
        let (thumb, tw, th) = make_thumb(&img, kind, scale);
        let shot = Shot {
            img: img.clone(),
            thumb,
            tw,
            th,
            path,
            kind,
        };
        if rm {
            self.shots.push(shot);
        } else {
            self.fly = Some(Fly {
                iw: img.width() as usize,
                ih: img.height() as usize,
                packed: pack_image(&img),
                from,
                t0: Instant::now(),
            });
            self.shots.push(shot); // reserve the slot — drawn at the landing point
        }
    }

    pub fn is_empty(&self) -> bool {
        self.shots.is_empty() && self.fly.is_none()
    }

    /// The anchor point of the newest thumb (fan index 0).
    fn anchor(&self, w: f64, h: f64) -> (f64, f64) {
        let x = if self.corner.right() {
            w - MARGIN - TW
        } else if self.corner == Corner::TC || self.corner == Corner::BC {
            (w - TW) / 2.0
        } else {
            MARGIN
        };
        let y = if self.corner.bottom() { h - MARGIN - TH } else { MARGIN };
        // (the chrome view already starts below the menu bar — macOS
        // clamps the window to visibleFrame — so no extra top inset)
        (x, y)
    }

    /// Rect of the thumb `i` positions behind the newest (0 = newest).
    /// Hover/pinned fans them horizontally toward the screen middle.
    fn thumb_rect(&self, i: usize, w: f64, h: f64) -> FRect {
        let (ax, ay) = self.anchor(w, h);
        let inward = if self.corner.right() { -1.0 } else { 1.0 };
        // hiding collapses every offset so the edge shows ONE sliver
        // (the newest thumb) instead of a diagonal trail of peeks
        let collapse = 1.0 - self.hide_t;
        let dx = inward * (STACK_DX + (FAN - STACK_DX) * self.fan_t) * i as f64 * collapse;
        // edge-hidden slides thumbs fully off-screen — the docked chip
        // is the only visible handle (a sliver + floating chip read as
        // two disjoint artifacts)
        let hide_dx = -inward * self.hide_t * (TW + MARGIN);
        // the fan is a straight horizontal row — the cascade's vertical
        // offset collapses as fan_t opens (was spreading diagonally)
        let dy = if self.corner.bottom() { -STACK_DY } else { STACK_DY }
            * i as f64 * (1.0 - self.fan_t) * collapse;
        FRect {
            x: ax + dx + hide_dx,
            y: ay + dy,
            w: TW,
            h: TH,
        }
    }

    fn chip_rect(&self, w: f64, h: f64) -> FRect {
        let (ax, ay) = self.anchor(w, h);
        let (cw, ch) = (64.0, 20.0);
        // beside the newest thumb, aligned to its far edge — never
        // clipped by the screen edge regardless of the dock corner
        let shown = if self.corner.right() { ax - cw - 8.0 } else { ax + TW + 8.0 };
        // hidden: dock flush against the screen edge as the sole handle
        let docked = if self.corner.right() { w - cw - 2.0 } else { 2.0 };
        let x = shown + (docked - shown) * self.hide_t;
        let y = if self.corner.bottom() {
            ay + TH - ch
        } else {
            ay
        };
        FRect { x, y, w: cw, h: ch }
    }

    /// Interactive regions in logical coords for the platform's
    /// hit-test/input-region. Order matters: first match wins. Thumb
    /// rects are padded so the × disc overhanging the edge stays live.
    pub fn input_rects(&self, w: f64, h: f64) -> Vec<(FRect, Hit)> {
        if self.lightbox.is_some() {
            return vec![(FRect { x: 0.0, y: 0.0, w, h }, Hit::Backdrop)];
        }
        let mut v = Vec::new();
        let n = self.shots.len().min(if self.pinned { MAX_VIS_PINNED } else { MAX_VIS });
        for i in 0..n {
            let mut r = self.thumb_rect(i, w, h);
            r.x -= 12.0;
            r.y -= 12.0;
            r.w += 24.0;
            r.h += 24.0;
            v.push((r, Hit::Thumb(i)));
        }
        if self.shots.len() >= 2 {
            v.push((self.chip_rect(w, h), Hit::Chip));
        }
        v
    }

    /// Point (logical) → role.
    pub fn hit(&self, x: f64, y: f64, w: f64, h: f64) -> Option<Hit> {
        if self.lightbox.is_none() {
            // the × disc only exists while its thumb is hovered
            if let Some(Hit::Thumb(i) | Hit::Del(i)) = self.hover {
                let r = self.thumb_rect(i, w, h);
                let (dx, dy) = self.del_pos(r);
                if (x - dx).powi(2) + (y - dy).powi(2) <= 11.0 * 11.0 {
                    return Some(Hit::Del(i));
                }
            }
        }
        for (r, hit) in self.input_rects(w, h) {
            if x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h {
                return Some(hit);
            }
        }
        None
    }

    fn del_pos(&self, r: FRect) -> (f64, f64) {
        if self.corner.bottom() {
            (r.x + r.w - 4.0, r.y - 4.0)
        } else {
            (r.x + r.w - 4.0, r.y + r.h + 4.0)
        }
    }

    /// Platform events distilled into state transitions; the returned
    /// command is for the host (menu, remove).
    pub fn pointer(&mut self, ev: ChromeEv, w: f64, h: f64) -> Option<ChromeCmd> {
        match ev {
            ChromeEv::Move { x, y } => {
                let hit = self.hit(x, y, w, h);
                if hit != self.hover {
                    // real hover transitions are activity — synthetic
                    // Move re-emits (sync_input, global monitor) aren't
                    self.touch();
                    self.hover = hit;
                    self.retarget_fan();
                    // hover drives the keyboard ring (macshot parity)
                    self.ring = match hit {
                        Some(Hit::Thumb(i)) | Some(Hit::Del(i)) => Some(i),
                        _ => self.ring.filter(|_| hit.is_some()),
                    };
                    return Some(ChromeCmd::Redraw);
                }
                None
            }
            ChromeEv::Leave => {
                if self.hover.take().is_some() {
                    self.retarget_fan();
                    return Some(ChromeCmd::Redraw);
                }
                None
            }
            ChromeEv::Down { x, y } => {
                self.touch();
                match self.hit(x, y, w, h) {
                Some(Hit::Del(i)) => Some(ChromeCmd::Remove(i)),
                Some(Hit::Chip) => {
                    self.pinned = !self.pinned;
                    self.retarget_fan();
                    Some(ChromeCmd::Redraw)
                }
                Some(Hit::Thumb(i)) => {
                    self.open_lightbox(i);
                    Some(ChromeCmd::Redraw)
                }
                Some(Hit::Backdrop) => {
                    self.lightbox = None;
                    Some(ChromeCmd::Redraw)
                }
                None => None,
                }
            }
            ChromeEv::RightDown { x, y } => {
                self.touch();
                match self.hit(x, y, w, h) {
                    Some(Hit::Thumb(i)) | Some(Hit::Del(i)) => Some(ChromeCmd::Menu { idx: i, x, y }),
                    _ => None,
                }
            }
            ChromeEv::Key(_) => None, // keys go through `key()`
        }
    }

    /// Keyboard input while the chrome window is key — arrows move the
    /// ring, Enter previews, ⌫ removes, Esc peels a layer off.
    pub fn key(&mut self, k: Key) -> Option<ChromeCmd> {
        self.touch();
        let n = self
            .shots
            .len()
            .min(if self.pinned { MAX_VIS_PINNED } else { MAX_VIS });
        match (k, self.lightbox) {
            (Key::Esc, Some(_)) => {
                self.lightbox = None;
                Some(ChromeCmd::Redraw)
            }
            (Key::Esc, None) if self.pinned => {
                self.pinned = false;
                self.retarget_fan();
                Some(ChromeCmd::Redraw)
            }
            (Key::Esc, None) => None,
            (Key::Left, None) | (Key::Right, None) if n > 0 => {
                let d = if matches!(k, Key::Left) { 1 } else { -1 };
                let cur = self.ring.unwrap_or(if d > 0 { n } else { 0 });
                self.ring = Some((cur as i64 + d).clamp(0, n as i64 - 1) as usize);
                Some(ChromeCmd::Redraw)
            }
            (Key::Enter, None) => match self.ring {
                Some(i) if i < n => {
                    self.open_lightbox(i);
                    Some(ChromeCmd::Redraw)
                }
                _ => None,
            },
            (Key::Back, _) => match self.ring.or(self.hover.and_then(|h| match h {
                Hit::Thumb(i) | Hit::Del(i) => Some(i),
                _ => None,
            })) {
                Some(i) => Some(ChromeCmd::Remove(i)),
                None => None,
            },
            _ => None,
        }
    }

    /// Retarget the fan tween whenever hover/pinned flips — opening
    /// overshoots slightly (ease-out-back, ~280ms) like a native
    /// popover; closing is a quicker plain ease-out (~180ms).
    fn retarget_fan(&mut self) {
        let t = if self.pinned || self.hover.is_some() { 1.0 } else { 0.0 };
        if t != self.fan_to {
            self.fan_from = self.fan_t;
            self.fan_to = t;
            self.fan_t0 = Instant::now();
            self.fan_anim = true;
        }
    }

    /// Any user activity resets the idle clock and pulls a hidden
    /// stack back out of the edge.
    fn touch(&mut self) {
        self.idle_since = Instant::now();
        if self.hide_to != 0.0 {
            self.hide_from = self.hide_t;
            self.hide_to = 0.0;
            self.hide_t0 = self.idle_since;
            self.hide_anim = true;
        }
    }

    /// Next wake for the idle-hide countdown — the host's pump sleeps
    /// until this when nothing is animating. None while hidden.
    pub fn idle_deadline(&self) -> Option<Instant> {
        if self.hide_to == 0.0
            && self.hover.is_none()
            && !self.pinned
            && self.lightbox.is_none()
            && !self.shots.is_empty()
        {
            Some(self.idle_since + Duration::from_millis(HIDE_AFTER_MS as u64))
        } else {
            None
        }
    }

    /// Fly + fan + lightbox + hide progress → true while frames are needed.
    pub fn animating(&self) -> bool {
        self.fly.is_some() || self.fan_anim || self.hide_anim || self.box_t0.is_some()
    }

    /// Open the lightbox on thumb `i` with a fade/scale-in — packs the
    /// shot's pixels once so frames only blit.
    fn open_lightbox(&mut self, i: usize) {
        if let Some(s) = self.shot(i) {
            let (p, w, h) = (pack_image(&s.img), s.img.width() as usize, s.img.height() as usize);
            self.box_img = p;
            self.box_iw = w;
            self.box_ih = h;
        }
        self.lightbox = Some(i);
        self.box_t0 = Some(Instant::now());
    }

    fn fly_t(&self, now: Instant) -> f64 {
        self.fly
            .as_ref()
            .map(|f| (now.duration_since(f.t0).as_secs_f64() * 1000.0 / FLY_MS).min(1.0))
            .unwrap_or(1.0)
    }

    /// Advance animations one tick — drop finished flights, run the
    /// fan tween and the lightbox fade-in to their ends.
    pub fn settle(&mut self, now: Instant) {
        if self.fly_t(now) >= 1.0 {
            self.fly = None;
        }
        if self.fan_anim {
            let ms = if self.fan_to > self.fan_from { FAN_OPEN_MS } else { FAN_CLOSE_MS };
            let p = (now.duration_since(self.fan_t0).as_secs_f64() * 1000.0 / ms).min(1.0);
            let e = if self.fan_to > self.fan_from { ease_out_back(p) } else { ease_out(p) };
            self.fan_t = self.fan_from + (self.fan_to - self.fan_from) * e;
            if p >= 1.0 {
                self.fan_t = self.fan_to;
                self.fan_anim = false;
            }
        }
        // idle → slide into the edge (a fresh shot resets the clock at land)
        if self.hide_to == 0.0
            && self.hide_t == 0.0
            && self.hover.is_none()
            && !self.pinned
            && self.lightbox.is_none()
            && self.fly.is_none()
            && now.duration_since(self.idle_since).as_secs_f64() * 1000.0 > HIDE_AFTER_MS
        {
            self.hide_from = self.hide_t;
            self.hide_to = 1.0;
            self.hide_t0 = now;
            self.hide_anim = true;
        }
        if self.hide_anim {
            let ms = if self.hide_to > self.hide_from { HIDE_MS } else { UNHIDE_MS };
            let p = (now.duration_since(self.hide_t0).as_secs_f64() * 1000.0 / ms).min(1.0);
            let e = if self.hide_to > self.hide_from { ease_out(p) } else { ease_out_back(p) };
            self.hide_t = self.hide_from + (self.hide_to - self.hide_from) * e;
            if p >= 1.0 {
                self.hide_t = self.hide_to;
                self.hide_anim = false;
            }
        }
        if let Some(t0) = self.box_t0 {
            if now.duration_since(t0).as_secs_f64() * 1000.0 >= BOX_MS {
                self.box_t0 = None;
            }
        }
    }

    /// Delete thumb `i` (0 = newest).
    pub fn remove(&mut self, i: usize) {
        let idx = self.shots.len() - 1 - i;
        if idx < self.shots.len() {
            self.shots.remove(idx);
        }
        if self.lightbox == Some(i) {
            self.lightbox = None;
        }
        self.hover = None;
        self.retarget_fan();
    }

    /// shot for hit-index i (0 = newest).
    pub fn shot(&self, i: usize) -> Option<&Shot> {
        self.shots.get(self.shots.len().checked_sub(1 + i)?)
    }

    pub fn shot_mut(&mut self, i: usize) -> Option<&mut Shot> {
        let idx = self.shots.len().checked_sub(1 + i)?;
        self.shots.get_mut(idx)
    }
}

/// Pointer/key events the platform view forwards.
#[allow(dead_code)] // Leave is part of the portable contract (Wayland wl_pointer.leave)
pub enum ChromeEv {
    Move { x: f64, y: f64 },
    Leave,
    Down { x: f64, y: f64 },
    RightDown { x: f64, y: f64 },
    Key(Key),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Key {
    Esc,
    Left,
    Right,
    Enter,
    Back,
}

/* ==================== rendering ==================== */

fn ease_out(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

/// easeOutBack — subtle overshoot (≈8%) so the fan "settles" rather
/// than stops dead, the way a native popover does.
fn ease_out_back(t: f64) -> f64 {
    let c = 1.70158 * 0.8;
    let u = t - 1.0;
    1.0 + (c + 1.0) * u * u * u + c * u * u
}

/// Source-over a packed premul pixel `A<<24|B<<16|G<<8|R` at coverage
/// `cov`. Scaling the premult channels by `cov` keeps color and alpha
/// consistent.
#[inline]
fn blit_px(cv: &mut Canvas, x: i32, y: i32, px: u32, cov: f64) {
    if cov <= 0.0 {
        return;
    }
    let i = y as usize * cv.w + x as usize;
    let sa = (px >> 24) as f64 / 255.0 * cov;
    let ia = 1.0 - sa;
    let d = cv.buf[i];
    let oa = sa * 255.0 + (d >> 24) as f64 * ia;
    let r = (px & 0xFF) as f64 * cov + (d & 0xFF) as f64 * ia;
    let g = ((px >> 8) & 0xFF) as f64 * cov + ((d >> 8) & 0xFF) as f64 * ia;
    let b = ((px >> 16) & 0xFF) as f64 * cov + ((d >> 16) & 0xFF) as f64 * ia;
    cv.buf[i] = (oa.round() as u32) << 24
        | (b.round() as u32) << 16
        | (g.round() as u32) << 8
        | r.round() as u32;
}

/// Nearest-neighbour blit of packed-premul `src` (sw×sh) into the
/// logical rect `dst`, clipped to a rounded rect `rad` (logical).
fn blit_img(cv: &mut Canvas, src: &[u32], sw: usize, sh: usize, dst: FRect, rad: f64) {
    let s = cv.scale;
    for py in (dst.y * s) as i32..((dst.y + dst.h) * s).ceil() as i32 {
        for px in (dst.x * s) as i32..((dst.x + dst.w) * s).ceil() as i32 {
            let lx = px as f64 / s;
            let ly = py as f64 / s;
            if rad > 0.0 {
                let d = rrect_sdf(lx, ly, dst.x + dst.w / 2.0, dst.y + dst.h / 2.0, dst.w / 2.0, dst.h / 2.0, rad);
                if d > 0.0 {
                    continue;
                }
                let cov = fill_cov(d, s);
                let u = (((lx - dst.x) / dst.w) * sw as f64) as usize;
                let v = (((ly - dst.y) / dst.h) * sh as f64) as usize;
                blit_px(cv, px, py, src[v.min(sh - 1) * sw + u.min(sw - 1)], cov);
            } else {
                let u = (((lx - dst.x) / dst.w) * sw as f64) as usize;
                let v = (((ly - dst.y) / dst.h) * sh as f64) as usize;
                blit_px(cv, px, py, src[v.min(sh - 1) * sw + u.min(sw - 1)], 1.0);
            }
        }
    }
}

fn rrect(cv: &mut Canvas, r: FRect, rad: f64, fill: u32, fa: f64, border: Option<u32>) {
    let s = cv.scale;
    for py in (r.y * s) as i32..((r.y + r.h) * s) as i32 {
        for px in (r.x * s) as i32..((r.x + r.w) * s) as i32 {
            let d = rrect_sdf(px as f64 / s, py as f64 / s, r.x + r.w / 2.0, r.y + r.h / 2.0, r.w / 2.0, r.h / 2.0, rad);
            if d <= 0.0 {
                cv.blend_over(px, py, fill, fill_cov(d, s) * fa);
            }
            if let Some(b) = border {
                let rim = stroke_cov(d, 0.7, s);
                if rim > 0.0 {
                    cv.blend_over(px, py, b, rim);
                }
            }
        }
    }
}

fn disc(cv: &mut Canvas, cx: f64, cy: f64, r: f64, fill: u32, fa: f64) {
    let s = cv.scale;
    for py in ((cy - r) * s) as i32..((cy + r) * s) as i32 {
        for px in ((cx - r) * s) as i32..((cx + r) * s) as i32 {
            let d = ellipse_sdf(px as f64 / s, py as f64 / s, cx, cy, r, r);
            if d <= 0.0 {
                cv.blend_over(px, py, fill, fill_cov(d, s) * fa);
            }
        }
    }
}

impl Stack {
    /// Paint the whole stack into the window buffer (already zeroed).
    pub fn render(&self, cv: &mut Canvas, font: &fontdue::Font, accent: u32, now: Instant) {
        let (lw, lh) = (cv.w as f64 / cv.scale, cv.h as f64 / cv.scale);
        let t = Txt { font, scale: cv.scale };

        if let Some(i) = self.lightbox {
            // backdrop dim fades in; the shot scales up from 94% —
            // real alpha here is the chrome-layer payoff
            let bp = self
                .box_t0
                .map(|t0| (now.duration_since(t0).as_secs_f64() * 1000.0 / BOX_MS).min(1.0))
                .unwrap_or(1.0);
            // uniform dim over a zeroed buffer → direct fill, no blend
            let dim = ((0.55 * ease_out(bp) * 255.0).round() as u32) << 24;
            cv.buf.fill(dim);
            if self.shot(i).is_some() {
                let grow = 0.94 + 0.06 * ease_out_back(bp);
                let fit = (lw * 0.8 / self.box_iw as f64)
                    .min(lh * 0.8 / self.box_ih as f64);
                let dw = self.box_iw as f64 * fit * grow;
                let dh = self.box_ih as f64 * fit * grow;
                let r = FRect { x: (lw - dw) / 2.0, y: (lh - dh) / 2.0 - 10.0, w: dw, h: dh };
                blit_img(cv, &self.box_img, self.box_iw, self.box_ih, r, 8.0);
                let fr = FRect { x: r.x - 1.0, y: r.y - 1.0, w: r.w + 2.0, h: r.h + 2.0 };
                rrect(cv, fr, 9.0, CARD, 0.0, Some(LINE));
                if bp >= 1.0 {
                    t.draw_over(cv, r.x, r.y + r.h + 24.0, 11.0, "ESC / CLICK TO CLOSE", MUTED);
                }
            }
            return;
        }

        // count chip
        if self.shots.len() >= 2 {
            let cr = self.chip_rect(lw, lh);
            rrect(cv, cr, 10.0, CARD, 0.92, Some(LINE));
            let label = format!("{} SHOTS", self.shots.len());
            let tw = t.width(9.5, &label);
            t.draw_over(cv, cr.x + (cr.w - tw) / 2.0, cr.y + 14.0, 9.5, &label, MUTED);
        }

        // thumbs, oldest first so newest draws on top
        let n = self.shots.len().min(if self.pinned { MAX_VIS_PINNED } else { MAX_VIS });
        for i in (0..n).rev() {
            // hit index i counts from newest; draw order is reverse
            let idx = self.shots.len() - 1 - i; // storage idx
            let Some(shot) = self.shots.get(idx) else { continue };
            let in_flight = self.fly.is_some() && idx == self.shots.len() - 1;
            if in_flight {
                continue; // its pixels are mid-flight
            }
            let r = self.thumb_rect(i, lw, lh);
            blit_img(cv, &shot.thumb, shot.tw, shot.th, r, 6.0);
            let hot = self.hover == Some(Hit::Thumb(i))
                || self.hover == Some(Hit::Del(i))
                || self.ring == Some(i);
            rrect(cv, r, 6.0, CARD, 0.0, Some(if hot { accent } else { LINE }));

            // badge
            let (bg, glyph) = match shot.kind {
                ShotKind::Saved | ShotKind::Copied | ShotKind::Ocr | ShotKind::Scroll => (GRN, "✓"),
                ShotKind::Shared => (accent, "⧉"),
                ShotKind::Record => (accent, "▶"),
            };
            let (bx, by) = (r.x + 6.0, r.y + r.h - 6.0);
            disc(cv, bx, by, 8.0, bg, 0.95);
            t.draw_over(cv, bx - t.width(9.0, glyph) / 2.0, by + 3.4, 9.0, glyph, 0x000000);

            // × on hover
            if self.hover == Some(Hit::Thumb(i)) || self.hover == Some(Hit::Del(i)) {
                let (dx, dy) = self.del_pos(r);
                disc(cv, dx, dy, 8.0, CARD, 0.95);
                let del_hot = self.hover == Some(Hit::Del(i));
                t.draw_over(cv, dx - t.width(10.0, "×") / 2.0, dy + 3.6, 10.0, "×", if del_hot { RED } else { MUTED });
            }
        }

        // the flight itself
        if let Some(f) = &self.fly {
            let p = ease_out(self.fly_t(now));
            let to = self.thumb_rect(0, lw, lh);
            let r = FRect {
                x: f.from.x + (to.x - f.from.x) * p,
                y: f.from.y + (to.y - f.from.y) * p,
                w: f.from.w + (to.w - f.from.w) * p,
                h: f.from.h + (to.h - f.from.h) * p,
            };
            // draw the capture scaled into the moving rect — border fades
            // from accent to hairline as it "becomes" a stack thumb
            blit_img(cv, &f.packed, f.iw, f.ih, r, 5.0);
            let border = if p < 0.6 { accent } else { LINE };
            rrect(cv, r, 5.0, CARD, 0.0, Some(border));
        }
    }
}

/// RgbaImage → packed premul `A<<24|B<<16|G<<8|R` pixels.
fn pack_image(img: &RgbaImage) -> Vec<u32> {
    img.pixels()
        .map(|p| {
            let (r, g, b, a) = (p[0] as u32, p[1] as u32, p[2] as u32, p[3] as u32);
            (a << 24)
                | ((b * a / 255) << 16)
                | ((g * a / 255) << 8)
                | (r * a / 255)
        })
        .collect()
}

/// Cover-crop normal aspects, letterbox extreme ones (scroll shots read
/// as "tall" — the shape is the metadata).
fn make_thumb(img: &RgbaImage, kind: ShotKind, scale: f64) -> (Vec<u32>, usize, usize) {
    let aspect = img.width() as f64 / img.height() as f64;
    let (tw, th) = ((TW * scale) as usize, (TH * scale) as usize);
    let letter = aspect < 0.5 || aspect > 3.0 || kind == ShotKind::Scroll;
    let (iw, ih) = (img.width() as usize, img.height() as usize);
    let mut out = vec![0u32; tw * th]; // transparent → #050505 mat drawn at blit
    let pack = |x: usize, y: usize| -> u32 {
        let p = img.get_pixel(x.min(iw - 1) as u32, y.min(ih - 1) as u32);
        let (r, g, b, a) = (p[0] as u32, p[1] as u32, p[2] as u32, p[3] as u32);
        (a << 24) | ((b * a / 255) << 16) | ((g * a / 255) << 8) | (r * a / 255)
    };
    if letter {
        let fit = (tw as f64 / iw as f64).min(th as f64 / ih as f64);
        let (dw, dh) = ((iw as f64 * fit) as usize, (ih as f64 * fit) as usize);
        let (ox, oy) = ((tw - dw) / 2, (th - dh) / 2);
        for y in 0..dh {
            for x in 0..dw {
                out[(oy + y) * tw + ox + x] = pack((x as f64 / fit) as usize, (y as f64 / fit) as usize);
            }
        }
    } else {
        let s = iw.max(ih) as f64 / tw.max(th) as f64;
        let (ox, oy) = (
            (iw as f64 - tw as f64 * s).max(0.0) / 2.0,
            (ih as f64 - th as f64 * s).max(0.0) / 2.0,
        );
        for y in 0..th {
            for x in 0..tw {
                out[y * tw + x] = pack((ox + x as f64 * s) as usize, (oy + y as f64 * s) as usize);
            }
        }
    }
    (out, tw, th)
}

/* ==================== manager ==================== */

/// Owns one `ChromeWin` per monitor; lazily creates the surface on the
/// monitor the capture came from.
pub struct Chrome {
    pub wins: Vec<ChromeWin>,
    accent: u32,
    pos: String,
    /// builds an event sink for a window id — the host wires its
    /// EventLoopProxy through this so view callbacks become HostEvents
    pub sink: Box<dyn Fn(winit::window::WindowId) -> Rc<dyn Fn(ChromeEv)>>,
}

/// Per-call inputs for `land` — keeps the signature readable.
pub struct Land {
    pub img: RgbaImage,
    pub kind: ShotKind,
    pub path: Option<PathBuf>,
    /// selection in virtual logical coords — the fly's origin
    pub sel: Option<qs_core::Rect>,
    /// skip the fly-in (rehydrated shots land already settled)
    pub instant: bool,
}

impl Chrome {
    pub fn new(
        accent: u32,
        pos: &str,
        sink: Box<dyn Fn(winit::window::WindowId) -> Rc<dyn Fn(ChromeEv)>>,
    ) -> Self {
        Self {
            wins: Vec::new(),
            accent,
            pos: pos.to_string(),
            sink,
        }
    }

    /// Fly the shot into the stack. On failure the `Land` is handed back
    /// so the host can fall back to the quick-access card.
    pub fn land(&mut self, el: &winit::event_loop::ActiveEventLoop, l: Land) -> Result<(), Land> {
        imp::land(self, el, l)
    }

    /// Window events (redraw, cursor-left). Returns true if consumed.
    pub fn window_event(&mut self, id: winit::window::WindowId, ev: &winit::event::WindowEvent) -> bool {
        self.wins.iter_mut().any(|w| w.id() == id && w.window_event(ev))
    }

    /// Events raised by the platform view (mouse, keys). Internal Redraw
    /// commands are handled here; only host-level commands surface.
    pub fn event(
        &mut self,
        id: winit::window::WindowId,
        ev: ChromeEv,
    ) -> Option<(winit::window::WindowId, ChromeCmd)> {
        let w = self.wins.iter_mut().find(|w| w.id() == id)?;
        match w.event(ev) {
            Some(ChromeCmd::Redraw) => {
                w.redraw();
                None
            }
            other => other.map(|c| (id, c)),
        }
    }

    /// `about_to_wait` wake for the fly animation.
    pub fn pump(&mut self) -> Option<Instant> {
        let mut wake = None;
        for w in &mut self.wins {
            if let Some(t) = w.pump() {
                wake = Some(wake.map_or(t, |x: Instant| x.min(t)));
            }
        }
        wake
    }

    /// Look up a stack shot for context-menu actions.
    pub fn shot(&self, id: winit::window::WindowId, idx: usize) -> Option<&Shot> {
        self.wins.iter().find(|w| w.id() == id)?.stack().shot(idx)
    }

    pub fn remove(&mut self, id: winit::window::WindowId, idx: usize) {
        if let Some(w) = self.wins.iter_mut().find(|w| w.id() == id) {
            w.stack_mut().remove(idx);
            w.redraw();
        }
    }

    /// Drop windows whose stack emptied (call after remove).
    pub fn sweep(&mut self) {
        self.wins.retain(|w| !w.stack().is_empty());
    }

    pub fn accent(&self) -> u32 {
        self.accent
    }
    pub fn pos(&self) -> &str {
        &self.pos
    }
}

/// `--chrome` demo: a bare event loop hosting one stack on the primary
/// monitor, landing synthetic shots so the layer can be verified by hand.
pub fn standalone(pos: &str, accent: u32) -> anyhow::Result<()> {
    imp::standalone(pos, accent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn shot_img(w: u32, h: u32, rgb: [u8; 3]) -> RgbaImage {
        // gradient blocks so thumbnails read as distinct captures
        RgbaImage::from_fn(w, h, |x, y| {
            let t = ((x * 255 / w) as u8).wrapping_add((y * 255 / h) as u8);
            Rgba([rgb[0].saturating_add(t / 3), rgb[1], rgb[2].saturating_sub(t / 4), 255])
        })
    }

    /// Renders a 3-shot stack + count chip to /tmp/qs-chrome.png — run
    /// `cargo test -p qs-app chrome` then eyeball the PNG.
    #[test]
    fn render_stack() {
        let mut st = Stack::new(Corner::BR);
        for (kind, rgb) in [
            (ShotKind::Saved, [40, 60, 90]),
            (ShotKind::Shared, [90, 40, 60]),
            (ShotKind::Record, [50, 80, 50]),
        ] {
            st.land(
                shot_img(1200, 800, rgb),
                kind,
                None,
                FRect { x: 100.0, y: 100.0, w: 400.0, h: 300.0 },
                true,
                2.0,
            );
        }
        let (lw, lh) = (1440.0, 900.0);
        let mut buf = vec![0u32; (lw * 2.0) as usize * (lh * 2.0) as usize];
        {
            let mut cv = Canvas { buf: &mut buf, w: 2880, h: 1800, scale: 2.0 };
            let font = crate::settings::system_font().expect("font");
            st.render(&mut cv, &font, 0x52C7EA, Instant::now());
        }
        // premul ABGR-packed → plain RGBA8 for the PNG
        let img = RgbaImage::from_fn(2880, 1800, |x, y| {
            let p = buf[(y as usize) * 2880 + x as usize];
            let a = ((p >> 24) & 0xFF) as u32;
            let unprem = |c: u32| -> u8 {
                if a == 0 { 0 } else { ((c * 255 / a).min(255)) as u8 }
            };
            Rgba([unprem(p & 0xFF), unprem((p >> 8) & 0xFF), unprem((p >> 16) & 0xFF), a as u8])
        });
        img.save("/tmp/qs-chrome.png").unwrap();

        // hover the newest thumb → fan-out + × + accent ring
        st.pointer(
            ChromeEv::Move {
                x: 1440.0 - 20.0 - TW / 2.0,
                y: 900.0 - 20.0 - TH / 2.0,
            },
            1440.0,
            900.0,
        );
        // step the fan tween to its end
        st.settle(Instant::now() + Duration::from_millis(400));
        let mut buf = vec![0u32; 2880 * 1800];
        let mut cv = Canvas { buf: &mut buf, w: 2880, h: 1800, scale: 2.0 };
        let font = crate::settings::system_font().expect("font");
        st.render(&mut cv, &font, 0x52C7EA, Instant::now());
        let img = RgbaImage::from_fn(2880, 1800, |x, y| {
            let p = buf[(y as usize) * 2880 + x as usize];
            let a = ((p >> 24) & 0xFF) as u32;
            let unprem = |c: u32| -> u8 {
                if a == 0 { 0 } else { ((c * 255 / a).min(255)) as u8 }
            };
            Rgba([unprem(p & 0xFF), unprem((p >> 8) & 0xFF), unprem((p >> 16) & 0xFF), a as u8])
        });
        img.save("/tmp/qs-chrome-hover.png").unwrap();
    }

    /// Idle past HIDE_AFTER_MS slides the stack into the edge and the
    /// cascade collapses — only the newest thumb's sliver shows.
    #[test]
    fn idle_hides_to_single_sliver() {
        let mut st = Stack::new(Corner::BR);
        for i in 0..3u8 {
            st.land(
                shot_img(1200, 800, [40 + i * 40, 60, 90]),
                ShotKind::Saved,
                None,
                FRect { x: 100.0, y: 100.0, w: 400.0, h: 300.0 },
                true,
                2.0,
            );
        }
        // wind the idle clock back past the threshold
        st.idle_since = Instant::now() - Duration::from_secs(10);
        st.settle(Instant::now());
        assert!(st.hide_anim, "idle past the deadline must start the hide tween");
        st.settle(Instant::now() + Duration::from_millis(400));
        assert_eq!(st.hide_t, 1.0);

        // every thumb converged on the newest one's rect — one sliver
        let r0 = st.thumb_rect(0, 1440.0, 900.0);
        for i in 1..3 {
            let r = st.thumb_rect(i, 1440.0, 900.0);
            assert_eq!(r.x, r0.x, "thumb {i} must collapse behind the newest");
            assert_eq!(r.y, r0.y);
        }
        // the thumbs sit fully off the right edge; the chip docks as
        // the sole handle flush against it
        assert!(r0.x >= 1440.0, "thumb must be off-screen, x={}", r0.x);
        let c = st.chip_rect(1440.0, 900.0);
        assert!((c.x - (1440.0 - c.w - 2.0)).abs() < 1.0, "chip x={}", c.x);

        // activity brings it back
        st.touch();
        st.settle(Instant::now() + Duration::from_millis(400));
        assert_eq!(st.hide_t, 0.0);
    }

    /// Click thumb0 → lightbox opens; backdrop click closes it.
    #[test]
    fn lightbox_opens_on_thumb_click() {
        let mut st = Stack::new(Corner::TR);
        st.land(
            shot_img(1200, 800, [40, 60, 90]),
            ShotKind::Saved,
            None,
            FRect { x: 100.0, y: 100.0, w: 400.0, h: 300.0 },
            true,
            2.0,
        );
        let r = st.thumb_rect(0, 1440.0, 900.0);
        let cmd = st.pointer(
            ChromeEv::Down { x: r.x + r.w / 2.0, y: r.y + r.h / 2.0 },
            1440.0,
            900.0,
        );
        assert!(matches!(cmd, Some(ChromeCmd::Redraw)));
        assert!(st.lightbox.is_some());
        st.pointer(ChromeEv::Down { x: 700.0, y: 450.0 }, 1440.0, 900.0);
        assert!(st.lightbox.is_none());
    }
}
