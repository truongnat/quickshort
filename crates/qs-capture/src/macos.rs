//! macOS backend — ScreenCaptureKit.
//!
//! Per-display one-shot captures via `SCScreenshotManager` (macOS 14+): no
//! stream setup, no subprocess. Logical rects come from `SCDisplay.frame`
//! (points, top-left origin — same space as `CGDisplayBounds` and winit's
//! monitor positions); scale = captured pixels / points.

use anyhow::{bail, Context, Result};
use image::RgbaImage;
use qs_core::{CapturedFrame, MonitorGeometry, Point, Size};
use screencapturekit::prelude::*;
use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};

use crate::ScreenCapture;

pub struct ScreenCaptureKit;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

impl ScreenCapture for ScreenCaptureKit {
    fn capture_all(&self) -> Result<Vec<CapturedFrame>> {
        ensure_screen_permission()?;

        let content = SCShareableContent::get().context("shareable content")?;
        let mut frames = Vec::with_capacity(content.displays().len());
        for display in content.displays() {
            let frame = display.frame();
            let filter = SCContentFilter::create()
                .with_display(&display)
                .with_excluding_windows(&[])
                .build()
                .context("content filter")?;
            let config = SCStreamConfiguration::new()
                .with_width(display.width())
                .with_height(display.height())
                .with_shows_cursor(false)
                .with_pixel_format(PixelFormat::BGRA);
            let image = SCScreenshotManager::capture_image(&filter, &config)
                .context("screenshot capture")?;

            let (w, h) = (image.width() as u32, image.height() as u32);
            let pixels = RgbaImage::from_raw(w, h, image.rgba_data()?)
                .context("captured image size mismatch")?;
            frames.push(CapturedFrame {
                geometry: MonitorGeometry {
                    origin: Point::new(
                        frame.origin.x.round() as i32,
                        frame.origin.y.round() as i32,
                    ),
                    size: Size::new(
                        frame.size.width.round() as u32,
                        frame.size.height.round() as u32,
                    ),
                    scale: w as f64 / frame.size.width,
                },
                pixels,
            });
        }
        Ok(frames)
    }
}

/// Screen Recording is a TCC permission: the system prompt appears once, and a
/// grant only applies to a *restarted* process — so ask, then fail loud.
fn ensure_screen_permission() -> Result<()> {
    if unsafe { CGPreflightScreenCaptureAccess() } {
        return Ok(());
    }
    unsafe { CGRequestScreenCaptureAccess() };
    bail!(
        "Screen Recording permission required: enable it for this terminal app \
         in System Settings > Privacy & Security > Screen Recording, then re-run"
    );
}
