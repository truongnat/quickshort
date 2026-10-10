use anyhow::Result;

use crate::ScreenCapture;

pub mod live;
mod portal;
pub mod wayland;
mod x11;

/// Which compositor/session family owns the display — the capture backend
/// and `record_start` both pick from this. Wayland has no global virtual
/// coordinate space exposed to clients; the mutter backend gets the real
/// layout from DisplayConfig instead. Non-GNOME compositors (KDE, wlroots)
/// go through xdg-desktop-portal — QS_CAPTURE=portal|mutter|x11 overrides
/// the auto pick.
#[derive(Clone, Copy)]
pub enum SessionKind {
    Mutter,
    Portal,
    X11,
}

pub fn session_kind() -> Result<SessionKind> {
    if let Ok(force) = std::env::var("QS_CAPTURE") {
        return match force.as_str() {
            "portal" => Ok(SessionKind::Portal),
            "mutter" => Ok(SessionKind::Mutter),
            "x11" => Ok(SessionKind::X11),
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
                Ok(SessionKind::Mutter)
            } else {
                Ok(SessionKind::Portal)
            }
        }
        _ => Ok(SessionKind::X11),
    }
}

/// Pick the right backend for the current platform/session.
pub fn backend() -> Result<Box<dyn ScreenCapture>> {
    match session_kind()? {
        SessionKind::Mutter => Ok(Box::new(wayland::MutterCapture)),
        SessionKind::Portal => Ok(Box::new(portal::PortalCapture)),
        SessionKind::X11 => Ok(Box::new(x11::X11Capture)),
    }
}
