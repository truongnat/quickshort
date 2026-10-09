//! Shared SDF primitives and the alpha-blending pixel target used by the
//! toolbar icons and the annotation renderer.

#[inline]
pub fn seg_sdf(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    ((px - ax - dx * t).powi(2) + (py - ay - dy * t).powi(2)).sqrt()
}

/// Signed distance to a rounded-rect boundary (negative inside).
#[inline]
pub fn rrect_sdf(px: f64, py: f64, cx: f64, cy: f64, hw: f64, hh: f64, r: f64) -> f64 {
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    let (ox, oy) = (qx.max(0.0), qy.max(0.0));
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

/// Approximate signed distance to an ellipse boundary (negative inside) —
/// Newton step `(k - 1) / |∇k|` on the implicit `x²/rx² + y²/ry² = 1`.
#[inline]
pub fn ellipse_sdf(px: f64, py: f64, cx: f64, cy: f64, rx: f64, ry: f64) -> f64 {
    let dx = px - cx;
    let dy = py - cy;
    let k = dx * dx / (rx * rx) + dy * dy / (ry * ry);
    let gx = 2.0 * dx / (rx * rx);
    let gy = 2.0 * dy / (ry * ry);
    (k - 1.0) / (gx * gx + gy * gy).sqrt().max(1e-6)
}

/// Coverage of a stroke of logical width `w` centered on `sdf` (logical dist).
#[inline]
pub fn stroke_cov(sdf_l: f64, w: f64, scale: f64) -> f64 {
    (w / 2.0 + 0.5 - sdf_l.abs() * scale).clamp(0.0, 1.0)
}

/// Coverage of a fill of the region with `sdf <= 0` (logical dist).
#[inline]
pub fn fill_cov(sdf_l: f64, scale: f64) -> f64 {
    (0.5 - sdf_l * scale).clamp(0.0, 1.0)
}

/// Glass card — the Liquid Glass signature baked into pixels: soft drop
/// shadow, a subtle top-to-bottom gradient fill, a hairline rim, and a
/// brighter 1px edge along the top half (edge refraction).
///
/// `top`/`bot` are the fill colors; keep them close for a quiet surface.
pub fn glass_card(cv: &mut Canvas, r: (f64, f64, f64, f64), rad: f64, top: u32, bot: u32) {
    let (x, y, w, h) = r;
    let s = cv.scale;
    let pad = 10.0; // shadow reach, logical px
    let (x0, y0) = (((x - pad) * s) as i32, ((y - pad) * s) as i32);
    let (x1, y1) = (((x + w + pad) * s) as i32, ((y + h + pad) * s) as i32);
    let cy = y + h / 2.0;
    for py in y0.max(0)..y1.min(cv.h as i32) {
        for px in x0.max(0)..x1.min(cv.w as i32) {
            let lx = px as f64 / s + 0.5 / s;
            let ly = py as f64 / s + 0.5 / s;
            let d = rrect_sdf(lx, ly, x + w / 2.0, cy, w / 2.0, h / 2.0, rad);
            if d > 0.0 {
                // soft shadow: offset down 1.5px, alpha falls off with distance
                let sd = rrect_sdf(lx, ly, x + w / 2.0, cy + 1.5, w / 2.0, h / 2.0, rad);
                if sd > 0.0 {
                    let a = 0.32 * (1.0 - sd / pad).clamp(0.0, 1.0);
                    if a > 0.002 {
                        cv.blend(px, py, 0x000000, a);
                    }
                }
                continue;
            }
            // fill: vertical gradient
            let t = ((ly - y) / h).clamp(0.0, 1.0);
            let rgb = lerp_rgb(top, bot, t);
            let cov = fill_cov(d, s);
            cv.blend(px, py, rgb, cov);
            // rim: hairline all around + stronger light edge on the top half
            let rim = stroke_cov(d, 0.6, s) * 0.10;
            if rim > 0.0 {
                cv.blend(px, py, 0xFFFFFF, rim);
            }
            if ly < cy {
                let topness = (1.0 - (ly - y) / (h / 2.0)).clamp(0.0, 1.0);
                let glow = stroke_cov(d, 1.0, s) * 0.22 * topness;
                if glow > 0.0 {
                    cv.blend(px, py, 0xFFFFFF, glow);
                }
            }
        }
    }
}

#[inline]
fn lerp_rgb(a: u32, b: u32, t: f64) -> u32 {
    let f = |sh: u32| ((a >> sh) & 0xFF) as f64 * (1.0 - t) + ((b >> sh) & 0xFF) as f64 * t;
    (f(16) as u32) << 16 | (f(8) as u32) << 8 | f(0) as u32
}

/// Physical-px drawing target: alpha-blends packed 0xRRGGBB over opaque
/// pixels (per-channel mix — no cross-channel carry).
pub struct Canvas<'a> {
    pub buf: &'a mut [u32],
    pub w: usize,
    pub h: usize,
    /// logical -> physical ratio
    pub scale: f64,
}

impl Canvas<'_> {
    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, rgb: u32, a: f64) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 || a <= 0.0 {
            return;
        }
        let i = y as usize * self.w + x as usize;
        let d = self.buf[i];
        let mix = |s: u8, d: u8| (s as f64 * a + d as f64 * (1.0 - a)).round() as u32;
        self.buf[i] = mix((rgb >> 16) as u8, (d >> 16) as u8) << 16
            | mix((rgb >> 8) as u8, (d >> 8) as u8) << 8
            | mix(rgb as u8, d as u8);
    }

    /// Source-over onto a **premultiplied-alpha** destination — the chrome
    /// layer's pixel format. `buf` holds `A<<24 | B<<16 | G<<8 | R` so the
    /// little-endian byte order is R,G,B,A (what NSBitmapImageRep/wl_shm
    /// ARGB8888 want); `rgb` is a straight 0xRRGGBB color at coverage `a`.
    /// Chrome surfaces must start fully transparent (`buf` = 0).
    #[inline]
    pub fn blend_over(&mut self, x: i32, y: i32, rgb: u32, a: f64) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 || a <= 0.0 {
            return;
        }
        let a = a.min(1.0);
        let i = y as usize * self.w + x as usize;
        let d = self.buf[i];
        let ia = 1.0 - a;
        let oa = a * 255.0 + (d >> 24) as f64 * ia;
        let or_ = ((rgb >> 16) & 0xFF) as f64 * a + (d & 0xFF) as f64 * ia;
        let og = ((rgb >> 8) & 0xFF) as f64 * a + ((d >> 8) & 0xFF) as f64 * ia;
        let ob = (rgb & 0xFF) as f64 * a + ((d >> 16) & 0xFF) as f64 * ia;
        self.buf[i] = (oa.round() as u32) << 24
            | (ob.round() as u32) << 16
            | (og.round() as u32) << 8
            | or_.round() as u32;
    }
}
