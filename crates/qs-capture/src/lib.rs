//! Platform capture backends. Each OS connects straight to its native API:
//! Windows = GDI, macOS = ScreenCaptureKit, Linux/X11 = xcb (x11rb),
//! Linux/Wayland = xdg-desktop-portal (+ wlr-layer-shell for the overlay).

use anyhow::Result;
use qs_core::CapturedFrame;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// Capture every monitor's raw pixels together with its virtual-space geometry.
pub trait ScreenCapture {
    fn capture_all(&self) -> Result<Vec<CapturedFrame>>;
}

/// Process-wide platform init — must run before both capture and the overlay
/// (Windows DPI awareness changes the monitor geometry winit reports).
pub fn platform_init() {
    #[cfg(target_os = "windows")]
    windows::enable_dpi_awareness();
}

/// Pick the right backend for the current platform/session.
pub fn backend() -> Result<Box<dyn ScreenCapture>> {
    #[cfg(target_os = "linux")]
    {
        linux::backend()
    }
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(windows::GdiCapture))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(macos::ScreenCaptureKit))
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        anyhow::bail!("unsupported platform")
    }
}

#[cfg(test)]
mod tests {
    /// Live capture — needs Screen Recording permission for the test host:
    /// `cargo test -p qs-capture -- --ignored`
    #[test]
    #[ignore]
    fn live_capture_is_physical_res() {
        let frames = crate::backend().unwrap().capture_all().unwrap();
        assert!(!frames.is_empty());
        for f in &frames {
            let g = &f.geometry;
            eprintln!(
                "display: {}x{} logical, {}x{} px, scale {}",
                g.size.w,
                g.size.h,
                f.pixels.width(),
                f.pixels.height(),
                g.scale
            );
            let expected = g.size.w as f64 * g.scale;
            assert!(
                (f.pixels.width() as f64 - expected).abs() <= 2.0,
                "captured {} px for {} logical @ {}",
                f.pixels.width(),
                g.size.w,
                g.scale
            );
        }
        // stitched output keeps physical res: a 200x100 logical selection on
        // the first monitor comes out at *its* pixel scale
        let g = frames[0].geometry;
        let sel = qs_core::Rect::new(g.origin.x + 10, g.origin.y + 10, 200, 100);
        let (img, s) = qs_core::stitch(&frames, sel).unwrap();
        eprintln!(
            "stitch: 200x100 logical -> {}x{} px (scale {})",
            img.width(),
            img.height(),
            s
        );
        assert_eq!(img.width(), (200.0 * s).round() as u32);
    }
}
