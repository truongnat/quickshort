//! Flameshot-style annotations: vector shapes kept in virtual logical coords.
//! Committed strokes rasterize once into an ARGB overlay layer (sized like the
//! virtual canvas); the output path re-rasterizes the same list onto the
//! stitched image — `stitch` normalizes to scale 1.0 so coords map 1:1.

use crate::sdf::{ellipse_sdf, rrect_sdf, seg_sdf, stroke_cov, Canvas};
use ab_glyph::{point, Font, FontVec, ScaleFont, VariableFont};
use image::RgbaImage;
use qs_core::{Point, Rect};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Palette for drawing tools (system colors).
pub const PALETTE: [u32; 8] = [
    0xFF453A, // red
    0xFF9F0A, // orange
    0xFFD60A, // yellow
    0x32D74B, // green
    0x0A84FF, // blue
    0xBF5AF2, // purple
    0xFFFFFF, // white
    0x1C1C1E, // black
];

/// Stroke widths per size level (S/M/L) — pen & shapes; marker is wider.
pub const SIZES: [f64; 3] = [2.0, 3.6, 6.0];
pub const MARKER_SIZES: [f64; 3] = [6.0, 9.5, 14.0];
const MARKER_A: f64 = 0.42;
/// Font size presets for the text tool — (logical px, toolbar label).
/// Picked as a number, independent from the stroke S/M/L levels.
pub const TEXT_SIZES: [(f64, &str); 6] = [
    (12.0, "12"),
    (14.0, "14"),
    (18.0, "18"),
    (24.0, "24"),
    (32.0, "32"),
    (48.0, "48"),
];
/// Index into `TEXT_SIZES` armed by default — 14 px.
pub const TEXT_SIZE_DEFAULT: usize = 1;
pub const PIX_BLOCK: i32 = 8; // pixelate block, logical px
const ACCENT: u32 = 0x0A84FF; // pixelate drag-preview outline

#[derive(Clone)]
pub enum Annotation {
    Pen {
        pts: Vec<Point>,
        w: f64,
        rgb: u32,
    },
    Marker {
        pts: Vec<Point>,
        w: f64,
        rgb: u32,
    },
    Arrow {
        a: Point,
        b: Point,
        w: f64,
        rgb: u32,
    },
    Rect {
        r: Rect,
        w: f64,
        rgb: u32,
    },
    Ellipse {
        r: Rect,
        w: f64,
        rgb: u32,
    },
    /// Destructive pixel edit — kept in the list so output ordering survives.
    Pixelate(Rect),
    /// Placed text — `pos` is the top-left of the line (logical coords).
    Text {
        pos: Point,
        text: String,
        /// font size, logical px
        px: f64,
        bold: bool,
        rgb: u32,
    },
}

impl Annotation {
    /// Stroke params (width, rgb, alpha); `None` for pixel edits.
    fn params(&self) -> Option<(f64, u32, f64)> {
        Some(match self {
            Self::Pen { w, rgb, .. } => (*w, *rgb, 1.0),
            Self::Marker { w, rgb, .. } => (*w, *rgb, MARKER_A),
            Self::Arrow { w, rgb, .. }
            | Self::Rect { w, rgb, .. }
            | Self::Ellipse { w, rgb, .. } => (*w, *rgb, 1.0),
            Self::Pixelate(_) | Self::Text { .. } => return None,
        })
    }

    /// Distance to the stroke centreline at logical point (lx, ly).
    fn dist(&self, lx: f64, ly: f64) -> f64 {
        match self {
            Self::Pen { pts, .. } | Self::Marker { pts, .. } => poly_dist(pts, lx, ly),
            Self::Arrow { a, b, w, .. } => arrow_dist(*a, *b, *w, lx, ly),
            Self::Rect { r, .. } => rrect_sdf(
                lx,
                ly,
                r.x as f64 + r.w as f64 / 2.0,
                r.y as f64 + r.h as f64 / 2.0,
                r.w as f64 / 2.0,
                r.h as f64 / 2.0,
                1.5,
            ),
            Self::Ellipse { r, .. } => ellipse_sdf(
                lx,
                ly,
                r.x as f64 + r.w as f64 / 2.0,
                r.y as f64 + r.h as f64 / 2.0,
                r.w as f64 / 2.0,
                r.h as f64 / 2.0,
            ),
            Self::Pixelate(_) | Self::Text { .. } => f64::INFINITY,
        }
    }

