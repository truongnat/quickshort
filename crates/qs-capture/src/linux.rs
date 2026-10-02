use anyhow::Result;

use crate::ScreenCapture;

mod wayland;
mod x11;

/// Wayland has no global virtual coordinate space exposed to clients; the
/// mutter backend gets the real layout from DisplayConfig instead.
pub fn backend() -> Result<Box<dyn ScreenCapture>> {
    match std::env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("wayland") => Ok(Box::new(wayland::MutterCapture)),
        _ => Ok(Box::new(x11::X11Capture)),
    }
}
