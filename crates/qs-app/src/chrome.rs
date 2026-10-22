//! Chrome layer — the post-capture album stack, fly-to-corner animation
//! and lightbox preview, drawn into one per-pixel-alpha surface per
//! monitor (see `chrome/macos.rs` for the AppKit presentation; other
//! platforms get a stub that declines `land` so the host falls back to
//! the quick-access card).
//!
//! Buffers are premultiplied `A<<24 | B<<16 | G<<8 | R` u32s — in memory
//! that is R,G,B,A bytes, what NSBitmapImageRep and wl_shm ARGB8888 take.
//! All drawing goes through `Canvas::blend_over`.

use anyhow::Result;
use image::RgbaImage;
use qs_overlay::sdf::{ellipse_sdf, fill_cov, rrect_sdf, stroke_cov, Canvas};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};
use winit::event_loop::ActiveEventLoop;
use winit::monitor::MonitorHandle;

#[cfg(target_os = "macos")]
#[path = "chrome/macos.rs"]
mod imp;
#[cfg(target_os = "windows")]
#[path = "chrome/windows.rs"]
mod imp;
#[cfg(target_os = "linux")]
#[path = "chrome/linux.rs"]
mod imp;
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
#[path = "chrome/stub.rs"]
mod imp;

pub use imp::ChromeWin;

use crate::config;
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
const HIDE_MS: f64 = 280.0; // slide-away
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
    /// seconds recorded — the ▶ thumb gets a `m:ss` duration badge
    pub dur_s: Option<u64>,
    /// live share link for this shot — draws the 🔗 chip and enables
    /// the "Revoke Link" menu row; set/cleared by the host
    pub shared: Option<crate::share::ShareEntry>,
}

impl Shot {
    /// Premul-packed thumbnail pixels + dims (physical px) — backends
    /// use it as the drag-session image.
    pub fn thumb(&self) -> (&[u32], usize, usize) {
        (&self.thumb, self.tw, self.th)
    }
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
    /// the fanned row's "clear all" pill (Notification-Centre style)
    ClearAll,
    /// context-menu row `i`
    MenuRow(usize),
    /// menu panel background (swallows clicks, no action)
    MenuPanel,
    /// lightbox backdrop (click = close)
    Backdrop,
    /// lightbox action button `i` (BOX_ACTIONS order)
    BoxAction(usize),
}


/// What the host must run in response to a chrome click/key.
#[derive(Debug)]
pub enum ChromeCmd {
    /// state changed — repaint the window
    Redraw,
    /// a context-menu row was picked — `id` is a MENU_* const
    MenuAction {
        idx: usize,
        id: &'static str,
    },
    Remove(usize),
    /// dismiss the whole stack (files on disk are untouched)
    ClearAll,
    /// thumb dragged out — the backend that owns the window starts the
    /// real file-promise drag; consumed locally, never reaches the host
    DragBegin {
        idx: usize,
    },
}

/// Menu row ids — drawn in-chrome, stable across backends.
pub const MENU_REVEAL: &str = "qs-chrome.reveal";
pub const MENU_COPY: &str = "qs-chrome.copy";
pub const MENU_SAVE: &str = "qs-chrome.save";
pub const MENU_PIN: &str = "qs-chrome.pin";
pub const MENU_BEAUTIFY: &str = "qs-chrome.beautify";
pub const MENU_SHARE: &str = "qs-chrome.share";
pub const MENU_REVOKE: &str = "qs-chrome.revoke";
pub const MENU_DEL: &str = "qs-chrome.del";
pub const MENU_CLEAR: &str = "qs-chrome.clear";

/// Lightbox action ids — same MENU_* namespace so host handling routes
/// through the existing on_chrome_menu match.
pub const MENU_OPEN: &str = "qs-chrome.open";
const BOX_ACTIONS: &[&str] = &[MENU_COPY, MENU_OPEN, MENU_SHARE, MENU_DEL];
const BOX_BTN_W: f64 = 104.0;
const BOX_BTN_H: f64 = 32.0;
const BOX_BTN_GAP: f64 = 10.0;

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

/// In-chrome context menu — a native popup can't position inside the
/// chrome surface (muda needs an NSView/HWND/gtk::Window per-OS), so the
/// menu is pixels in the same layer: one path on every backend.
struct Menu {
    /// thumb index the menu is anchored to (0 = newest)
    idx: usize,
    anchor: FRect,
    /// (MENU_* id, enabled)
    rows: Vec<(&'static str, bool)>,
}

const MENU_ROW_H: f64 = 28.0;
const MENU_PAD: f64 = 6.0;
const MENU_W: f64 = 210.0;

/// Per-window stack state — pure logic, platform code only forwards
/// events and asks `render` for pixels.
pub struct Stack {
    pub shots: Vec<Shot>, // newest LAST
    fly: Option<Fly>,
    /// newest thumb is reserved but withheld from the canvas while a
    /// native (Core Animation) fly layer carries it in — macOS only;
    /// the software path leaves this false and draws its own flight
    native_flight: bool,
    hover: Option<Hit>,
    /// cursor within the wider retention zone — hover hysteresis so a
    /// moving rect can't slide out from under a resting cursor (fan
    /// closes → thumb returns under the cursor → opens → jitter loop)
    inside: bool,
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
    menu: Option<Menu>,
    /// a held left-press on a thumb — position + index; `Up` resolves
    /// it to a click, `Dragged` past the threshold to a drag-out
    press: Option<(f64, f64, usize)>,
    corner: Corner,
}

impl Stack {
    pub fn new(corner: Corner) -> Self {
        Self {
            shots: Vec::new(),
            fly: None,
            native_flight: false,
            hover: None,
            inside: false,
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
            menu: None,
            press: None,
            corner,
        }
    }

    /// Queue the fly-in; `from` is the selection rect in window-local
    /// logical coords. `rm` (reduce motion) lands instantly.
    pub fn land(
        &mut self,
        img: RgbaImage,
        kind: ShotKind,
        path: Option<PathBuf>,
        from: FRect,
        rm: bool,
        scale: f64,
        dur_s: Option<u64>,
        shared: Option<crate::share::ShareEntry>,
    ) {
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
            dur_s,
            shared,
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

    /// Withhold the newest thumb from the canvas while a native flight
    /// layer carries it in; the reserved slot still counts in the chip
    /// and paint bounds, so landing swaps in cleanly.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn hold_newest(&mut self, hold: bool) {
        self.native_flight = hold;
    }

    /// Where a native fly should land — the newest thumb at the settled
    /// fan/hide endpoints, so it can't miss a target that is still
    /// tweening when the flight starts.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn landing_rect(&self, w: f64, h: f64) -> FRect {
        self.thumb_rect_at(0, w, h, self.fan_to, self.hide_to)
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
        let y = if self.corner.bottom() {
            h - MARGIN - TH
        } else {
            MARGIN
        };
        // (the chrome view already starts below the menu bar — macOS
        // clamps the window to visibleFrame — so no extra top inset)
        (x, y)
    }

    /// Rect of the thumb `i` positions behind the newest (0 = newest).
    /// Hover/pinned fans them horizontally toward the screen middle.
    fn thumb_rect(&self, i: usize, w: f64, h: f64) -> FRect {
        self.thumb_rect_at(i, w, h, self.fan_t, self.hide_t)
    }

    /// Same math with explicit tween values — `zone_hit` tests the
    /// endpoints so a rect mid-sweep can't flap the retention zone.
    fn thumb_rect_at(&self, i: usize, w: f64, h: f64, fan_t: f64, hide_t: f64) -> FRect {
        let (ax, ay) = self.anchor(w, h);
        let inward = if self.corner.right() { -1.0 } else { 1.0 };
        // hiding collapses every offset so the edge shows ONE sliver
        // (the newest thumb) instead of a diagonal trail of peeks
        let collapse = 1.0 - hide_t;
        let dx = inward * (STACK_DX + (FAN - STACK_DX) * fan_t) * i as f64 * collapse;
        // edge-hidden slides thumbs fully off-screen — the docked chip
        // is the only visible handle (a sliver + floating chip read as
        // two disjoint artifacts)
        let hide_dx = -inward * hide_t * (TW + MARGIN);
        // the fan is a straight horizontal row — the cascade's vertical
        // offset collapses as fan_t opens (was spreading diagonally)
        let dy = if self.corner.bottom() {
            -STACK_DY
        } else {
            STACK_DY
        } * i as f64
            * (1.0 - fan_t)
            * collapse;
        FRect {
            x: ax + dx + hide_dx,
            y: ay + dy,
            w: TW,
            h: TH,
        }
    }