    /// Bounding rect (logical) inflated by the stroke pad.
    pub fn bbox(&self) -> Rect {
        let mut b = match self {
            Self::Pen { pts, .. } | Self::Marker { pts, .. } => {
                pts.iter()
                    .fold(Rect::new(pts[0].x, pts[0].y, 0, 0), |b, p| {
                        let x = b.x.min(p.x);
                        let y = b.y.min(p.y);
                        Rect::new(
                            x,
                            y,
                            (b.right().max(p.x) - x) as u32,
                            (b.bottom().max(p.y) - y) as u32,
                        )
                    })
            }
            Self::Arrow { a, b, .. } => Rect::from_drag(*a, *b),
            Self::Rect { r, .. } | Self::Ellipse { r, .. } | Self::Pixelate(r) => *r,
            Self::Text { pos, text, px, bold, .. } => {
                let (w, h) = text_extent(text, *px, *bold);
                Rect::new(pos.x, pos.y, w.ceil() as u32 + 2, h.ceil() as u32 + 2)
            }
        };
        let pad = 4 + self
            .params()
            .map(|(w, ..)| (w / 2.0).ceil() as i32)
            .unwrap_or(0);
        b.x -= pad;
        b.y -= pad;
        b.w += (pad * 2) as u32;
        b.h += (pad * 2) as u32;
        b
    }

    /// Rasterize into the logical-res ARGB layer. Cell (i,j) of the layer
    /// corresponds to logical position `off + (i,j)`.
    pub fn render_into(&self, layer: &mut ArgbLayer, off: Point) {
        let Some((w, rgb, alpha)) = self.params() else {
            return;
        };
        let bb = self.bbox();
        let x0 = (bb.x - off.x).max(0);
        let y0 = (bb.y - off.y).max(0);
        let x1 = (bb.right() - off.x).min(layer.w as i32);
        let y1 = (bb.bottom() - off.y).min(layer.h as i32);
        for cy in y0..y1 {
            for cx in x0..x1 {
                let d = self.dist(
                    off.x as f64 + cx as f64 + 0.5,
                    off.y as f64 + cy as f64 + 0.5,
                );
                let a = stroke_cov(d, w, 1.0) * alpha;
                if a > 0.0 {
                    layer.blend(cx, cy, rgb, a);
                }
            }
        }
    }

    /// Render at physical res into a blending target — used both for the
    /// in-progress preview and for committing strokes into a window's
    /// physical-size ARGB layer, so what you see while dragging is exactly
    /// what gets committed (no logical->physical upsample blur).
    pub fn render_phys(&self, t: &mut impl Blend, vrect: Rect, scale: f64) {
        if let Self::Pixelate(r) = self {
            // drag preview for pixelate: thin accent outline
            let x0 = ((r.x - vrect.x).max(0) as f64 * scale).round() as i32;
            let y0 = ((r.y - vrect.y).max(0) as f64 * scale).round() as i32;
            let x1 = ((r.right() - vrect.x) as f64 * scale).round() as i32;
            let y1 = ((r.bottom() - vrect.y) as f64 * scale).round() as i32;
            for x in x0..x1 {
                t.blend(x, y0, ACCENT, 0.9);
                t.blend(x, y1 - 1, ACCENT, 0.9);
            }
            for y in y0..y1 {
                t.blend(x0, y, ACCENT, 0.9);
                t.blend(x1 - 1, y, ACCENT, 0.9);
            }
            return;
        }
        if let Self::Text { pos, text, px, bold, rgb } = self {
            render_text(t, *pos, text, *px, *bold, *rgb, vrect, scale);
            return;
        }
        let Some((w, rgb, alpha)) = self.params() else {
            return;
        };
        let bb = self.bbox();
        let x0 = ((bb.x - vrect.x).max(0) as f64 * scale) as i32;
        let y0 = ((bb.y - vrect.y).max(0) as f64 * scale) as i32;
        let x1 = ((bb.right() - vrect.x) as f64 * scale) as i32;
        let y1 = ((bb.bottom() - vrect.y) as f64 * scale) as i32;
        for y in y0..y1 {
            for x in x0..x1 {
                let lx = vrect.x as f64 + (x as f64 + 0.5) / scale;
                let ly = vrect.y as f64 + (y as f64 + 0.5) / scale;
                let a = stroke_cov(self.dist(lx, ly), w, scale) * alpha;
                if a > 0.0 {
                    t.blend(x, y, rgb, a);
                }
            }
        }
    }
}

