use anyhow::Result;

use crate::ScreenCapture;

mod portal;
mod wayland;
mod x11;

/// Wayland has no global virtual coordinate space exposed to clients; the
/// mutter backend gets the real layout from DisplayConfig instead.
/// Non-GNOME compositors (KDE, wlroots) go through xdg-desktop-portal —
/// QS_CAPTURE=portal|mutter|x11 overrides the auto pick.
pub fn backend() -> Result<Box<dyn ScreenCapture>> {
    if let Ok(force) = std::env::var("QS_CAPTURE") {
        return match force.as_str() {
            "portal" => Ok(Box::new(portal::PortalCapture)),
            "mutter" => Ok(Box::new(wayland::MutterCapture)),
            "x11" => Ok(Box::new(x11::X11Capture)),
            other => anyhow::bail!("QS_CAPTURE={other}: expected portal|mutter|x11"),
        };
    }
    match std::env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("wayland") => {
            let gnome = std::env::var("XDG_CURRENT_DESKTOP")
                .unwrap_or_default()
                .to_ascii_uppercase()
                .split(':')
                .any(|d| d.trim() == "GNOME");
            if gnome {
                Ok(Box::new(wayland::MutterCapture))
            } else {
                Ok(Box::new(portal::PortalCapture))
            }
        }
        _ => Ok(Box::new(x11::X11Capture)),
    }
}