    fn chip_rect(&self, w: f64, h: f64) -> FRect {
        self.chip_rect_at(w, h, self.hide_t)
    }

    fn chip_rect_at(&self, w: f64, h: f64, hide_t: f64) -> FRect {
        let (ax, ay) = self.anchor(w, h);
        let (cw, ch) = (64.0, 20.0);
        // beside the newest thumb, aligned to its far edge — never
        // clipped by the screen edge regardless of the dock corner
        let shown = if self.corner.right() {
            ax - cw - 8.0
        } else {
            ax + TW + 8.0
        };
        // hidden: dock flush against the screen edge as the sole handle
        let docked = if self.corner.right() {
            w - cw - 2.0
        } else {
            2.0
        };
        let x = shown + (docked - shown) * hide_t;
        let y = if self.corner.bottom() {
            ay + TH - ch
        } else {
            ay
        };
        FRect { x, y, w: cw, h: ch }
    }

    /// "CLEAR SHELF" pill centred under the fanned row — like the
    /// Clear All at the edge of Notification Centre's list, it only
    /// dismisses the shelf; files on disk are untouched. Exists once
    /// the fan is mostly open (hit/live gating matches `input_rects`).
    fn clear_rect(&self, w: f64, h: f64) -> Option<FRect> {
        if self.shots.len() < 2 || self.fan_t < 0.5 {
            return None;
        }
        let n = self
            .shots
            .len()
            .min(if self.pinned { MAX_VIS_PINNED } else { MAX_VIS });
        let first = self.thumb_rect_at(0, w, h, self.fan_t, self.hide_t);
        let last = self.thumb_rect_at(n - 1, w, h, self.fan_t, self.hide_t);
        let (pw, ph) = (92.0, 22.0);
        let (left, right) = if first.x < last.x {
            (first.x, last.x + last.w)
        } else {
            (last.x, first.x + first.w)
        };
        let x = (left + right) / 2.0 - pw / 2.0;
        // below the row for top corners, above it for bottom ones
        let y = if self.corner.bottom() {
            first.y - ph - 10.0
        } else {
            first.y + first.h + 10.0
        };
        Some(FRect { x, y, w: pw, h: ph })
    }

    /* ---- in-chrome context menu ---- */

    /// Open the thumb menu at the click point. Row ids are the MENU_*
    /// consts; the host answers `MenuAction` events.
    fn open_menu(&mut self, idx: usize, x: f64, y: f64) {
        let has_path = self.shot(idx).and_then(|s| s.path.as_ref()).is_some();
        let revocable = self
            .shot(idx)
            .and_then(|s| s.shared.as_ref())
            .is_some_and(|e| !e.delete_key.is_empty());
        self.menu = Some(Menu {
            idx,
            anchor: FRect {
                x,
                y,
                w: 0.0,
                h: 0.0,
            },
            rows: vec![
                (MENU_REVEAL, has_path),
                (MENU_SAVE, !has_path),
                (MENU_COPY, true),
                (MENU_PIN, true),
                (MENU_BEAUTIFY, true),
                (MENU_SHARE, !revocable),
                (MENU_REVOKE, revocable),
                (MENU_DEL, true),
                (MENU_CLEAR, true),
            ],
        });
    }

    /// Menu panel rect — anchored at the click, clamped inside the
    /// monitor like a native menu.
    fn menu_rect(&self, w: f64, h: f64) -> Option<FRect> {
        let m = self.menu.as_ref()?;
        let mh = m.rows.len() as f64 * MENU_ROW_H + MENU_PAD * 2.0;
        let x = m.anchor.x.min(w - MENU_W - 4.0).max(4.0);
        let y = m.anchor.y.min(h - mh - 4.0).max(4.0);
        Some(FRect {
            x,
            y,
            w: MENU_W,
            h: mh,
        })
    }

    /// Row rect `i` inside the panel.
    fn menu_row_rect(&self, i: usize, w: f64, h: f64) -> Option<FRect> {
        let p = self.menu_rect(w, h)?;
        Some(FRect {
            x: p.x + MENU_PAD,
            y: p.y + MENU_PAD + i as f64 * MENU_ROW_H,
            w: p.w - MENU_PAD * 2.0,
            h: MENU_ROW_H,
        })
    }

    fn menu_row_at(&self, x: f64, y: f64, w: f64, h: f64) -> Option<usize> {
        let m = self.menu.as_ref()?;
        for i in 0..m.rows.len() {
            if let Some(r) = self.menu_row_rect(i, w, h) {
                if x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h {
                    return Some(i);
                }
            }
        }
        None
    }

