//! Core domain types and the image-buffer stitching algorithm.
//!
//! Coordinate model: all monitors live in one *virtual* coordinate space
//! expressed in **logical** pixels. Each monitor has an `origin` (its top-left
//! corner in virtual space) and a `scale_factor` relating logical units to the
//! physical pixels stored in its `CapturedFrame`:
//!
//! ```text
//! physical = logical * scale_factor
//! ```

use image::{imageops, RgbaImage};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Size {
    pub w: u32,
    pub h: u32,
}

impl Size {
    pub fn new(w: u32, h: u32) -> Self {
        Self { w, h }
    }
}

/// Rectangle in logical virtual coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    /// Rect from two arbitrary drag points (any direction).
    pub fn from_drag(a: Point, b: Point) -> Self {
        let x = a.x.min(b.x);
        let y = a.y.min(b.y);
        Self {
            x,
            y,
            w: (a.x - b.x).unsigned_abs(),
            h: (a.y - b.y).unsigned_abs(),
        }
    }

    pub fn right(&self) -> i32 {
        self.x + self.w as i32
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h as i32
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    pub fn translated(&self, dx: i32, dy: i32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            ..*self
        }
    }

    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let r = self.right().min(other.right());
        let b = self.bottom().min(other.bottom());
        (r > x && b > y).then(|| Rect::new(x, y, (r - x) as u32, (b - y) as u32))
    }
}

/// One monitor's placement in virtual space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MonitorGeometry {
    /// Top-left corner in virtual logical coords (may be negative).
    pub origin: Point,
    /// Logical size (`physical_pixels / scale_factor`).
    pub size: Size,
    /// physical / logical ratio (1.0 on X11, 2.0 on Retina, 1.25/1.5/... on Windows).
    pub scale: f64,
}

impl MonitorGeometry {
    pub fn logical_rect(&self) -> Rect {
        Rect::new(self.origin.x, self.origin.y, self.size.w, self.size.h)
    }
}

/// Raw screenshot of one monitor plus where it sits in virtual space.
pub struct CapturedFrame {
    pub geometry: MonitorGeometry,
    /// Physical pixels; expected ≈ `size * scale` in each dimension.
    pub pixels: RgbaImage,
}

/// Bounding rect of all frames in virtual logical coords.
pub fn virtual_bounds(frames: &[CapturedFrame]) -> Rect {
    let mut b = Rect::default();
    for (i, f) in frames.iter().enumerate() {
        let r = f.geometry.logical_rect();
        b = if i == 0 {
            r
        } else {
            let x = b.x.min(r.x);
            let y = b.y.min(r.y);
            Rect::new(
                x,
                y,
                (b.right().max(r.right()) - x) as u32,
                (b.bottom().max(r.bottom()) - y) as u32,
            )
        };
    }
    b
}

