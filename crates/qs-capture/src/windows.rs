//! Windows backend — placeholder.
//!
//! Real implementation: mark the process PER_MONITOR_AWARE_V2, enumerate
//! monitors via EnumDisplayMonitors (virtual coords in logical px, scale from
//! GetDpiForMonitor), then BitBlt each monitor's rect from the screen DC.
//! DXGI Desktop Duplication only if GDI proves too slow.

use anyhow::{bail, Result};
use qs_core::CapturedFrame;

use crate::ScreenCapture;

pub struct GdiCapture;

impl ScreenCapture for GdiCapture {
    fn capture_all(&self) -> Result<Vec<CapturedFrame>> {
        bail!("windows backend not implemented yet")
    }
}