    /// Menu label for a row id — rendered at draw time.
    fn menu_label(id: &'static str) -> &'static str {
        match id {
            MENU_REVEAL => {
                if cfg!(target_os = "macos") {
                    "Reveal in Finder"
                } else if cfg!(target_os = "windows") {
                    "Reveal in Explorer"
                } else {
                    "Reveal in Folder"
                }
            }
            MENU_SAVE => "Save to Screenshots",
            MENU_OPEN => {
                if cfg!(target_os = "macos") {
                    "Open in Preview"
                } else {
                    "Open in Editor"
                }
            }
            MENU_COPY => "Copy Image",
            MENU_PIN => "Pin",
            MENU_BEAUTIFY => "Beautify",
            MENU_SHARE => "Share Link",
            MENU_REVOKE => "Revoke Link",
            MENU_DEL => "Remove from Shelf",
            MENU_CLEAR => "Clear Shelf",
            _ => "?",
        }
    }

    /// The wide retention zone as rects — platform backends feed these
    /// to their input-region API (Wayland `wl_region`, X11 XShape,
    /// Windows `SetWindowRgn`) so pointer moves inside the hysteresis
    /// zone still reach the surface while the rest of the desktop stays
    /// click-through. Rects may overlap; regions union them.
    #[allow(dead_code)] // consumed by the Linux/Windows backends' input regions
    pub fn zone_rects(&self, w: f64, h: f64) -> Vec<FRect> {
        if self.lightbox.is_some() || self.menu.is_some() {
            return vec![FRect {
                x: 0.0,
                y: 0.0,
                w,
                h,
            }];
        }
        let mut v = Vec::new();
        let n = self
            .shots
            .len()
            .min(if self.pinned { MAX_VIS_PINNED } else { MAX_VIS });
        for &ht in &[self.hide_t, 0.0, 1.0] {
            for &ft in &[self.fan_t, 0.0, 1.0] {
                for i in 0..n {
                    v.push(self.thumb_rect_at(i, w, h, ft, ht));
                }
                if !self.shots.is_empty() {
                    v.push(self.chip_rect_at(w, h, ht));
                }
            }
        }
        for r in &mut v {
            r.x -= 28.0;
            r.y -= 28.0;
            r.w += 56.0;
            r.h += 56.0;
        }
        v
    }

    /// Union of everything `render` can touch this frame, in logical
    /// window coords — backends use it as the dirty rect so the layer
    /// upload covers the moving primitives, not the whole monitor.
    /// `None` means nothing is on screen (empty + fully hidden).
    pub fn paint_bounds(&self, w: f64, h: f64, now: Instant) -> Option<FRect> {
        if self.lightbox.is_some() || self.menu.is_some() {
            return Some(FRect {
                x: 0.0,
                y: 0.0,
                w,
                h,
            });
        }
        let mut b: Option<FRect> = None;
        let mut acc = |r: FRect| match &mut b {
            Some(u) => {
                let x0 = u.x.min(r.x);
                let y0 = u.y.min(r.y);
                u.w = (u.x + u.w).max(r.x + r.w) - x0;
                u.h = (u.y + u.h).max(r.y + r.h) - y0;
                u.x = x0;
                u.y = y0;
            }
            None => b = Some(r),
        };
        // thumbs at both tween ends (fan/hide sweep between them) —
        // shadows and the × disc live in the pad
        let n = self
            .shots
            .len()
            .min(if self.pinned { MAX_VIS_PINNED } else { MAX_VIS });
        for i in 0..n {
            for &ft in &[self.fan_t, self.fan_to] {
                for &ht in &[self.hide_t, self.hide_to] {
                    acc(self.thumb_rect_at(i, w, h, ft, ht));
                }
            }
        }
        for &ht in &[self.hide_t, self.hide_to] {
            acc(self.chip_rect_at(w, h, ht));
        }
        if let Some(r) = self.clear_rect(w, h) {
            acc(r);
        }
        if let Some(f) = &self.fly {
            // current position; the previous frame's rect is covered by
            // the caller's prev-dirty union
            let p = ease_out(self.fly_t(now));
            let to = self.thumb_rect(0, w, h);
            acc(FRect {
                x: f.from.x + (to.x - f.from.x) * p,
                y: f.from.y + (to.y - f.from.y) * p,
                w: f.from.w + (to.w - f.from.w) * p,
                h: f.from.h + (to.h - f.from.h) * p,
            });
        }
        b.map(|mut r| {
            r.x -= 30.0;
            r.y -= 30.0;
            r.w += 60.0;
            r.h += 60.0;
            r.x = r.x.max(0.0);
            r.y = r.y.max(0.0);
            r.w = r.w.min(w - r.x);
            r.h = r.h.min(h - r.y);
            r
        })
    }

    /// rects are padded so the × disc overhanging the edge stays live.
    pub fn input_rects(&self, w: f64, h: f64) -> Vec<(FRect, Hit)> {
        if self.lightbox.is_some() || self.menu.is_some() {
            return vec![(
                FRect {
                    x: 0.0,
                    y: 0.0,
                    w,
                    h,
                },
                Hit::Backdrop,
            )];
        }
        let mut v = Vec::new();
        // the pill sits in the thumbs' hit padding — first so it wins
        if let Some(c) = self.clear_rect(w, h) {
            v.push((c, Hit::ClearAll));
        }
        let n = self
            .shots
            .len()
            .min(if self.pinned { MAX_VIS_PINNED } else { MAX_VIS });
        // a native flight has not landed yet — its slot must not catch
        // clicks at a position where no thumb is drawn
        for i in usize::from(self.native_flight)..n {
            let mut r = self.thumb_rect(i, w, h);
            r.x -= 12.0;
            r.y -= 12.0;
            r.w += 24.0;
            r.h += 24.0;
            v.push((r, Hit::Thumb(i)));
        }
        if !self.shots.is_empty() {
            v.push((self.chip_rect(w, h), Hit::Chip));
        }
        v
    }

    /// Retention zone around the interactive rects — while `wide` the
    /// fan/hide tweens only collapse once the cursor leaves the union of
    /// BOTH animation endpoints (the union covers the swept path, so
    /// mid-tween geometry can never flip the answer). Without the union
    /// a thumb can slide out from under a resting cursor, hover drops,
    /// the tween reverses, the thumb slides back — a jitter loop.
    /// Acquire (`wide` = false) stays at current geometry so the
    /// hover area doesn't swallow the screen.
    fn zone_hit(&self, x: f64, y: f64, w: f64, h: f64, wide: bool) -> bool {
        if self.lightbox.is_some() || self.menu.is_some() {
            return true;
        }
        let pad = if wide { 28.0 } else { 12.0 };
        let fan_opts: &[f64] = if wide {
            &[self.fan_t, 0.0, 1.0][..]
        } else {
            &[self.fan_t][..]
        };
        let hide_opts: &[f64] = if wide {
            &[self.hide_t, 0.0, 1.0][..]
        } else {
            &[self.hide_t][..]
        };
        let n = self
            .shots
            .len()
            .min(if self.pinned { MAX_VIS_PINNED } else { MAX_VIS });
        for &ht in hide_opts {
            for &ft in fan_opts {
                for i in usize::from(self.native_flight)..n {
                    let r = self.thumb_rect_at(i, w, h, ft, ht);
                    if x >= r.x - pad
                        && x <= r.x + r.w + pad
                        && y >= r.y - pad
                        && y <= r.y + r.h + pad
                    {
                        return true;
                    }
                }
                if !self.shots.is_empty() {
                    let c = self.chip_rect_at(w, h, ht);
                    if x >= c.x - pad
                        && x <= c.x + c.w + pad
                        && y >= c.y - pad
                        && y <= c.y + c.h + pad
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Point (logical) → role.
    pub fn hit(&self, x: f64, y: f64, w: f64, h: f64) -> Option<Hit> {
        if self.menu.is_some() {
            if let Some(i) = self.menu_row_at(x, y, w, h) {
                return Some(Hit::MenuRow(i));
            }
            if let Some(p) = self.menu_rect(w, h) {
                if x >= p.x && x <= p.x + p.w && y >= p.y && y <= p.y + p.h {
                    return Some(Hit::MenuPanel);
                }
            }
            return Some(Hit::Backdrop);
        }
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
        // lightbox action bar sits inside the backdrop — check its
        // buttons first so a click hits the action, not "close"
        if self.lightbox.is_some() {
            for (i, r) in self.box_action_rects(w, h).iter().enumerate() {
                if x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h {
                    return Some(Hit::BoxAction(i));
                }
            }
            return Some(Hit::Backdrop);
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

    /// Settled-position image rect the lightbox draws into — shared by
    /// the action bar layout and hit testing.
    fn box_img_rect(&self, w: f64, h: f64) -> FRect {
        let fit = (w * 0.8 / self.box_iw as f64).min(h * 0.8 / self.box_ih as f64);
        let dw = self.box_iw as f64 * fit;
        let dh = self.box_ih as f64 * fit;
        FRect {
            x: (w - dw) / 2.0,
            y: (h - dh) / 2.0 - 10.0,
            w: dw,
            h: dh,
        }
    }

    /// One rect per BOX_ACTIONS entry, centred under the preview.
    fn box_action_rects(&self, w: f64, h: f64) -> Vec<FRect> {
        let img = self.box_img_rect(w, h);
        let n = BOX_ACTIONS.len() as f64;
        let total = n * BOX_BTN_W + (n - 1.0) * BOX_BTN_GAP;
        let x0 = (w - total) / 2.0;
        let mut y = img.y + img.h + 18.0;
        if y + BOX_BTN_H > h - 12.0 {
            // tall images: dock the bar over the bottom margin instead
            y = img.y + img.h - BOX_BTN_H - 14.0;
        }
        BOX_ACTIONS
            .iter()
            .enumerate()
            .map(|(i, _)| FRect {
                x: x0 + i as f64 * (BOX_BTN_W + BOX_BTN_GAP),
                y,
                w: BOX_BTN_W,
                h: BOX_BTN_H,
            })
            .collect()
    }

    /// Platform events distilled into state transitions; the returned
    /// command is for the host (menu, remove).
    pub fn pointer(&mut self, ev: ChromeEv, w: f64, h: f64) -> Option<ChromeCmd> {
        match ev {
            ChromeEv::Move { x, y } => {
                let hit = self.hit(x, y, w, h);
                let inside = hit.is_some() || self.zone_hit(x, y, w, h, self.inside);
                if hit != self.hover || inside != self.inside {
                    // real hover transitions are activity — synthetic
                    // Move re-emits (sync_input, global monitor) aren't
                    self.touch();
                    self.hover = hit;
                    self.inside = inside;
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
                self.inside = false;
                if self.hover.take().is_some() {
                    self.retarget_fan();
                    return Some(ChromeCmd::Redraw);
                }
                self.retarget_fan();
                None
            }
            ChromeEv::Down { x, y } => {
                self.touch();
                match self.hit(x, y, w, h) {
                    Some(Hit::Del(i)) => Some(ChromeCmd::Remove(i)),
                    Some(Hit::ClearAll) => Some(ChromeCmd::ClearAll),
                    Some(Hit::Chip) => {
                        self.pinned = !self.pinned;
                        self.retarget_fan();
                        Some(ChromeCmd::Redraw)
                    }
                    Some(Hit::Thumb(i)) => {
                        // click resolves on release — the same press may
                        // become a drag-out instead
                        self.press = Some((x, y, i));
                        Some(ChromeCmd::Redraw)
                    }
                    Some(Hit::MenuRow(i)) => {
                        let m = self.menu.take().unwrap();
                        let (id, enabled) = m.rows[i];
                        if enabled {
                            Some(ChromeCmd::MenuAction { idx: m.idx, id })
                        } else {
                            // disabled row: keep the menu open, like a native one
                            self.menu = Some(m);
                            Some(ChromeCmd::Redraw)
                        }
                    }
                    Some(Hit::MenuPanel) => None, // swallowed, menu stays
                    Some(Hit::BoxAction(i)) => self.lightbox.and_then(|idx| {
                        BOX_ACTIONS
                            .get(i)
                            .map(|&id| ChromeCmd::MenuAction { idx, id })
                    }),
                    Some(Hit::Backdrop) => {
                        self.lightbox = None;
                        self.menu = None;
                        Some(ChromeCmd::Redraw)
                    }
                    None => None,
                }
            }
            ChromeEv::RightDown { x, y } => {
                self.touch();
                match self.hit(x, y, w, h) {
                    Some(Hit::Thumb(i)) | Some(Hit::Del(i)) => {
                        self.open_menu(i, x, y);
                        Some(ChromeCmd::Redraw)
                    }
                    // a second right-click on another thumb retargets the
                    // menu; anywhere else it dismisses — matching native
                    // context-menu behaviour. hit() runs with the menu
                    // taken so thumbs are visible to it again.
                    _ => {
                        if self.menu.take().is_some() {
                            if let Some(Hit::Thumb(i) | Hit::Del(i)) = self.hit(x, y, w, h) {
                                self.open_menu(i, x, y);
                            }
                            Some(ChromeCmd::Redraw)
                        } else {
                            None
                        }
                    }
                }
            }
            ChromeEv::Dragged { x, y } => {
                self.touch();
                match self.press.take() {
                    Some((px, py, i)) if (x - px).hypot(y - py) >= 4.0 => {
                        // past the native drag threshold — the backend
                        // turns this into a real file-promise drag
                        self.press = None;
                        Some(ChromeCmd::DragBegin { idx: i })
                    }
                    Some((px, py, i)) => {
                        self.press = Some((px, py, i));
                        None
                    }
                    None => None,
                }
            }
            ChromeEv::Up { x, y } => {
                self.touch();
                match self.press.take() {
                    // released on the same thumb → the deferred click
                    Some((_, _, i)) if matches!(self.hit(x, y, w, h), Some(Hit::Thumb(j)) if j == i) =>
                    {
                        self.open_lightbox(i);
                        Some(ChromeCmd::Redraw)
                    }
                    // released elsewhere — just a cancelled press
                    Some(_) => None,
                    None => None,
                }
            }
            ChromeEv::Key(_) => None, // keys go through `key()`
            ChromeEv::Tick => None, // backend-driven — never enters pointer logic
        }
    }

    /// Keyboard input while the chrome window is key — arrows move the
    /// ring, Enter previews, O/S open and share a preview, ⌫ removes,
    /// Esc peels a layer off.
    pub fn key(&mut self, k: Key) -> Option<ChromeCmd> {
        self.touch();
        let n = self
            .shots
            .len()
            .min(if self.pinned { MAX_VIS_PINNED } else { MAX_VIS });
        if k == Key::Esc && self.menu.take().is_some() {
            return Some(ChromeCmd::Redraw);
        }
        if self.menu.is_some() {
            return None; // menu is modal while open
        }
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
            (Key::Enter, None) | (Key::Space, None) => match self.key_target(n) {
                Some(i) => {
                    self.open_lightbox(i);
                    Some(ChromeCmd::Redraw)
                }
                None => None,
            },
            // Enter inside a lightbox copies the previewed shot — the
            // macshot mapping the r2 spec steals
            (Key::Enter, Some(_)) => self.key_target(n).map(|i| ChromeCmd::MenuAction {
                idx: i,
                id: MENU_COPY,
            }),
            // the lightbox legend's O / S — the same actions the bar
            // buttons expose, without a mouse
            (Key::Open, Some(_)) => self.key_target(n).map(|i| ChromeCmd::MenuAction {
                idx: i,
                id: MENU_OPEN,
            }),
            (Key::Share, Some(_)) => self.key_target(n).map(|i| ChromeCmd::MenuAction {
                idx: i,
                id: MENU_SHARE,
            }),
            (Key::Copy, _) => self.key_target(n).map(|i| ChromeCmd::MenuAction {
                idx: i,
                id: MENU_COPY,
            }),
            (Key::Back, _) => match self.key_target(n) {
                Some(i) => Some(ChromeCmd::Remove(i)),
                None => None,
            },
            _ => None,
        }
    }

    /// The shot a keypress applies to: the ring selection first, then
    /// a hovered thumb, falling back to the newest — matches the
    /// hover-and-arrows-share-one-ring contract.
    fn key_target(&self, n: usize) -> Option<usize> {
        // an open lightbox owns the action — ⌘C/⌫ apply to the shot
        // being previewed, not whatever sits under the ring
        self.lightbox
            .filter(|i| *i < n)
            .or(self.ring.filter(|i| *i < n))
            .or_else(|| {
                self.hover.and_then(|h| match h {
                    Hit::Thumb(i) | Hit::Del(i) => Some(i),
                    _ => None,
                })
            })
            .or(if n > 0 { Some(0) } else { None })
    }

    /// Retarget the fan tween whenever hover/pinned flips — opening
    /// overshoots slightly (ease-out-back, ~280ms) like a native
    /// popover; closing is a quicker plain ease-out (~180ms).
    fn retarget_fan(&mut self) {
        let t = if self.pinned || self.inside { 1.0 } else { 0.0 };
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
            && !self.inside
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
            let (p, w, h) = (
                pack_image(&s.img),
                s.img.width() as usize,
                s.img.height() as usize,
            );
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
            let ms = if self.fan_to > self.fan_from {
                FAN_OPEN_MS
            } else {
                FAN_CLOSE_MS
            };
            let p = (now.duration_since(self.fan_t0).as_secs_f64() * 1000.0 / ms).min(1.0);
            let e = if self.fan_to > self.fan_from {
                ease_out_back(p)
            } else {
                ease_out(p)
            };
            self.fan_t = self.fan_from + (self.fan_to - self.fan_from) * e;
            if p >= 1.0 {
                self.fan_t = self.fan_to;
                self.fan_anim = false;
            }
        }
        // idle → slide into the edge (a fresh shot resets the clock at land)
        if self.hide_to == 0.0
            && self.hide_t == 0.0
            && !self.inside
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
            let ms = if self.hide_to > self.hide_from {
                HIDE_MS
            } else {
                UNHIDE_MS
            };
            let p = (now.duration_since(self.hide_t0).as_secs_f64() * 1000.0 / ms).min(1.0);
            let e = if self.hide_to > self.hide_from {
                ease_out(p)
            } else {
                ease_out_back(p)
            };
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
        self.menu = None;
        self.retarget_fan();
    }

    /// Dismiss the whole stack — "clear all" like Notification Centre.
    /// Files on disk are untouched; this only empties the shelf.
    pub fn clear_all(&mut self) {
        self.shots.clear();
        self.fly = None;
        self.native_flight = false;
        self.lightbox = None;
        self.box_t0 = None;
        self.box_img.clear();
        self.menu = None;
        self.hover = None;
        self.inside = false;
        self.pinned = false;
        self.ring = None;
        self.fan_t = 0.0;
        self.fan_from = 0.0;
        self.fan_to = 0.0;
        self.fan_anim = false;
        self.hide_t = 0.0;
        self.hide_from = 0.0;
        self.hide_to = 0.0;
        self.hide_anim = false;
        self.idle_since = Instant::now();
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
    Move {
        x: f64,
        y: f64,
    },
    Leave,
    Down {
        x: f64,
        y: f64,
    },
    RightDown {
        x: f64,
        y: f64,
    },
    /// The left button moved past the drag threshold while held — the
    /// backend distinguishes this from `Move` itself (macOS
    /// `mouseDragged:`, winit `CursorMoved` + cached button state).
    Dragged {
        x: f64,
        y: f64,
    },
    /// Left button released — a thumb click resolves here so the same
    /// press can become a `Dragged` instead.
    Up {
        x: f64,
        y: f64,
    },
    Key(Key),
    /// Native vsync tick — the backend fires this from its display clock
    /// (CADisplayLink / wl_surface.frame) so animation settles and
    /// presents on the compositor's cadence instead of a timer.
    Tick,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Key {
    Esc,
    Left,
    Right,
    Enter,
    Back,
    /// Space — quick look, same preview the Enter key opens
    Space,
    /// ⌘C / Ctrl+C — copy the focused shot (backends supply the
    /// modifier check)
    Copy,
    /// O — open the shot being previewed
    Open,
    /// S — share the shot being previewed
    Share,
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
/// consistent. The bounds check is load-bearing: without it an
/// off-screen rect (e.g. a thumb hidden past the right edge) wraps
/// `y*w + x` into the next row — the image re-appears on the LEFT.
#[inline]
fn blit_px(cv: &mut Canvas, x: i32, y: i32, px: u32, cov: f64) {
    if cov <= 0.0 || x < 0 || y < 0 || x >= cv.w as i32 || y >= cv.h as i32 {
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
    // clip to the buffer — off-screen pixels are dead work, and would
    // wrap into visible rows downstream
    let (x0, x1) = (
        ((dst.x * s) as i32).max(0),
        (((dst.x + dst.w) * s).ceil() as i32).min(cv.w as i32),
    );
    let (y0, y1) = (
        ((dst.y * s) as i32).max(0),
        (((dst.y + dst.h) * s).ceil() as i32).min(cv.h as i32),
    );
    for py in y0..y1 {
        for px in x0..x1 {
            let lx = px as f64 / s + 0.5 / s;
            let ly = py as f64 / s + 0.5 / s;
            if rad > 0.0 {
                let d = rrect_sdf(
                    lx,
                    ly,
                    dst.x + dst.w / 2.0,
                    dst.y + dst.h / 2.0,
                    dst.w / 2.0,
                    dst.h / 2.0,
                    rad,
                );
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
    // +0.5/s: evaluate the SDF at pixel CENTERS like glass_card — corner
    // sampling lands the edge exactly on the top/left boundary but half
    // a pixel past it on bottom/right, so the hairline dies there
    for py in (r.y * s) as i32..((r.y + r.h) * s) as i32 {
        for px in (r.x * s) as i32..((r.x + r.w) * s) as i32 {
            let d = rrect_sdf(
                px as f64 / s + 0.5 / s,
                py as f64 / s + 0.5 / s,
                r.x + r.w / 2.0,
                r.y + r.h / 2.0,
                r.w / 2.0,
                r.h / 2.0,
                rad,
            );
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
            let d = ellipse_sdf(
                px as f64 / s + 0.5 / s,
                py as f64 / s + 0.5 / s,
                cx,
                cy,
                r,
                r,
            );
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
        let t = Txt {
            font,
            scale: cv.scale,
        };

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
                let settled = self.box_img_rect(lw, lh);
                let r = FRect {
                    x: settled.x + settled.w * (1.0 - grow) / 2.0,
                    y: settled.y + settled.h * (1.0 - grow) / 2.0,
                    w: settled.w * grow,
                    h: settled.h * grow,
                };
                blit_img(cv, &self.box_img, self.box_iw, self.box_ih, r, 8.0);
                let fr = FRect {
                    x: r.x - 1.0,
                    y: r.y - 1.0,
                    w: r.w + 2.0,
                    h: r.h + 2.0,
                };
                rrect(cv, fr, 9.0, CARD, 0.0, Some(LINE));
                if bp >= 1.0 {
                    // action bar — the mouse path out of the preview
                    // (keys: Esc close, Enter copy, O open, S share,
                    // ⌫ remove)
                    for (bi, br) in self.box_action_rects(lw, lh).iter().enumerate() {
                        let hot = self.hover == Some(Hit::BoxAction(bi));
                        let danger = BOX_ACTIONS[bi] == MENU_DEL;
                        // white labels need the deepened accent — raw
                        // orange/pink carry white at only ~2-3:1
                        let fill = if hot { config::deepen(accent) } else { CARD };
                        let fa = if hot { 0.9 } else { 0.92 };
                        rrect(cv, *br, 8.0, fill, fa, Some(LINE));
                        let label = Self::menu_label(BOX_ACTIONS[bi]);
                        let tw = t.width(10.0, label);
                        t.draw_over(
                            cv,
                            br.x + (br.w - tw) / 2.0,
                            br.y + 12.5,
                            10.0,
                            label,
                            if danger && !hot { RED } else { 0xFFFFFF },
                        );
                    }
                    // shortcut legend, on its own pill so it stays legible
                    // over any wallpaper; skipped when a tall image leaves
                    // no room below the action bar
                    let hint = "ESC CLOSE · O OPEN · S SHARE · ⌫ REMOVE";
                    let hw = t.width(9.5, hint);
                    let hy = settled.y + settled.h + 58.0;
                    if hy + 22.0 <= lh - 8.0 {
                        let hr = FRect {
                            x: (lw - hw) / 2.0 - 10.0,
                            y: hy,
                            w: hw + 20.0,
                            h: 22.0,
                        };
                        rrect(cv, hr, 11.0, CARD, 0.92, Some(LINE));
                        t.draw_over(cv, hr.x + 10.0, hr.y + 15.0, 9.5, hint, MUTED);
                    }
                }
            }
            return;
        }

        // count chip
        if !self.shots.is_empty() {
            let cr = self.chip_rect(lw, lh);
            rrect(cv, cr, 10.0, CARD, 0.92, Some(LINE));
            let n = self.shots.len();
            let label = if n == 1 {
                "1 SHOT".to_string()
            } else {
                format!("{n} SHOTS")
            };
            let tw = t.width(9.5, &label);
            t.draw_over(
                cv,
                cr.x + (cr.w - tw) / 2.0,
                cr.y + 14.0,
                9.5,
                &label,
                MUTED,
            );
        }

        // thumbs, oldest first so newest draws on top
        let n = self
            .shots
            .len()
            .min(if self.pinned { MAX_VIS_PINNED } else { MAX_VIS });
        for i in (0..n).rev() {
            // hit index i counts from newest; draw order is reverse
            let idx = self.shots.len() - 1 - i; // storage idx
            let Some(shot) = self.shots.get(idx) else {
                continue;
            };
            let in_flight =
                idx == self.shots.len() - 1 && (self.fly.is_some() || self.native_flight);
            if in_flight {
                continue; // its pixels travel on the (native) fly layer
            }
            let r = self.thumb_rect(i, lw, lh);
            blit_img(cv, &shot.thumb, shot.tw, shot.th, r, 6.0);
            let hot = self.hover == Some(Hit::Thumb(i))
                || self.hover == Some(Hit::Del(i))
                || self.ring == Some(i);
            rrect(cv, r, 6.0, CARD, 0.0, Some(if hot { accent } else { LINE }));

            // badges are corner-anchored: while the stack is collapsed
            // only the top card can wear them, or every offset card
            // piles a badge into the same corner
            let detailed = i == 0 || self.fan_t >= 0.5;
            if detailed {
                // distinct marks per outcome — four different actions all
                // showing "✓" told the user nothing at a glance
                let (bg, glyph) = match shot.kind {
                    ShotKind::Saved => (GRN, "✓"),
                    ShotKind::Copied => (GRN, "⧉"),
                    ShotKind::Ocr => (GRN, "T"),
                    ShotKind::Scroll => (GRN, "↕"),
                    ShotKind::Shared => (accent, "↗"),
                    ShotKind::Record => (accent, "▶"),
                };
                let (bx, by) = (r.x + 6.0, r.y + r.h - 6.0);
                disc(cv, bx, by, 8.0, bg, 0.95);
                t.draw_over(
                    cv,
                    bx - t.width(9.0, glyph) / 2.0,
                    by + 3.4,
                    9.0,
                    glyph,
                    0x000000,
                );

                // a live share link gets its own dot on the opposite
                // corner — the kind badge marks provenance, ● marks
                // "this file is still public until revoked/expired"
                if shot.shared.is_some() {
                    let lx = r.x + r.w - 6.0;
                    let ly = r.y + r.h - 6.0;
                    disc(cv, lx, ly, 8.0, accent, 0.95);
                    t.draw_over(
                        cv,
                        lx - t.width(9.0, "●") / 2.0,
                        ly + 3.4,
                        9.0,
                        "●",
                        0x000000,
                    );
                }

                // recordings also carry their length — a `m:ss` pill
                // hugging the ▶ disc, bottom-left like CleanShot
                if shot.kind == ShotKind::Record {
                    if let Some(d) = shot.dur_s {
                        let label = format!("{}:{:02}", d / 60, d % 60);
                        let tw = t.width(8.5, &label);
                        let (px, py) = (bx + 10.0, by + 5.5);
                        rrect(
                            cv,
                            FRect {
                                x: px - 4.0,
                                y: py - 5.0,
                                w: tw + 8.0,
                                h: 11.0,
                            },
                            5.5,
                            CARD,
                            0.9,
                            None,
                        );
                        t.draw_over(cv, px, py, 8.5, &label, 0xFFFFFF);
                    }
                }
            }

            // × on hover
            if self.hover == Some(Hit::Thumb(i)) || self.hover == Some(Hit::Del(i)) {
                let (dx, dy) = self.del_pos(r);
                disc(cv, dx, dy, 8.0, CARD, 0.95);
                let del_hot = self.hover == Some(Hit::Del(i));
                t.draw_over(
                    cv,
                    dx - t.width(10.0, "×") / 2.0,
                    dy + 3.6,
                    10.0,
                    "×",
                    if del_hot { RED } else { MUTED },
                );
            }
        }

        // "CLEAR SHELF" pill under the row, fading in with the fan —
        // red text on hover like the per-thumb × (dismiss-only)
        if let Some(cr) = self.clear_rect(lw, lh) {
            let a = ((self.fan_t - 0.4) / 0.6).clamp(0.0, 1.0);
            let hot = self.hover == Some(Hit::ClearAll);
            rrect(
                cv,
                cr,
                11.0,
                CARD,
                0.92 * a,
                Some(if hot { accent } else { LINE }),
            );
            let label = "× CLEAR SHELF";
            let tw = t.width(9.5, label);
            t.draw_over(
                cv,
                cr.x + (cr.w - tw) / 2.0,
                cr.y + 15.0,
                9.5,
                label,
                if hot { RED } else { MUTED },
            );
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

        // context menu floats above everything in the layer
        if self.menu.is_some() {
            if let Some(p) = self.menu_rect(lw, lh) {
                // dim everything behind the menu slightly so it pops
                // (backdrop clicks close it — hit() handles routing)
                rrect(cv, p, 10.0, CARD, 0.96, Some(LINE));
                let m = self.menu.as_ref().unwrap();
                for (i, &(id, enabled)) in m.rows.iter().enumerate() {
                    let Some(rr) = self.menu_row_rect(i, lw, lh) else {
                        continue;
                    };
                    let hot = self.hover == Some(Hit::MenuRow(i)) && enabled;
                    if hot {
                        rrect(cv, rr, 6.0, accent, 0.22, None);
                    }
                    let col = if !enabled {
                        LINE
                    } else if hot {
                        0xFFFFFF
                    } else {
                        MUTED
                    };
                    t.draw_over(
                        cv,
                        rr.x + 10.0,
                        rr.y + 19.0,
                        10.5,
                        Self::menu_label(id),
                        col,
                    );
                }
            }
        }
    }
}

/// RgbaImage → packed premul `A<<24|B<<16|G<<8|R` pixels.
fn pack_image(img: &RgbaImage) -> Vec<u32> {
    img.pixels()
        .map(|p| {
            let (r, g, b, a) = (p[0] as u32, p[1] as u32, p[2] as u32, p[3] as u32);
            (a << 24) | ((b * a / 255) << 16) | ((g * a / 255) << 8) | (r * a / 255)
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
                out[(oy + y) * tw + ox + x] =
                    pack((x as f64 / fit) as usize, (y as f64 / fit) as usize);
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
    /// recording length in seconds — drives the `m:ss` badge on ▶
    /// thumbs; ignored for other kinds
    pub dur_s: Option<u64>,
    /// live share link — drawn as the ↗ chip; set by the host when the
    /// shot already has one (rehydrate, or Share ran before the fly-in)
    pub shared: Option<crate::share::ShareEntry>,
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
        land(self, el, l)
    }

    /// Window events (redraw, cursor-left). Returns true if consumed.
    pub fn window_event(
        &mut self,
        id: winit::window::WindowId,
        ev: &winit::event::WindowEvent,
    ) -> bool {
        self.wins
            .iter_mut()
            .any(|w| w.id() == id && w.window_event(ev))
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

    /// Mutable variant — the host flips `shared` on share/revoke.
    pub fn shot_mut(&mut self, id: winit::window::WindowId, idx: usize) -> Option<&mut Shot> {
        self.wins
            .iter_mut()
            .find(|w| w.id() == id)?
            .stack_mut()
            .shot_mut(idx)
    }

    /// Repaint one stack window (badge/chip state changed).
    pub fn redraw(&mut self, id: winit::window::WindowId) {
        if let Some(w) = self.wins.iter_mut().find(|w| w.id() == id) {
            w.redraw();
        }
    }

    pub fn remove(&mut self, id: winit::window::WindowId, idx: usize) {
        if let Some(w) = self.wins.iter_mut().find(|w| w.id() == id) {
            // a removal reshuffles the landing slot — the fly layer is
            // pointing at geometry that no longer exists
            #[cfg(target_os = "macos")]
            w.cancel_native();
            w.stack_mut().remove(idx);
            w.redraw();
        }
    }

    /// Dismiss the whole stack on window `id` (clear-all). The caller
    /// sweeps afterwards so the emptied window goes away.
    pub fn clear_all(&mut self, id: winit::window::WindowId) {
        if let Some(w) = self.wins.iter_mut().find(|w| w.id() == id) {
            #[cfg(target_os = "macos")]
            w.cancel_native();
            w.stack_mut().clear_all();
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

/* ==================== entry point (shared by every backend) ========== */

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

/// Positions/name don't make a stable cross-platform id — the key only
/// needs to tell one monitor from another within a session.
fn mon_key(m: &MonitorHandle) -> String {
    format!(
        "{:?}@{:?}x{:?}s{:.3}",
        m.position(),
        m.size().width,
        m.size().height,
        m.scale_factor()
    )
}

/// `Chrome::land` — the shared entry. Err hands the `Land` back intact
/// so the host can fall back to the quick-access card.
fn land(chrome: &mut Chrome, el: &ActiveEventLoop, l: Land) -> Result<(), Land> {
    let Some(mon) = monitor_for(el, l.sel) else {
        return Err(l);
    };
    let key = mon_key(&mon);
    if chrome.wins.iter().all(|w| w.mon_key() != key) {
        let font = Rc::new(match crate::settings::system_font() {
            Ok(f) => f,
            Err(_) => return Err(l),
        });
        let corner = Corner::from_popup_pos(chrome.pos());
        let w = match ChromeWin::new(el, &mon, corner, font, chrome.accent()) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("chrome win: {e:#}");
                return Err(l);
            }
        };
        // the sink binds the window id, which only exists after creation
        w.set_sink((chrome.sink)(w.id()));
        chrome.wins.push(w);
    }
    let w = chrome.wins.iter_mut().find(|w| w.mon_key() == key).unwrap();
    w.land(l);
    Ok(())
}

/// `--chrome` demo: a bare event loop hosting one stack on the primary
/// monitor, landing synthetic shots so the layer can be verified by hand.
pub fn standalone(pos: &str, accent: u32) -> anyhow::Result<()> {
    use winit::application::ApplicationHandler;
    use winit::event_loop::{ControlFlow, EventLoop, EventLoopProxy};
    use winit::window::WindowId;

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
            dur_s: None,
            shared: None,
        }
    }

    struct Demo {
        chrome: Chrome,
        drops: usize,
        next_drop: Instant,
    }
    impl Demo {
        fn drop(&mut self, el: &ActiveEventLoop) {
            let n = self.drops;
            if self.chrome.land(el, shot(n)).is_ok() {
                self.drops += 1;
            }
            self.next_drop = Instant::now() + Duration::from_secs(1);
        }
    }
    impl ApplicationHandler<Ev> for Demo {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            self.drop(el);
        }
        fn window_event(
            &mut self,
            _el: &ActiveEventLoop,
            id: WindowId,
            ev: winit::event::WindowEvent,
        ) {
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
                    ChromeCmd::ClearAll => {
                        self.chrome.clear_all(wid);
                        self.chrome.sweep();
                    }
                    ChromeCmd::MenuAction { idx, id } => eprintln!("menu: thumb {idx} → {id}"),
                    ChromeCmd::DragBegin { idx, .. } => eprintln!("drag: thumb {idx}"),
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

        fn exiting(&mut self, _el: &ActiveEventLoop) {
            // Wl::drop marshals destroys over winit's wl_display —
            // only safe before the loop disconnects it.
            self.chrome.wins.clear();
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn shot_img(w: u32, h: u32, rgb: [u8; 3]) -> RgbaImage {
        // gradient blocks so thumbnails read as distinct captures
        RgbaImage::from_fn(w, h, |x, y| {
            let t = ((x * 255 / w) as u8).wrapping_add((y * 255 / h) as u8);
            Rgba([
                rgb[0].saturating_add(t / 3),
                rgb[1],
                rgb[2].saturating_sub(t / 4),
                255,
            ])
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
                FRect {
                    x: 100.0,
                    y: 100.0,
                    w: 400.0,
                    h: 300.0,
                },
                true,
                2.0,
                None,
                None,
            );
        }
        let (lw, lh) = (1440.0, 900.0);
        let mut buf = vec![0u32; (lw * 2.0) as usize * (lh * 2.0) as usize];
        {
            let mut cv = Canvas {
                buf: &mut buf,
                w: 2880,
                h: 1800,
                scale: 2.0,
            };
            let font = crate::settings::system_font().expect("font");
            st.render(&mut cv, &font, 0x52C7EA, Instant::now());
        }
        // premul ABGR-packed → plain RGBA8 for the PNG
        let img = RgbaImage::from_fn(2880, 1800, |x, y| {
            let p = buf[(y as usize) * 2880 + x as usize];
            let a = ((p >> 24) & 0xFF) as u32;
            let unprem = |c: u32| -> u8 {
                if a == 0 {
                    0
                } else {
                    ((c * 255 / a).min(255)) as u8
                }
            };
            Rgba([
                unprem(p & 0xFF),
                unprem((p >> 8) & 0xFF),
                unprem((p >> 16) & 0xFF),
                a as u8,
            ])
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
        let mut cv = Canvas {
            buf: &mut buf,
            w: 2880,
            h: 1800,
            scale: 2.0,
        };
        let font = crate::settings::system_font().expect("font");
        st.render(&mut cv, &font, 0x52C7EA, Instant::now());
        let img = RgbaImage::from_fn(2880, 1800, |x, y| {
            let p = buf[(y as usize) * 2880 + x as usize];
            let a = ((p >> 24) & 0xFF) as u32;
            let unprem = |c: u32| -> u8 {
                if a == 0 {
                    0
                } else {
                    ((c * 255 / a).min(255)) as u8
                }
            };
            Rgba([
                unprem(p & 0xFF),
                unprem((p >> 8) & 0xFF),
                unprem((p >> 16) & 0xFF),
                a as u8,
            ])
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
                FRect {
                    x: 100.0,
                    y: 100.0,
                    w: 400.0,
                    h: 300.0,
                },
                true,
                2.0,
                None,
                None,
            );
        }
        // wind the idle clock back past the threshold
        st.idle_since = Instant::now() - Duration::from_secs(10);
        st.settle(Instant::now());
        assert!(
            st.hide_anim,
            "idle past the deadline must start the hide tween"
        );
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

        // mid-hide frame for eyeballing the slide path
        st.hide_t = 0.5;
        let mut buf = vec![0u32; 2880 * 1800];
        let mut cv = Canvas {
            buf: &mut buf,
            w: 2880,
            h: 1800,
            scale: 2.0,
        };
        let font = crate::settings::system_font().expect("font");
        st.render(&mut cv, &font, 0x52C7EA, Instant::now());
        let img = RgbaImage::from_fn(2880, 1800, |x, y| {
            let p = buf[(y as usize) * 2880 + x as usize];
            let a = ((p >> 24) & 0xFF) as u32;
            let unprem = |c: u32| -> u8 {
                if a == 0 {
                    0
                } else {
                    ((c * 255 / a).min(255)) as u8
                }
            };
            Rgba([
                unprem(p & 0xFF),
                unprem((p >> 8) & 0xFF),
                unprem((p >> 16) & 0xFF),
                a as u8,
            ])
        });
        img.save("/tmp/qs-chrome-midhide.png").unwrap();
    }

    /// A thumb parked fully past the RIGHT edge must not wrap into the
    /// next buffer row — an unchecked `y*w + x` used to re-emit it on
    /// the LEFT edge.
    #[test]
    fn hidden_thumbs_dont_wrap_to_left_edge() {
        let mut st = Stack::new(Corner::BR);
        st.land(
            shot_img(1200, 800, [90, 60, 40]),
            ShotKind::Saved,
            None,
            FRect {
                x: 100.0,
                y: 100.0,
                w: 400.0,
                h: 300.0,
            },
            true,
            2.0,
            None,
            None,
        );
        st.hide_t = 1.0; // fully slid off the right edge
        let (bw, bh) = (2880usize, 1800usize);
        let mut buf = vec![0u32; bw * bh];
        {
            let mut cv = Canvas {
                buf: &mut buf,
                w: bw,
                h: bh,
                scale: 2.0,
            };
            let font = crate::settings::system_font().expect("font");
            st.render(&mut cv, &font, 0x52C7EA, Instant::now());
        }
        // nothing but the docked chip may be opaque — and it lives at
        // the right edge, so the left half must be empty
        for y in 0..bh {
            for x in 0..bw / 2 {
                assert_eq!(
                    buf[y * bw + x],
                    0,
                    "hidden thumb wrapped to left edge at phys ({x},{y})"
                );
            }
        }
    }

    /// The retention zone covers BOTH fan endpoints — a cursor resting
    /// inside the cascade's overhang keeps `inside` after the fan opens
    /// and slides the thumb away. Without it the hover flapped
    /// open/closed every tween (the reported bottom-edge jitter).
    #[test]
    fn hover_survives_fan_sweep() {
        let mut st = Stack::new(Corner::TR);
        for i in 0..3u8 {
            st.land(
                shot_img(1200, 800, [40 + i * 40, 60, 90]),
                ShotKind::Saved,
                None,
                FRect {
                    x: 100.0,
                    y: 100.0,
                    w: 400.0,
                    h: 300.0,
                },
                true,
                2.0,
                None,
                None,
            );
        }
        // cursor parked inside the collapsed cascade's low edge — a spot
        // the fanned row vacates (cascade dy collapses on open)
        let r2 = st.thumb_rect_at(2, 1440.0, 900.0, 0.0, 0.0);
        let (cx, cy) = (r2.x + r2.w / 2.0, r2.y + r2.h + 10.0); // just below
        let _ = st.pointer(ChromeEv::Move { x: cx, y: cy }, 1440.0, 900.0);
        assert!(
            st.inside,
            "cursor on the cascade edge should engage the zone"
        );
        // run the fan open to its end — the cascade's dy is gone now
        st.settle(Instant::now() + Duration::from_millis(400));
        assert_eq!(st.fan_t, 1.0);
        // same resting cursor: still inside via the union, no flap
        let _ = st.pointer(ChromeEv::Move { x: cx, y: cy }, 1440.0, 900.0);
        assert!(st.inside, "fan sweep must not drop the hover zone");
        assert_eq!(st.fan_to, 1.0, "fan must stay open under a resting cursor");
    }

    /// Click thumb0 → lightbox opens; backdrop click closes it.
    #[test]
    fn lightbox_opens_on_thumb_click() {
        let mut st = Stack::new(Corner::TR);
        st.land(
            shot_img(1200, 800, [40, 60, 90]),
            ShotKind::Saved,
            None,
            FRect {
                x: 100.0,
                y: 100.0,
                w: 400.0,
                h: 300.0,
            },
            true,
            2.0,
            None,
            None,
        );
        let r = st.thumb_rect(0, 1440.0, 900.0);
        // a click resolves on release — press alone must not open the
        // lightbox, or drag-out could never preempt it
        let cmd = st.pointer(
            ChromeEv::Down {
                x: r.x + r.w / 2.0,
                y: r.y + r.h / 2.0,
            },
            1440.0,
            900.0,
        );
        assert!(matches!(cmd, Some(ChromeCmd::Redraw)));
        assert!(st.lightbox.is_none());
        st.pointer(
            ChromeEv::Up {
                x: r.x + r.w / 2.0,
                y: r.y + r.h / 2.0,
            },
            1440.0,
            900.0,
        );
        assert!(st.lightbox.is_some());
        st.pointer(ChromeEv::Down { x: 700.0, y: 450.0 }, 1440.0, 900.0);
        assert!(st.lightbox.is_none());
    }

    /// O / S in the lightbox route to the same actions as the bar buttons.
    #[test]
    fn lightbox_keys_open_and_share() {
        let mut st = Stack::new(Corner::TR);
        st.land(
            shot_img(1200, 800, [40, 60, 90]),
            ShotKind::Saved,
            None,
            FRect {
                x: 100.0,
                y: 100.0,
                w: 400.0,
                h: 300.0,
            },
            true,
            2.0,
            None,
            None,
        );
        let r = st.thumb_rect(0, 1440.0, 900.0);
        st.pointer(
            ChromeEv::Down {
                x: r.x + r.w / 2.0,
                y: r.y + r.h / 2.0,
            },
            1440.0,
            900.0,
        );
        st.pointer(
            ChromeEv::Up {
                x: r.x + r.w / 2.0,
                y: r.y + r.h / 2.0,
            },
            1440.0,
            900.0,
        );
        assert!(st.lightbox.is_some());
        assert!(matches!(
            st.key(Key::Open),
            Some(ChromeCmd::MenuAction {
                idx: 0,
                id: MENU_OPEN
            })
        ));
        assert!(matches!(
            st.key(Key::Share),
            Some(ChromeCmd::MenuAction {
                idx: 0,
                id: MENU_SHARE
            })
        ));
        // outside a preview, O/S have no shot to act on
        st.lightbox = None;
        assert!(st.key(Key::Open).is_none());
    }

    /// A thumb press that drifts ≥4px before release becomes a drag —
    /// `DragBegin` fires and no lightbox opens; a sub-threshold wobble
    /// stays a click.
    #[test]
    fn thumb_drag_emits_drag_begin_not_lightbox() {
        let mut st = Stack::new(Corner::TR);
        st.land(
            shot_img(1200, 800, [40, 60, 90]),
            ShotKind::Saved,
            None,
            FRect {
                x: 100.0,
                y: 100.0,
                w: 400.0,
                h: 300.0,
            },
            true,
            2.0,
            None,
            None,
        );
        let r = st.thumb_rect(0, 1440.0, 900.0);
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        st.pointer(ChromeEv::Down { x: cx, y: cy }, 1440.0, 900.0);
        let cmd = st.pointer(
            ChromeEv::Dragged {
                x: cx + 12.0,
                y: cy,
            },
            1440.0,
            900.0,
        );
        assert!(matches!(cmd, Some(ChromeCmd::DragBegin { idx: 0 })));
        assert!(st.lightbox.is_none());
        // release must not resurrect the click
        st.pointer(
            ChromeEv::Up {
                x: cx + 12.0,
                y: cy,
            },
            1440.0,
            900.0,
        );
        assert!(st.lightbox.is_none());
        // a sub-threshold wobble stays a click
        st.pointer(ChromeEv::Down { x: cx, y: cy }, 1440.0, 900.0);
        st.pointer(ChromeEv::Dragged { x: cx + 2.0, y: cy }, 1440.0, 900.0);
        st.pointer(ChromeEv::Up { x: cx + 2.0, y: cy }, 1440.0, 900.0);
        assert!(st.lightbox.is_some());
    }

    /// The "CLEAR ALL" pill rides the far end of the fanned row: it only
    /// exists once the fan is open with ≥2 shots, clicking it emits
    /// ChromeCmd::ClearAll, and clear_all() empties every bit of state.
    #[test]
    fn clear_all_pill_empties_stack() {
        let (lw, lh) = (1440.0, 900.0);
        let mut st = Stack::new(Corner::TR);
        for i in 0..3u8 {
            st.land(
                shot_img(1200, 800, [40 + i * 40, 60, 90]),
                ShotKind::Saved,
                None,
                FRect {
                    x: 100.0,
                    y: 100.0,
                    w: 400.0,
                    h: 300.0,
                },
                true,
                2.0,
                None,
                None,
            );
        }
        // no pill while the stack sits collapsed
        assert!(st.clear_rect(lw, lh).is_none());
        // hover → fan open to the end
        let r0 = st.thumb_rect_at(0, lw, lh, 0.0, 0.0);
        let _ = st.pointer(
            ChromeEv::Move {
                x: r0.x + r0.w / 2.0,
                y: r0.y + r0.h / 2.0,
            },
            lw,
            lh,
        );
        st.settle(Instant::now() + Duration::from_millis(400));
        assert_eq!(st.fan_t, 1.0);

        // the pill sits centred under the fanned row
        let cr = st.clear_rect(lw, lh).expect("pill exists when fanned");
        let row = st.thumb_rect_at(0, lw, lh, 1.0, 0.0);
        assert!(cr.y >= row.y + row.h, "pill must sit below the row");
        assert!(cr.x + cr.w / 2.0 > row.x - 400.0 && cr.x + cr.w / 2.0 < row.x + row.w + 1.0);
        let cmd = st.pointer(
            ChromeEv::Down {
                x: cr.x + cr.w / 2.0,
                y: cr.y + cr.h / 2.0,
            },
            lw,
            lh,
        );
        assert!(matches!(cmd, Some(ChromeCmd::ClearAll)));

        // clearing empties everything — shots, hover, lightbox, pinned
        st.clear_all();
        assert!(st.is_empty());
        assert!(st.shots.is_empty());
        assert!(st.clear_rect(lw, lh).is_none());
        assert!(st.input_rects(lw, lh).is_empty());

        // also clears a lightbox/pinned state mid-flight
        let mut st = Stack::new(Corner::TR);
        for _ in 0..2 {
            st.land(
                shot_img(1200, 800, [40, 60, 90]),
                ShotKind::Saved,
                None,
                FRect {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                },
                true,
                2.0,
                None,
                None,
            );
        }
        st.pinned = true;
        st.lightbox = Some(0);
        st.clear_all();
        assert!(st.is_empty());
        assert!(st.lightbox.is_none());
        assert!(!st.pinned);
    }

    /// Right-click opens the in-chrome menu; a row click emits the row's
    /// MenuAction; Esc/backdrop dismiss without acting.
    #[test]
    fn context_menu_lifecycle() {
        let (lw, lh) = (1440.0, 900.0);
        let mut st = Stack::new(Corner::TR);
        for i in 0..2u8 {
            st.land(
                shot_img(1200, 800, [40 + i * 40, 60, 90]),
                ShotKind::Saved,
                Some(PathBuf::from(format!("/tmp/shot{i}.png"))),
                FRect {
                    x: 100.0,
                    y: 100.0,
                    w: 400.0,
                    h: 300.0,
                },
                true,
                2.0,
                None,
                None,
            );
        }
        let r = st.thumb_rect(0, lw, lh);
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);

        // right-click on the newest thumb opens the menu — and widens
        // the input region to the whole screen for the dismiss-outside
        st.pointer(ChromeEv::RightDown { x: cx, y: cy }, lw, lh);
        assert!(st.menu.is_some());
        assert_eq!(
            st.zone_rects(lw, lh).len(),
            1,
            "menu should cover the screen"
        );
        assert!(matches!(st.hit(5.0, 5.0, lw, lh), Some(Hit::Backdrop)));

        // a row click emits the row's action and closes the menu
        let m = st.menu.as_ref().unwrap();
        let copy_row = m.rows.iter().position(|(id, _)| *id == MENU_COPY).unwrap();
        let rr = st.menu_row_rect(copy_row, lw, lh).unwrap();
        let cmd = st.pointer(
            ChromeEv::Down {
                x: rr.x + rr.w / 2.0,
                y: rr.y + rr.h / 2.0,
            },
            lw,
            lh,
        );
        assert!(
            matches!(
                cmd,
                Some(ChromeCmd::MenuAction {
                    idx: 0,
                    id: MENU_COPY
                })
            ),
            "row click must emit MenuAction, got {cmd:?}"
        );
        assert!(st.menu.is_none());

        // reopen, then Esc peels the menu off without touching the stack
        st.pointer(ChromeEv::RightDown { x: cx, y: cy }, lw, lh);
        assert!(st.menu.is_some());
        let cmd = st.key(Key::Esc);
        assert!(matches!(cmd, Some(ChromeCmd::Redraw)));
        assert!(st.menu.is_none());
        assert_eq!(st.shots.len(), 2);

        // a disabled row never emits — Reveal needs a saved path
        let mut st = Stack::new(Corner::TR);
        st.land(
            shot_img(1200, 800, [40, 60, 90]),
            ShotKind::Copied,
            None,
            FRect {
                x: 100.0,
                y: 100.0,
                w: 400.0,
                h: 300.0,
            },
            true,
            2.0,
            None,
            None,
        );
        let r = st.thumb_rect(0, lw, lh);
        st.pointer(
            ChromeEv::RightDown {
                x: r.x + r.w / 2.0,
                y: r.y + r.h / 2.0,
            },
            lw,
            lh,
        );
        let m = st.menu.as_ref().unwrap();
        let reveal_row = m
            .rows
            .iter()
            .position(|(id, _)| *id == MENU_REVEAL)
            .unwrap();
        assert!(
            !m.rows[reveal_row].1,
            "Reveal must be disabled without a path"
        );
        let rr = st.menu_row_rect(reveal_row, lw, lh).unwrap();
        let cmd = st.pointer(
            ChromeEv::Down {
                x: rr.x + rr.w / 2.0,
                y: rr.y + rr.h / 2.0,
            },
            lw,
            lh,
        );
        assert!(
            matches!(cmd, Some(ChromeCmd::Redraw)),
            "disabled row must not act"
        );
        assert!(st.menu.is_some(), "disabled row leaves the menu open");
    }
}
