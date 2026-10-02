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
