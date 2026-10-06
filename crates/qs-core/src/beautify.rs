//! Beautify: mount a capture on a soft gradient backdrop with rounded
//! corners and a drop shadow — the "screenshot for a social post" look.
//! Pure image transform so it stays unit-testable and UI-agnostic.

use image::RgbaImage;

/// Backdrop geometry. `pad` is breathing room around the shot, `radius`
/// the shot's corner rounding, `shadow` the drop-shadow blur radius and
/// `dy` its vertical offset.
#[derive(Clone, Copy)]
pub struct Beautify {
    pub pad: u32,
    pub radius: u32,
    pub shadow: u32,
    pub dy: u32,
}

impl Default for Beautify {
    fn default() -> Self {
        Self {
            pad: 64,
            radius: 16,
            shadow: 32,
            dy: 16,
        }
    }
}

/// Gradient stops, top-left → bottom-right (linear light backdrop).
const BG_HI: [f64; 3] = [0xF6 as f64, 0xF8 as f64, 0xFC as f64];
const BG_LO: [f64; 3] = [0xC3 as f64, 0xCE as f64, 0xE2 as f64];
/// Shadow color/peak opacity — softened again by the blur falloff.
const SHADOW: u32 = 0x101828;
const SHADOW_A: f64 = 0.32;

/// `img` padded onto the backdrop. Output is `img + 2*pad` per axis.
pub fn beautify(img: &RgbaImage) -> RgbaImage {
    beautify_opts(img, Beautify::default())
}

pub fn beautify_opts(img: &RgbaImage, o: Beautify) -> RgbaImage {
    let (iw, ih) = (img.width() as usize, img.height() as usize);
    let (pad, r) = (o.pad as usize, o.radius as f64);
    let (w, h) = (iw + 2 * pad, ih + 2 * pad);
    let mut out = RgbaImage::new(w as u32, h as u32);

    // diagonal light gradient
    for y in 0..h {
        for x in 0..w {
            let t = (x as f64 / w as f64 + y as f64 / h as f64) / 2.0;
            let c = |i: usize| (BG_HI[i] + (BG_LO[i] - BG_HI[i]) * t).round() as u8;
            out.put_pixel(x as u32, y as u32, image::Rgba([c(0), c(1), c(2), 255]));
        }
    }

    // drop shadow: rounded-rect stencil, gaussian-approximated by three
    // separable box-blur passes
    let mut a = vec![0f32; w * h];
    let (scx, scy) = ((pad + iw / 2) as f64, (pad + o.dy as usize + ih / 2) as f64);
    for y in 0..h {
        for x in 0..w {
            let d = rrect(x as f64, y as f64, scx, scy, iw as f64 / 2.0, ih as f64 / 2.0, r);
            a[y * w + x] = (-d).clamp(0.0, 1.0) as f32;
        }
    }
    for _ in 0..3 {
        a = box_blur(&a, w, h, o.shadow as usize / 3);
    }
    for y in 0..h {
        for x in 0..w {
            let al = (a[y * w + x] as f64 * SHADOW_A).min(1.0);
            if al > 0.0 {
                blend(&mut out, x, y, SHADOW, al);
            }
        }
    }

    // the shot, its corners clipped by the same rounded mask
    let (icx, icy) = ((pad + iw / 2) as f64, (pad + ih / 2) as f64);
    for y in 0..ih {
        for x in 0..iw {
            let d = rrect(
                (pad + x) as f64,
                (pad + y) as f64,
                icx,
                icy,
                iw as f64 / 2.0,
                ih as f64 / 2.0,
                r,
            );
            let cov = (-d).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }
            let p = img.get_pixel(x as u32, y as u32);
            let rgb = ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32;
            blend(&mut out, pad + x, pad + y, rgb, cov * p[3] as f64 / 255.0);
        }
    }
    out
}

/// Signed distance to a rounded rect centred at (cx,cy) — negative inside.
fn rrect(px: f64, py: f64, cx: f64, cy: f64, hw: f64, hh: f64, r: f64) -> f64 {
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    qx.max(qy).min(0.0) + qx.max(0.0).hypot(qy.max(0.0)) - r
}

/// One horizontal+vertical box blur pass; `rad` is the kernel half-width.
fn box_blur(src: &[f32], w: usize, h: usize, rad: usize) -> Vec<f32> {
    if rad == 0 {
        return src.to_vec();
    }
    let mut tmp = vec![0f32; w * h];
    let mut out = vec![0f32; w * h];
    let n = (2 * rad + 1) as f32;
    for y in 0..h {
        let mut acc = 0f32;
        for x in 0..w {
            let xa = x.saturating_sub(rad + 1);
            let xb = (x + rad).min(w - 1);
            if x == 0 {
                acc = (0..=xb).map(|i| src[y * w + i]).sum();
            } else {
                acc += src[y * w + xb];
                if x > rad {
                    acc -= src[y * w + xa];
                }
            }
            tmp[y * w + x] = acc / n;
        }
    }
    for x in 0..w {
        let mut acc = 0f32;
        for y in 0..h {
            let ya = y.saturating_sub(rad + 1);
            let yb = (y + rad).min(h - 1);
            if y == 0 {
                acc = (0..=yb).map(|i| tmp[i * w + x]).sum();
            } else {
                acc += tmp[yb * w + x];
                if y > rad {
                    acc -= tmp[ya * w + x];
                }
            }
            out[y * w + x] = acc / n;
        }
    }
    out
}

/// Alpha-blend `rgb` over the pixel at (x,y).
fn blend(img: &mut RgbaImage, x: usize, y: usize, rgb: u32, a: f64) {
    let d = img.get_pixel_mut(x as u32, y as u32);
    let mix = |s: u8, d: u8| (s as f64 * a + d as f64 * (1.0 - a)).round() as u8;
    d[0] = mix((rgb >> 16) as u8, d[0]);
    d[1] = mix((rgb >> 8) as u8, d[1]);
    d[2] = mix(rgb as u8, d[2]);
    d[3] = 255;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba(c))
    }

    #[test]
    fn output_size_grows_by_padding() {
        let out = beautify(&solid(200, 100, [200, 0, 0, 255]));
        assert_eq!((out.width(), out.height()), (200 + 128, 100 + 128));
    }

    #[test]
    fn center_keeps_the_image() {
        let out = beautify(&solid(200, 100, [200, 0, 0, 255]));
        let p = out.get_pixel(64 + 100, 64 + 50);
        assert_eq!(*p, image::Rgba([200, 0, 0, 255]));
    }

    #[test]
    fn corners_show_backdrop_not_image() {
        // pad-1 inside the image's own corner → rounded clip reveals the
        // backdrop, which must not be the image's red
        let out = beautify(&solid(200, 100, [200, 0, 0, 255]));
        let p = out.get_pixel(66, 66);
        assert_ne!(p[0], 200);
    }

    #[test]
    fn shadow_falls_below_the_image() {
        let out = beautify(&solid(200, 100, [255, 255, 255, 255]));
        // below the shot's bottom edge the backdrop darkens (shadow),
        // above it stays bright
        let below = out.get_pixel(164, 64 + 100 + 20);
        let above = out.get_pixel(164, 30);
        assert!(below[0] < above[0], "below={below:?} above={above:?}");
    }
}
