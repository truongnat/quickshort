//! Guided scrolling capture: the overlay's Scroll action hands the
//! selection here; the loop re-captures the same rect on a timer while
//! the user scrolls, then overlap-stitches the grabs vertically.
//!
//! Finish conditions: the content stops moving (three consecutive
//! identical grabs) or the safety caps (shot count / total time). When a
//! grab can't be matched to the previous one — fast scroll past the
//! frame height, or the page replaced its content — the loop keeps what
//! it already has rather than emit a broken image.

use anyhow::{Context, Result};
use image::RgbaImage;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(450);
const MAX_SHOTS: usize = 24;
const MAX_TIME: Duration = Duration::from_secs(90);
const IDLE_STOP: u32 = 3;
/// Minimum overlap a match may claim — smaller overlaps can't tell a big
/// scroll from unrelated content.
const MIN_OVERLAP: usize = 32;
/// Mean abs per-channel difference accepted as "same content".
const EPS: f64 = 6.0;

/// Block until the scroll settles, then return the stitched tall image.
pub fn run(sel: qs_core::Rect) -> Result<RgbaImage> {
    let backend = qs_capture::backend()?;
    // One session reused across polls — a fresh portal session each grab
    // would re-prompt for consent on Wayland.
    let mut session = backend.session()?;
    let mut grab = || -> Result<RgbaImage> {
        let frames = session()?;
        qs_core::stitch(&frames, sel)
            .map(|(i, _)| i)
            .context("selection covers no monitor")
    };

    // (image, top offset in the final canvas)
    let mut shots: Vec<(RgbaImage, usize)> = vec![(grab()?, 0)];
    let mut top = 0usize;
    let mut idle = 0u32;
    let t0 = Instant::now();

    for _ in 0..MAX_SHOTS - 1 {
        if t0.elapsed() > MAX_TIME {
            break;
        }
        std::thread::sleep(POLL);
        let cur = grab()?;
        match shift(&shots.last().unwrap().0, &cur) {
            Some(0) => {
                idle += 1;
                if idle >= IDLE_STOP {
                    break;
                }
            }
            Some(dy) => {
                top += dy;
                shots.push((cur, top));
                idle = 0;
            }
            None => break,
        }
    }

    let h = shots
        .last()
        .map(|(i, t)| t + i.height() as usize)
        .context("no frames captured")?;
    let mut out = RgbaImage::new(shots[0].0.width(), h as u32);
    for (img, t) in &shots {
        image::imageops::overlay(&mut out, img, 0, *t as i64);
    }
    Ok(out)
}

/// How far `b` is scrolled down relative to `a` — `Some(0)` is identical
/// content, `None` means no trustworthy overlap.
fn shift(a: &RgbaImage, b: &RgbaImage) -> Option<usize> {
    let (ha, hb) = (a.height() as usize, b.height() as usize);
    let max_o = ha.min(hb);
    if diff(a, b, 0, max_o) < EPS {
        return Some(0);
    }
    let mut best = (f64::MAX, 0usize);
    // every overlap — the diff only drops near the true alignment, so a
    // coarse step could skip it entirely
    for o in (MIN_OVERLAP..max_o).rev() {
        let d = diff(a, b, ha - o, o);
        if d < best.0 {
            best = (d, ha - o);
        }
    }
    (best.0 < EPS).then_some(best.1)
}

/// Mean abs difference over the overlap: `a`'s rows [a_off, a_off+rows)
/// vs `b`'s rows [0, rows), sampled sparsely.
fn diff(a: &RgbaImage, b: &RgbaImage, a_off: usize, rows: usize) -> f64 {
    let w = a.width().min(b.width()) as usize;
    let (mut s, mut n) = (0u64, 0u64);
    for r in (0..rows).step_by(4) {
        for x in (0..w).step_by(16) {
            let pa = a.get_pixel(x as u32, (a_off + r) as u32).0;
            let pb = b.get_pixel(x as u32, r as u32).0;
            s += pa[0].abs_diff(pb[0]) as u64
                + pa[1].abs_diff(pb[1]) as u64
                + pa[2].abs_diff(pb[2]) as u64;
            n += 3;
        }
    }
    if n == 0 {
        f64::MAX
    } else {
        s as f64 / n as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tall "page": each row's color is a hash of its y so a one-row
    /// misalignment already diffs hard (smooth content would mask it).
    fn page(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([
                (y.wrapping_mul(37) % 256) as u8,
                (y.wrapping_mul(101) % 256) as u8,
                (x % 64) as u8,
                255,
            ])
        })
    }

    fn crop(src: &RgbaImage, top: u32, h: u32) -> RgbaImage {
        image::imageops::crop_imm(src, 0, top, src.width(), h).to_image()
    }

    #[test]
    fn identical_frames_give_zero_shift() {
        let p = page(64, 600);
        let a = crop(&p, 100, 300);
        assert_eq!(shift(&a, &a.clone()), Some(0));
    }

    #[test]
    fn scrolled_content_finds_its_shift() {
        let p = page(64, 1000);
        let a = crop(&p, 100, 300);
        let b = crop(&p, 160, 300); // user scrolled down 60px
        assert_eq!(shift(&a, &b), Some(60));
    }

    #[test]
    fn unrelated_content_rejects() {
        let p = page(64, 600);
        let a = crop(&p, 100, 300);
        // noise frame shares no overlap
        let b = RgbaImage::from_fn(64, 300, |x, y| {
            image::Rgba([(x * 7 % 256) as u8, (y * 13 % 256) as u8, 0, 255])
        });
        assert_eq!(shift(&a, &b), None);
    }

    #[test]
    fn small_scroll_still_matches() {
        let p = page(64, 800);
        let a = crop(&p, 200, 300);
        let b = crop(&p, 208, 300);
        assert_eq!(shift(&a, &b), Some(8));
    }
}
