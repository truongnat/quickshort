//! macOS backend — placeholder.
//!
//! Real implementation: ScreenCaptureKit (SCShareableContent -> per-display
//! streams) or CGWindowListCreateImage per display rect. Logical rects come
//! from NSScreen.frame, scale from NSScreen.backingScaleFactor (2.0 Retina).

use anyhow::{bail, Result};
use qs_core::CapturedFrame;

use crate::ScreenCapture;

pub struct ScreenCaptureKit;

impl ScreenCapture for ScreenCaptureKit {
    fn capture_all(&self) -> Result<Vec<CapturedFrame>> {
        bail!("macos backend not implemented yet")
    }
}