fn poly_dist(pts: &[Point], x: f64, y: f64) -> f64 {
    pts.windows(2).fold(f64::INFINITY, |d, s| {
        d.min(seg_sdf(
            x,
            y,
            s[0].x as f64,
            s[0].y as f64,
            s[1].x as f64,
            s[1].y as f64,
        ))
    })
}

/// Shaft + two wings rotated ±28° off the backward direction; head length
/// grows with stroke width.
fn arrow_dist(a: Point, b: Point, w: f64, x: f64, y: f64) -> f64 {
    let (ax, ay, bx, by) = (a.x as f64, a.y as f64, b.x as f64, b.y as f64);
    let mut d = seg_sdf(x, y, ax, ay, bx, by);
    let (dx, dy) = (bx - ax, by - ay);
    let len = dx.hypot(dy);
    if len > 1e-3 {
        let (ux, uy) = (-dx / len, -dy / len); // backward
        let hl = (6.0 + w * 2.0).min(len * 0.35);
        let (c, s) = (28f64.to_radians().cos(), 28f64.to_radians().sin());
        for t in [1.0, -1.0] {
            let wx = ux * c - uy * s * t;
            let wy = ux * s * t + uy * c;
            d = d.min(seg_sdf(x, y, bx, by, bx + wx * hl, by + wy * hl));
        }
    }
    d
}

/// ARGB overlay layer (logical res) that alpha-composites strokes over each
/// other; composited onto the opaque surface afterwards.
pub struct ArgbLayer<'a> {
    pub buf: &'a mut [u32], // 0xAARRGGBB
    pub w: usize,
    pub h: usize,
}

impl ArgbLayer<'_> {
    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, rgb: u32, sa: f64) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 || sa <= 0.0 {
            return;
        }
        let i = y as usize * self.w + x as usize;
        let d = self.buf[i];
        let da = ((d >> 24) & 0xFF) as f64 / 255.0;
        let oa = sa + da * (1.0 - sa);
        if oa <= 0.0 {
            return;
        }
        let mix = |s: u32, d: u32| ((s as f64 * sa + d as f64 * da * (1.0 - sa)) / oa) as u32;
        self.buf[i] = ((oa * 255.0) as u32) << 24
            | mix((rgb >> 16) & 0xFF, (d >> 16) & 0xFF) << 16
            | mix((rgb >> 8) & 0xFF, (d >> 8) & 0xFF) << 8
            | mix(rgb & 0xFF, d & 0xFF);
    }
}

/// src-over blending target — a softbuffer surface pixel or an ARGB layer
/// cell. Lets `Annotation::render_phys` serve both live preview and
/// physical-res layer commits with one code path.
pub trait Blend {
    fn blend(&mut self, x: i32, y: i32, rgb: u32, a: f64);
}

impl Blend for Canvas<'_> {
    fn blend(&mut self, x: i32, y: i32, rgb: u32, a: f64) {
        Canvas::blend(self, x, y, rgb, a);
    }
}

impl Blend for ArgbLayer<'_> {
    fn blend(&mut self, x: i32, y: i32, rgb: u32, a: f64) {
        ArgbLayer::blend(self, x, y, rgb, a);
    }
}

/// Union bbox of all annotations (logical coords).
pub fn annots_bbox(annots: &[Annotation]) -> Option<Rect> {
    let mut it = annots.iter().map(Annotation::bbox);
    let mut u = it.next()?;
    for b in it {
        let x = u.x.min(b.x);
        let y = u.y.min(b.y);
        u = Rect::new(
            x,
            y,
            (u.right().max(b.right()) - x) as u32,
            (u.bottom().max(b.bottom()) - y) as u32,
        );
    }
    Some(u)
}

