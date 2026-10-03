//! Flameshot-style annotations: vector shapes kept in virtual logical coords.
//! Committed strokes rasterize once into an ARGB overlay layer (sized like the
//! virtual canvas); the output path re-rasterizes the same list onto the
//! stitched image — `stitch` normalizes to scale 1.0 so coords map 1:1.

use crate::sdf::{ellipse_sdf, rrect_sdf, seg_sdf, stroke_cov, Canvas};
use image::RgbaImage;
use qs_core::{Point, Rect};

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
            Self::Pixelate(_) => return None,
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
            Self::Pixelate(_) => f64::INFINITY,
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
