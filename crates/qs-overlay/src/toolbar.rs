//! Selection toolbar: circular action buttons docked to whichever selection
//! edge has room — bottom -> top -> right -> left, falling back inside the
//! selection's bottom-right corner. Rendered straight into the softbuffer
//! surface via SDF primitives at physical resolution (crisp on HiDPI).

use qs_core::{Point, Rect};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Save,
    Copy,
    Ocr,
}

const R: f64 = 14.0; // button radius, logical px
const GAP: f64 = 8.0; // between buttons
const PAD: f64 = 6.0; // toolbar edge padding
const EDGE_GAP: f64 = 4.0; // between selection edge and toolbar
const STROKE: f64 = 1.8; // icon stroke width, logical px

const BG: u32 = 0x2C2C2E;
const BG_HOVER: u32 = 0x48484D;
const FG: u32 = 0xFFFFFF;
const SHADOW: u32 = 0x000000;

pub struct Toolbar {
    /// bounding box in virtual logical coords
    pub rect: Rect,
    /// button hit rects in logical coords
    pub buttons: [(Action, Rect); 3],
    pub hover: Option<usize>,
}

impl Toolbar {
    /// `mr` = logical rect of the monitor holding the selection.
    pub fn new(sel: Rect, mr: Rect) -> Self {
        let d = R * 2.0;
        let n = 3.0;
        let (hw, hh) = (PAD * 2.0 + n * d + (n - 1.0) * GAP, PAD * 2.0 + d); // horizontal
        let (vw, vh) = (PAD * 2.0 + d, PAD * 2.0 + n * d + (n - 1.0) * GAP); // vertical
        let (sx0, sy0) = (sel.x as f64, sel.y as f64);
        let (sx1, sy1) = (sel.right() as f64, sel.bottom() as f64);
        let (mx0, my0) = (mr.x as f64, mr.y as f64);
        let (mx1, my1) = (mr.right() as f64, mr.bottom() as f64);
        let clamp =
            |v: f64, lo: f64, hi: f64| if lo <= hi { v.clamp(lo, hi) } else { hi };

        // (x, y, vertical, fits?)
        let candidates = [
            (
                clamp(sx1 - hw, mx0 + 2.0, mx1 - hw - 2.0),
                sy1 + EDGE_GAP,
                false,
                sy1 + EDGE_GAP + hh <= my1,
            ),
            (
                clamp(sx1 - hw, mx0 + 2.0, mx1 - hw - 2.0),
                sy0 - EDGE_GAP - hh,
                false,
                sy0 - EDGE_GAP - hh >= my0,
            ),
            (
                sx1 + EDGE_GAP,
                clamp(sy1 - vh, my0 + 2.0, my1 - vh - 2.0),
                true,
                sx1 + EDGE_GAP + vw <= mx1,
            ),
            (
                sx0 - EDGE_GAP - vw,
                clamp(sy1 - vh, my0 + 2.0, my1 - vh - 2.0),
                true,
                sx0 - EDGE_GAP - vw >= mx0,
            ),
            // inside bottom-right of the selection — always the fallback
            (
                clamp(sx1 - hw - EDGE_GAP, mx0 + 2.0, mx1 - hw - 2.0),
                clamp(sy1 - hh - EDGE_GAP, my0 + 2.0, my1 - hh - 2.0),
                false,
                true,
            ),
        ];

        let (x, y, vertical, _) = candidates
            .iter()
            .find(|c| c.3)
            .copied()
            .unwrap();
        let rect = Rect::new(
            x.round() as i32,
            y.round() as i32,
            (if vertical { vw } else { hw }).round() as u32,
            (if vertical { vh } else { hh }).round() as u32,
        );
        let mut buttons = [
            (Action::Save, Rect::default()),
            (Action::Copy, Rect::default()),
            (Action::Ocr, Rect::default()),
        ];
        for (i, b) in buttons.iter_mut().enumerate() {
            let off = PAD + i as f64 * (d + GAP);
            b.1 = if vertical {
                Rect::new(
                    rect.x + PAD as i32,
                    rect.y + off.round() as i32,
                    d as u32,
                    d as u32,
                )
            } else {
                Rect::new(
                    rect.x + off.round() as i32,
                    rect.y + PAD as i32,
                    d as u32,
                    d as u32,
                )
            };
        }
        Self {
            rect,
            buttons,
            hover: None,
        }
    }

    /// Index of the button under `p`, or `None`.
    pub fn hit(&self, p: Point) -> Option<usize> {
        self.buttons.iter().position(|(_, r)| {
            p.x >= r.x && p.x < r.right() && p.y >= r.y && p.y < r.bottom()
        })
    }

    /// `p` inside the toolbar box (incl. padding) — clicks there should not
    /// start a new selection.
    pub fn contains(&self, p: Point) -> bool {
        let r = &self.rect;
        p.x >= r.x && p.x < r.right() && p.y >= r.y && p.y < r.bottom()
    }

    /// Paint buttons into the physical surface. `vrect` = this window's monitor
    /// rect in virtual coords; parts outside this monitor clip naturally.
    pub fn paint(&self, buf: &mut [u32], sw: usize, sh: usize, vrect: Rect, scale: f64) {
        for (i, (action, r)) in self.buttons.iter().enumerate() {
            let cx = (r.x as f64 + r.w as f64 / 2.0 - vrect.x as f64) * scale;
            let cy = (r.y as f64 + r.h as f64 / 2.0 - vrect.y as f64) * scale;
            paint_button(buf, sw, sh, cx, cy, R * scale, scale, self.hover == Some(i), *action);
        }
    }
}