/// Box-average pixelate `r` (canvas coords) of a 0xRRGGBB canvas; returns the
/// overwritten pixels so the op can be undone.
pub fn pixelate_canvas(canvas: &mut [u32], cw: usize, ch: usize, r: Rect, block: i32) -> Vec<u32> {
    let (x0, y0) = (r.x.max(0) as usize, r.y.max(0) as usize);
    let (x1, y1) = ((r.right() as usize).min(cw), (r.bottom() as usize).min(ch));
    let mut backup = Vec::with_capacity((x1 - x0) * (y1 - y0));
    for y in y0..y1 {
        backup.extend_from_slice(&canvas[y * cw + x0..y * cw + x1]);
    }
    let b = block as usize;
    for by in (y0..y1).step_by(b) {
        for bx in (x0..x1).step_by(b) {
            let (bw, bh) = (b.min(x1 - bx), b.min(y1 - by));
            let (mut sr, mut sg, mut sb, mut n) = (0u64, 0u64, 0u64, 0u64);
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let p = canvas[y * cw + x];
                    sr += ((p >> 16) & 0xFF) as u64;
                    sg += ((p >> 8) & 0xFF) as u64;
                    sb += (p & 0xFF) as u64;
                    n += 1;
                }
            }
            let avg = (((sr / n) as u32) << 16) | (((sg / n) as u32) << 8) | (sb / n) as u32;
            for y in by..by + bh {
                canvas[y * cw + bx..y * cw + bx + bw].fill(avg);
            }
        }
    }
    backup
}

pub fn restore_canvas(canvas: &mut [u32], cw: usize, ch: usize, r: Rect, backup: &[u32]) {
    let (x0, y0) = (r.x.max(0) as usize, r.y.max(0) as usize);
    let (x1, y1) = ((r.right() as usize).min(cw), (r.bottom() as usize).min(ch));
    let mut i = 0;
    for y in y0..y1 {
        let n = x1 - x0;
        if i + n <= backup.len() {
            canvas[y * cw + x0..y * cw + x1].copy_from_slice(&backup[i..i + n]);
        }
        i += n;
    }
}

/// Pixelate a rect (image coords) of the RGBA output image.
fn pixelate_rgba(img: &mut RgbaImage, r: Rect, block: i32) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let (x0, y0) = (r.x.max(0), r.y.max(0));
    let (x1, y1) = (r.right().min(w), r.bottom().min(h));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let stride = img.width() as usize * 4;
    let raw = img.as_mut();
    for by in (y0..y1).step_by(block as usize) {
        for bx in (x0..x1).step_by(block as usize) {
            let (bw, bh) = (block.min(x1 - bx), block.min(y1 - by));
            let (mut s, mut n) = ([0u64; 4], 0u64);
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let p = &raw[y as usize * stride + x as usize * 4..][..4];
                    for (k, s) in s.iter_mut().enumerate() {
                        *s += p[k] as u64;
                    }
                    n += 1;
                }
            }
            let avg = [s[0] / n, s[1] / n, s[2] / n, 255];
            for y in by..by + bh {
                for x in bx..bx + bw {
                    raw[y as usize * stride + x as usize * 4..][..4].copy_from_slice(&[
                        avg[0] as u8,
                        avg[1] as u8,
                        avg[2] as u8,
                        255,
                    ]);
                }
            }
        }
    }
}

/// Bake annotations into the stitched image. `annots` are in virtual logical
/// coords; the image covers `sel` at `sel * scale` physical res, so strokes
/// rasterize through `render_phys` at native density (no upsample blur).
/// Pixel edits apply in list order, vector strokes composite on top.
pub fn annotate_image(img: &mut RgbaImage, sel: Rect, annots: &[Annotation], scale: f64) {
    if annots.is_empty() {
        return;
    }
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut layer = vec![0u32; w * h];
    let flush = |layer: &[u32], img: &mut RgbaImage| {
        let raw = img.as_mut();
        for (i, &l) in layer.iter().enumerate() {
            let a = (l >> 24) & 0xFF;
            if a == 0 {
                continue;
            }
            let fa = a as f64 / 255.0;
            let p = &mut raw[i * 4..i * 4 + 4];
            p[0] = (((l >> 16) & 0xFF) as f64 * fa + p[0] as f64 * (1.0 - fa)).round() as u8;
            p[1] = (((l >> 8) & 0xFF) as f64 * fa + p[1] as f64 * (1.0 - fa)).round() as u8;
            p[2] = ((l & 0xFF) as f64 * fa + p[2] as f64 * (1.0 - fa)).round() as u8;
            p[3] = 255;
        }
    };
    for a in annots {
        match a {
            Annotation::Pixelate(r) => {
                flush(&layer, img);
                layer.fill(0);
                let img_rect = Rect::new(
                    ((r.x - sel.x) as f64 * scale).round() as i32,
                    ((r.y - sel.y) as f64 * scale).round() as i32,
                    (r.w as f64 * scale).round().max(1.0) as u32,
                    (r.h as f64 * scale).round().max(1.0) as u32,
                );
                pixelate_rgba(
                    img,
                    img_rect,
                    (PIX_BLOCK as f64 * scale).round().max(2.0) as i32,
                );
            }
            v => v.render_phys(
                &mut ArgbLayer {
                    buf: &mut layer,
                    w,
                    h,
                },
                sel,
                scale,
            ),
        }
    }
    flush(&layer, img);
}