/// Crop `selection` (virtual logical coords) out of `frames` and stitch into one
/// image.
///
/// Output canvas size = selection size in **logical** pixels, i.e. everything is
/// normalized to scale 1.0. Hi-DPI regions are downsampled — predictable output
/// size was preferred over keeping per-monitor native resolution.
///
/// Returns `None` when the selection covers no monitor area.
pub fn stitch(frames: &[CapturedFrame], selection: Rect) -> Option<RgbaImage> {
    if selection.is_empty() {
        return None;
    }
    let mut out = RgbaImage::new(selection.w, selection.h);
    let mut covered = false;

    for frame in frames {
        let mon = frame.geometry.logical_rect();
        let Some(inter) = selection.intersection(&mon) else {
            continue;
        };
        covered = true;

        // intersection in monitor-local logical coords -> physical crop rect
        let local = inter.translated(-mon.x, -mon.y);
        let s = frame.geometry.scale;
        let px = (local.x as f64 * s).round().max(0.0) as u32;
        let py = (local.y as f64 * s).round().max(0.0) as u32;
        let pr = ((local.x + inter.w as i32) as f64 * s).round() as u32;
        let pb = ((local.y + inter.h as i32) as f64 * s).round() as u32;
        // clamp to the real buffer; scale * size may drift by a pixel
        let px = px.min(frame.pixels.width());
        let py = py.min(frame.pixels.height());
        let pw = pr.min(frame.pixels.width()).saturating_sub(px);
        let ph = pb.min(frame.pixels.height()).saturating_sub(py);
        if pw == 0 || ph == 0 {
            continue;
        }

        let crop = imageops::crop_imm(&frame.pixels, px, py, pw, ph).to_image();
        let dx = (inter.x - selection.x) as i64;
        let dy = (inter.y - selection.y) as i64;
        if (s - 1.0).abs() < f64::EPSILON && pw == inter.w && ph == inter.h {
            imageops::overlay(&mut out, &crop, dx, dy);
        } else {
            let normalized =
                imageops::resize(&crop, inter.w, inter.h, imageops::FilterType::Triangle);
            imageops::overlay(&mut out, &normalized, dx, dy);
        }
    }

    covered.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba(c))
    }

    fn frame(x: i32, y: i32, lw: u32, lh: u32, scale: f64, c: [u8; 4]) -> CapturedFrame {
        let pw = (lw as f64 * scale).round() as u32;
        let ph = (lh as f64 * scale).round() as u32;
        CapturedFrame {
            geometry: MonitorGeometry {
                origin: Point::new(x, y),
                size: Size::new(lw, lh),
                scale,
            },
            pixels: solid(pw, ph, c),
        }
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    #[test]
    fn stitch_single_monitor() {
        let frames = vec![frame(0, 0, 100, 100, 1.0, RED)];
        let out = stitch(&frames, Rect::new(10, 10, 40, 30)).unwrap();
        assert_eq!((out.width(), out.height()), (40, 30));
        assert_eq!(out.get_pixel(0, 0).0, RED);
        assert_eq!(out.get_pixel(39, 29).0, RED);
    }

    #[test]
    fn stitch_across_two_monitors_side_by_side() {
        // mon A: [0,0 100x100] RED | mon B: [100,0 100x100] GREEN
        let frames = vec![
            frame(0, 0, 100, 100, 1.0, RED),
            frame(100, 0, 100, 100, 1.0, GREEN),
        ];
        // selection crosses the boundary: x in [50,150)
        let out = stitch(&frames, Rect::new(50, 20, 100, 50)).unwrap();
        assert_eq!((out.width(), out.height()), (100, 50));
        assert_eq!(out.get_pixel(0, 0).0, RED); // left half from A
        assert_eq!(out.get_pixel(49, 25).0, RED);
        assert_eq!(out.get_pixel(50, 0).0, GREEN); // right half from B
        assert_eq!(out.get_pixel(99, 49).0, GREEN);
    }

    #[test]
    fn stitch_monitor_with_negative_origin() {
        // mon B sits *left* of primary: origin (-100, 0)
        let frames = vec![
            frame(0, 0, 100, 100, 1.0, RED),
            frame(-100, 0, 100, 100, 1.0, GREEN),
        ];
        let out = stitch(&frames, Rect::new(-50, 0, 100, 100)).unwrap();
        assert_eq!(out.get_pixel(0, 0).0, GREEN); // x∈[-50,0) from B
        assert_eq!(out.get_pixel(50, 0).0, RED); // x∈[0,50) from A
    }

    #[test]
    fn stitch_normalizes_high_dpi() {
        // mon A scale 1 (RED) | mon B scale 2: logical 100x100 -> 200x200 phys (BLUE)
        let frames = vec![
            frame(0, 0, 100, 100, 1.0, RED),
            frame(100, 0, 100, 100, 2.0, BLUE),
        ];
        let out = stitch(&frames, Rect::new(50, 0, 100, 100)).unwrap();
        assert_eq!((out.width(), out.height()), (100, 100));
        assert_eq!(out.get_pixel(25, 50).0, RED);
        assert_eq!(out.get_pixel(75, 50).0, BLUE); // downscaled but correct half
    }

    #[test]
    fn stitch_stacked_vertical() {
        // A on top, B below
        let frames = vec![
            frame(0, 0, 100, 100, 1.0, RED),
            frame(0, 100, 100, 100, 1.0, GREEN),
        ];
        let out = stitch(&frames, Rect::new(0, 50, 100, 100)).unwrap();
        assert_eq!(out.get_pixel(50, 10).0, RED);
        assert_eq!(out.get_pixel(50, 60).0, GREEN);
    }

    #[test]
    fn stitch_selection_outside_all_monitors_is_none() {
        let frames = vec![frame(0, 0, 100, 100, 1.0, RED)];
        assert!(stitch(&frames, Rect::new(500, 500, 10, 10)).is_none());
        assert!(stitch(&frames, Rect::new(0, 0, 0, 0)).is_none());
    }

    #[test]
    fn stitch_gap_between_monitors_is_black() {
        // gap x∈[100,110) has no monitor
        let frames = vec![
            frame(0, 0, 100, 100, 1.0, RED),
            frame(110, 0, 100, 100, 1.0, BLUE),
        ];
        let out = stitch(&frames, Rect::new(90, 0, 40, 10)).unwrap();
        assert_eq!(out.get_pixel(5, 5).0, RED); // x=95 <100
        assert_eq!(out.get_pixel(15, 5).0, [0, 0, 0, 0]); // x=105 in the gap
        assert_eq!(out.get_pixel(25, 5).0, BLUE); // x=115 >=110
    }

    #[test]
    fn rect_from_drag_normalizes_direction() {
        let r = Rect::from_drag(Point::new(150, 80), Point::new(50, 20));
        assert_eq!(r, Rect::new(50, 20, 100, 60));
    }

    #[test]
    fn virtual_bounds_spans_all_monitors() {
        let frames = vec![
            frame(-100, 0, 100, 100, 1.0, GREEN),
            frame(0, 0, 200, 150, 1.0, RED),
        ];
        assert_eq!(virtual_bounds(&frames), Rect::new(-100, 0, 300, 150));
    }
}