#[inline]
fn blend(buf: &mut [u32], sw: usize, sh: usize, x: i32, y: i32, rgb: u32, a: f64) {
    if x < 0 || y < 0 || x >= sw as i32 || y >= sh as i32 || a <= 0.0 {
        return;
    }
    let i = y as usize * sw + x as usize;
    let d = buf[i];
    let mix = |s: u8, d: u8| (s as f64 * a + d as f64 * (1.0 - a)).round() as u32;
    buf[i] = mix((rgb >> 16) as u8, (d >> 16) as u8) << 16
        | mix((rgb >> 8) as u8, (d >> 8) as u8) << 8
        | mix(rgb as u8, d as u8);
}

#[inline]
fn seg_sdf(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    ((px - ax - dx * t).powi(2) + (py - ay - dy * t).powi(2)).sqrt()
}

/// Signed distance to a rounded-rect boundary (negative inside).
#[inline]
fn rrect_sdf(px: f64, py: f64, cx: f64, cy: f64, hw: f64, hh: f64, r: f64) -> f64 {
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    let (ox, oy) = (qx.max(0.0), qy.max(0.0));
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

/// Coverage of a stroke of logical width `w` centered on `sdf` (logical dist).
#[inline]
fn stroke_cov(sdf_l: f64, w: f64, scale: f64) -> f64 {
    (w / 2.0 + 0.5 - sdf_l.abs() * scale).clamp(0.0, 1.0)
}

/// Coverage of a fill of the region with `sdf <= 0` (logical dist).
#[inline]
fn fill_cov(sdf_l: f64, scale: f64) -> f64 {
    (0.5 - sdf_l * scale).clamp(0.0, 1.0)
}

fn paint_button(
    buf: &mut [u32],
    sw: usize,
    sh: usize,
    cx: f64,
    cy: f64,
    rad: f64,
    scale: f64,
    hovered: bool,
    action: Action,
) {
    // bbox covers button + soft shadow falloff
    let pad = (STROKE * scale).ceil() as i32 + (4.0 * scale).ceil() as i32 + 2;
    let r = rad.ceil() as i32 + pad;
    let bg = if hovered { BG_HOVER } else { BG };
    for y in (cy as i32 - r)..=(cy as i32 + r) {
        for x in (cx as i32 - r)..=(cx as i32 + r) {
            let px = x as f64 + 0.5 - cx;
            let py = y as f64 + 0.5 - cy;
            // soft shadow: disc shifted down, alpha falls off over ~4 phys px
            let sd = px.hypot(py - 1.5 * scale) - rad;
            if sd > 0.0 {
                let a = 0.24 * (1.0 - sd / (4.0 * scale)).clamp(0.0, 1.0);
                if a > 0.0 {
                    blend(buf, sw, sh, x, y, SHADOW, a);
                }
            }
            // disc + hairline rim
            let d = px.hypot(py) - rad;
            let a = (0.5 - d).clamp(0.0, 1.0);
            if a > 0.0 {
                blend(buf, sw, sh, x, y, bg, a * 0.9);
                let rim = (0.5 - d.abs()).clamp(0.0, 1.0);
                if rim > 0.0 {
                    blend(buf, sw, sh, x, y, FG, rim * 0.12);
                }
            }
            // icon — work in logical units, sdf * scale -> physical coverage
            let (lx, ly) = (px / scale, py / scale);
            match action {
                Action::Copy => {
                    let d_back = rrect_sdf(lx, ly, -3.5, -3.5, 4.5, 4.5, 1.4);
                    let d_front = rrect_sdf(lx, ly, 3.5, 3.5, 4.5, 4.5, 1.4);
                    let fill = fill_cov(d_front, scale);
                    if fill > 0.0 {
                        blend(buf, sw, sh, x, y, bg, fill); // occlude back square
                    }
                    let a = stroke_cov(d_back, STROKE, scale)
                        .max(stroke_cov(d_front, STROKE, scale));
                    if a > 0.0 {
                        blend(buf, sw, sh, x, y, FG, a);
                    }
                }
                Action::Ocr => {
                    // paragraph: left-aligned lines, last one shorter
                    let d = seg_sdf(lx, ly, -5.75, -4.5, 4.5, -4.5)
                        .min(seg_sdf(lx, ly, -5.75, 0.0, 5.75, 0.0))
                        .min(seg_sdf(lx, ly, -5.75, 4.5, 1.5, 4.5));
                    let a = stroke_cov(d, STROKE, scale);
                    if a > 0.0 {
                        blend(buf, sw, sh, x, y, FG, a);
                    }
                }
                Action::Save => {
                    // tray: U shape open at top; arrow dips into it
                    let d = seg_sdf(lx, ly, -5.75, -0.5, -5.75, 5.5)
                        .min(seg_sdf(lx, ly, -5.75, 5.5, 5.75, 5.5))
                        .min(seg_sdf(lx, ly, 5.75, 5.5, 5.75, -0.5))
                        .min(seg_sdf(lx, ly, 0.0, -6.5, 0.0, 1.5))
                        .min(seg_sdf(lx, ly, -3.2, -1.5, 0.0, 1.5))
                        .min(seg_sdf(lx, ly, 3.2, -1.5, 0.0, 1.5));
                    let a = stroke_cov(d, STROKE, scale);
                    if a > 0.0 {
                        blend(buf, sw, sh, x, y, FG, a);
                    }
                }
            }
        }
    }
}
