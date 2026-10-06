//! macOS backend — ScreenCaptureKit.
//!
//! Per-display one-shot captures via `SCScreenshotManager` (macOS 14+): no
//! stream setup, no subprocess. Logical rects come from `SCDisplay.frame`
//! (points, top-left origin — same space as `CGDisplayBounds` and winit's
//! monitor positions); scale = captured pixels / points.

use anyhow::{bail, Context, Result};
use image::RgbaImage;
use qs_core::{CapturedFrame, MonitorGeometry, Point, Rect, Size};
use screencapturekit::prelude::*;
use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};

use crate::ScreenCapture;

pub struct ScreenCaptureKit;

/// One display → one CapturedFrame. `shows_cursor` is on for the
/// recording path — a screencast without a cursor is useless.
fn grab(display: &SCDisplay, shows_cursor: bool) -> Result<CapturedFrame> {
    let frame = display.frame();
    let filter = SCContentFilter::create()
        .with_display(&display)
        .with_excluding_windows(&[])
        .build()
        .context("content filter")?;
    // `display.width()` is points — asking SCK for that size gives a
    // logical-res image (blurry on Retina). Request framebuffer pixels.
    let (pw, ph) = unsafe {
        let mode = CGDisplayCopyDisplayMode(display.display_id());
        let d = if mode.is_null() {
            (0, 0)
        } else {
            let d = (
                CGDisplayModeGetPixelWidth(mode),
                CGDisplayModeGetPixelHeight(mode),
            );
            CFRelease(mode.cast());
            d
        };
        if d.0 > 0 && d.1 > 0 {
            (d.0 as u32, d.1 as u32)
        } else {
            (display.width(), display.height())
        }
    };
    let config = SCStreamConfiguration::new()
        .with_width(pw)
        .with_height(ph)
        .with_shows_cursor(shows_cursor)
        .with_pixel_format(PixelFormat::BGRA);
    let image =
        SCScreenshotManager::capture_image(&filter, &config).context("screenshot capture")?;

    let (w, h) = (image.width() as u32, image.height() as u32);
    let pixels =
        RgbaImage::from_raw(w, h, image.rgba_data()?).context("captured image size mismatch")?;
    Ok(CapturedFrame {
        geometry: MonitorGeometry {
            origin: Point::new(frame.origin.x.round() as i32, frame.origin.y.round() as i32),
            size: Size::new(
                frame.size.width.round() as u32,
                frame.size.height.round() as u32,
            ),
            scale: w as f64 / frame.size.width,
        },
        pixels,
    })
}

/// Logical rect of a display's SCK frame — same space as `capture_all`.
fn display_rect(display: &SCDisplay) -> Rect {
    let f = display.frame();
    Rect::new(
        f.origin.x.round() as i32,
        f.origin.y.round() as i32,
        f.size.width.round() as u32,
        f.size.height.round() as u32,
    )
}

impl ScreenCapture for ScreenCaptureKit {
    fn capture_all(&self) -> Result<Vec<CapturedFrame>> {
        ensure_screen_permission()?;
        let content = SCShareableContent::get().context("shareable content")?;
        content.displays().iter().map(|d| grab(d, false)).collect()
    }

    /// Recording polls this ~15x/s — skip every display the selection
    /// doesn't touch; the cursor rides along.
    fn capture_monitor(&self, sel: Rect) -> Result<CapturedFrame> {
        ensure_screen_permission()?;
        let content = SCShareableContent::get().context("shareable content")?;
        let display = content
            .displays()
            .into_iter()
            .max_by_key(|d| {
                sel.intersection(&display_rect(d))
                    .map(|r| r.w as i64 * r.h as i64)
                    .unwrap_or(0)
            })
            .context("no display overlaps the selection")?;
        grab(&display, true)
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    /// The display's current mode carries the real framebuffer size
    /// (`CGDisplayPixelsWide` reports scaled-down values on Retina).
    fn CGDisplayCopyDisplayMode(display: u32) -> *mut std::ffi::c_void;
    fn CGDisplayModeGetPixelWidth(mode: *mut std::ffi::c_void) -> usize;
    fn CGDisplayModeGetPixelHeight(mode: *mut std::ffi::c_void) -> usize;
    fn CFRelease(cf: *mut std::ffi::c_void);
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
