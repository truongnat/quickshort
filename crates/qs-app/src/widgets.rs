//! Shared softbuffer widgets for the small always-on-top windows
//! (toast, quick-access card, recording stop). fontdue rasterization,
//! the card/pill SDF shapes, and monitor placement live here once —
//! every popup draws itself on top of these.

use fontdue::Font;
use qs_overlay::sdf::{fill_cov, rrect_sdf, stroke_cov, Canvas};

/// Text rasterizer: `x`/`y` in logical px, `y` = glyph baseline.
pub struct Txt<'a> {
    pub font: &'a Font,
    pub scale: f64,
}

impl Txt<'_> {
    pub fn draw(&self, cv: &mut Canvas, x: f64, y: f64, size: f64, s: &str, rgb: u32) {
        let px = size * self.scale;
        let mut pen = x * self.scale;
        let baseline = y * self.scale;
        for ch in s.chars() {
            let (m, bmp) = self.font.rasterize(ch, px as f32);
            let gx = (pen + m.xmin as f64) as i32;
            let gy = (baseline - m.height as f64 - m.ymin as f64) as i32;
            for row in 0..m.height {
                for col in 0..m.width {
                    let a = bmp[row * m.width + col] as f64 / 255.0;
                    if a > 0.0 {
                        cv.blend(gx + col as i32, gy + row as i32, rgb, a);
                    }
                }
            }
            pen += m.advance_width as f64;
        }
    }

    pub fn width(&self, size: f64, s: &str) -> f64 {
        let px = (size * self.scale) as f32;
        s.chars().map(|c| self.font.metrics(c, px).advance_width).sum::<f32>() as f64
            / self.scale
    }
}

/// Window-sized rounded card: fill + rim inside, soft fringe outside
/// (stands in for the drop shadow a transparent window can't show —
/// softbuffer ignores per-pixel alpha, so the whole surface is painted).
pub fn card_chrome(
    cv: &mut Canvas,
    w: f64,
    h: f64,
    rad: f64,
    fill: u32,
    border: u32,
    fringe: u32,
) {
    let scale = cv.scale;
    for py in 0..cv.h as i32 {
        for px in 0..cv.w as i32 {
            let d = rrect_sdf(
                px as f64 / scale,
                py as f64 / scale,
                w / 2.0,
                h / 2.0,
                w / 2.0,
                h / 2.0,
                rad,
            );
            if d > 0.0 {
                cv.blend(px, py, fringe, 0.35 * (1.0 - d / 2.5).clamp(0.0, 1.0));
                continue;
            }
            cv.blend(px, py, fill, fill_cov(d, scale));
            let rim = stroke_cov(d, 0.7, scale);
            if rim > 0.0 {
                cv.blend(px, py, border, rim);
            }
        }
    }
}

/// Rounded button background + rim. `alpha` dims the fill (disabled).
pub fn pill(
    cv: &mut Canvas,
    rect: (f64, f64, f64, f64),
    rad: f64,
    bg: u32,
    border: u32,
    alpha: f64,
) {
    let (bx, by, bw, bh) = rect;
    let scale = cv.scale;
    for py in (by * scale) as i32..((by + bh) * scale) as i32 {
        for px in (bx * scale) as i32..((bx + bw) * scale) as i32 {
            let d = rrect_sdf(
                px as f64 / scale,
                py as f64 / scale,
                bx + bw / 2.0,
                by + bh / 2.0,
                bw / 2.0,
                bh / 2.0,
                rad,
            );
            if d <= 0.0 {
                cv.blend(px, py, bg, fill_cov(d, scale) * alpha);
                let rim = stroke_cov(d, 0.7, scale);
                if rim > 0.0 {
                    cv.blend(px, py, border, rim);
                }
            }
        }
    }
}

/// Multiply 0xRRGGBB channels — hover shade for a filled accent pill.
pub fn shade(rgb: u32, f: f64) -> u32 {
    let c = |sh: u32| (((rgb >> sh) & 0xFF) as f64 * f) as u32;
    (c(16) << 16) | (c(8) << 8) | c(0)
}

/// Monitor containing `anchor`'s centre — popups dock on the screen
/// that was captured, not always the primary. Fallback: primary → first.
pub fn monitor_of(
    el: &winit::event_loop::ActiveEventLoop,
    anchor: Option<qs_core::Rect>,
) -> Option<winit::monitor::MonitorHandle> {
    let m = anchor.and_then(|a| {
        let (cx, cy) = (a.x as f64 + a.w as f64 / 2.0, a.y as f64 + a.h as f64 / 2.0);
        el.available_monitors().find(|m| {
            let sf = m.scale_factor();
            let p = m.position();
            let s = m.size();
            let (x, y) = (p.x as f64 / sf, p.y as f64 / sf);
            let (w, h) = (s.width as f64 / sf, s.height as f64 / sf);
            cx >= x && cx < x + w && cy >= y && cy < y + h
        })
    });
    m.or_else(|| {
        el.primary_monitor()
            .or_else(|| el.available_monitors().next())
    })
}