/// Bundled Inter variable font (OFL — fonts/OFL.txt): `wght` 100–900 and
/// `opsz` live in one file, so Regular/Bold are runtime coordinates.
/// `set_variation` needs `&mut`, so the instance sits in a Mutex.
fn with_font<T>(px: f64, bold: bool, f: impl FnOnce(&FontVec) -> T) -> T {
    static FONT: OnceLock<Mutex<FontVec>> = OnceLock::new();
    let mut font = FONT
        .get_or_init(|| {
            Mutex::new(
                FontVec::try_from_vec(include_bytes!("../fonts/Inter.var.ttf").to_vec())
                    .expect("bundled Inter.var.ttf must parse"),
            )
        })
        .lock()
        .unwrap();
    font.set_variation(b"wght", if bold { 700.0 } else { 400.0 });
    font.set_variation(b"opsz", px as f32);
    f(&font)
}

/// (advance width, line height) of `text` at `px` logical — used for the
/// caret position and the annotation bbox.
pub fn text_extent(text: &str, px: f64, bold: bool) -> (f64, f64) {
    with_font(px, bold, |f| {
        let s = f.as_scaled(px as f32);
        let mut w = 0.0f32;
        let mut prev = None;
        for c in text.chars() {
            let id = s.glyph_id(c);
            if let Some(p) = prev {
                w += s.kern(p, id);
            }
            w += s.h_advance(id);
            prev = Some(id);
        }
        (w as f64, s.height() as f64)
    })
}

/// Rasterize text at physical res into the blend target. `pos` is the
/// top-left in logical coords; the baseline sits at `pos.y + ascent`.
#[allow(clippy::too_many_arguments)]
fn render_text(
    t: &mut impl Blend,
    pos: Point,
    text: &str,
    px: f64,
    bold: bool,
    rgb: u32,
    vrect: Rect,
    scale: f64,
) {
    if text.is_empty() {
        return;
    }
    with_font(px, bold, |f| {
        let s = f.as_scaled((px * scale) as f32);
        let mut pen = point(
            (pos.x - vrect.x) as f32 * scale as f32,
            (pos.y - vrect.y) as f32 * scale as f32 + s.ascent(),
        );
        let mut prev = None;
        for c in text.chars() {
            let id = s.glyph_id(c);
            if let Some(p) = prev {
                pen.x += s.kern(p, id);
            }
            if let Some(og) = f.outline_glyph(id.with_scale_and_position((px * scale) as f32, pen))
            {
                let bb = og.px_bounds();
                og.draw(|gx, gy, cov| {
                    t.blend(
                        bb.min.x as i32 + gx as i32,
                        bb.min.y as i32 + gy as i32,
                        rgb,
                        cov as f64,
                    );
                });
            }
            pen.x += s.h_advance(id);
            prev = Some(id);
        }
    });
}

/// Raster resolution of the letter icon masks (grid covers logical ±8).
pub(crate) const ICON_MASK_N: usize = 56;

