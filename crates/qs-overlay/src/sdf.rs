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
}