/// Text-label coverage grid for the toolbar icons — the real font instead
/// of hand-drawn SDF glyphs, so "T", "B" and the size numbers stay legible.
pub(crate) fn label_mask(label: &'static str, bold: bool) -> &'static [f32] {
    static MASKS: OnceLock<HashMap<(&'static str, bool), Vec<f32>>> = OnceLock::new();
    &MASKS.get_or_init(|| {
        let mut m = HashMap::new();
        m.insert(("T", false), raster_label("T", false));
        m.insert(("B", true), raster_label("B", true));
        for (_, lbl) in TEXT_SIZES {
            m.insert((lbl, false), raster_label(lbl, false));
        }
        m
    })[&(label, bold)]
}

fn raster_label(label: &str, bold: bool) -> Vec<f32> {
    let n = ICON_MASK_N;
    let mut m = vec![0f32; n * n];
    with_font(16.0, bold, |f| {
        let px = n as f32 * 0.72;
        let s = f.as_scaled(px);
        // lay the run out along a y=0 baseline and keep the outlines
        let mut pen = point(0.0, 0.0);
        let mut prev = None;
        let mut glyphs = Vec::new();
        for c in label.chars() {
            let id = s.glyph_id(c);
            if let Some(p) = prev {
                pen.x += s.kern(p, id);
            }
            if let Some(og) = f.outline_glyph(id.with_scale_and_position(px, pen)) {
                glyphs.push(og);
            }
            pen.x += s.h_advance(id);
            prev = Some(id);
        }
        if glyphs.is_empty() {
            return;
        }
        // union ink bounds -> center the whole run in the grid
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for g in &glyphs {
            let b = g.px_bounds();
            x0 = x0.min(b.min.x);
            y0 = y0.min(b.min.y);
            x1 = x1.max(b.max.x);
            y1 = y1.max(b.max.y);
        }
        let (w, h) = (x1 - x0, y1 - y0);
        for g in &glyphs {
            let bb = g.px_bounds();
            g.draw(|x, y, cov| {
                let gx = (bb.min.x - x0 + x as f32 + (n as f32 - w) / 2.0) as i32;
                let gy = (bb.min.y - y0 + y as f32 + (n as f32 - h) / 2.0) as i32;
                if (0..n as i32).contains(&gx) && (0..n as i32).contains(&gy) {
                    let i = gy as usize * n + gx as usize;
                    m[i] = m[i].max(cov);
                }
            });
        }
    });
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text must rasterize, and bold must lay down more ink than regular.
    #[test]
    fn text_renders_and_bold_fattens() {
        fn ink(text: &str, bold: bool) -> usize {
            let mut buf = vec![0u32; 220 * 60];
            let an = Annotation::Text {
                pos: Point::new(6, 6),
                text: text.into(),
                px: 28.0,
                bold,
                rgb: 0xFF0000,
            };
            an.render_phys(
                &mut ArgbLayer {
                    buf: &mut buf,
                    w: 220,
                    h: 60,
                },
                Rect::new(0, 0, 220, 60),
                1.0,
            );
            buf.iter().filter(|&&p| (p >> 24) > 0).count()
        }
        let reg = ink("Quick", false);
        let bold = ink("Quick", true);
        assert!(reg > 200, "regular text left no ink: {reg}");
        assert!(bold > reg, "bold {bold} should exceed regular {reg}");
        let (w, h) = text_extent("Quick", 28.0, false);
        assert!(w > 40.0 && h > 20.0, "extent {w}x{h}");
        // toolbar label masks (digits + letters) must contain real ink
        for (lbl, b) in [("T", false), ("B", true), ("14", false), ("48", false)] {
            let m = label_mask(lbl, b);
            assert!(
                m.iter().any(|&v| v > 0.5),
                "label mask {lbl:?} has no ink"
            );
        }
    }

    /// Eyeball check — `cargo test -p qs-overlay -- --ignored` then open
    /// /tmp/qs-text-test.png
    #[test]
    #[ignore]
    fn text_visual() {
        let mut img = RgbaImage::from_pixel(400, 200, image::Rgba([40, 44, 52, 255]));
        let annots = vec![
            Annotation::Text {
                pos: Point::new(20, 20),
                text: "Quick short".into(),
                px: TEXT_SIZES[1].0,
                bold: false,
                rgb: PALETTE[4],
            },
            Annotation::Text {
                pos: Point::new(20, 60),
                text: "Regular & Bold".into(),
                px: TEXT_SIZES[3].0,
                bold: true,
                rgb: PALETTE[6],
            },
            Annotation::Text {
                pos: Point::new(20, 110),
                text: "Xin chào Việt Nam".into(),
                px: TEXT_SIZES[5].0,
                bold: false,
                rgb: PALETTE[0],
            },
        ];
        annotate_image(&mut img, Rect::new(0, 0, 400, 200), &annots, 1.0);
        img.save("/tmp/qs-text-test.png").unwrap();
    }
}
