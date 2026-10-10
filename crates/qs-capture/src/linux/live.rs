//! Live one-monitor screencast, dispatched on the session kind: GNOME
//! mutter gets its private push API, other Wayland compositors the
//! xdg-desktop-portal stream, X11 a re-capture polling loop.

use anyhow::Result;
use image::RgbaImage;
use qs_core::Rect;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread::JoinHandle;

use super::{portal, session_kind, wayland, SessionKind};

/// Live screencast of one monitor — full-monitor RGBA frames arrive on
/// `rx` until `stop` fires. Dropping stops the pump thread, joins it,
/// then runs the backend's session teardown.
pub struct LiveStream {
    /// monitor rect in virtual logical coords — the app crops `sel` out
    /// of each frame with `scale` (physical px per logical px)
    pub logical: Rect,
    pub scale: f64,
    rx: Receiver<RgbaImage>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    /// backend session cleanup (mutter Stop / portal Session Close /
    /// pipewire connection) — runs after the pump thread is joined so
    /// the producer outlives its consumer
    on_stop: Option<Box<dyn FnOnce() + Send>>,
}

impl LiveStream {
    pub(crate) fn new(
        logical: Rect,
        scale: f64,
        rx: Receiver<RgbaImage>,
        stop: Arc<AtomicBool>,
        handle: JoinHandle<()>,
        on_stop: Option<Box<dyn FnOnce() + Send>>,
    ) -> Self {
        Self {
            logical,
            scale,
            rx,
            stop,
            handle: Some(handle),
            on_stop,
        }
    }

    pub fn frames(&self) -> &Receiver<RgbaImage> {
        &self.rx
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for LiveStream {
    fn drop(&mut self) {
        self.stop();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        if let Some(f) = self.on_stop.take() {
            f();
        }
    }
}

/// Screencast of the monitor overlapping `sel` the most.
pub fn record_start(sel: Rect) -> Result<LiveStream> {
    match session_kind()? {
        SessionKind::Mutter => wayland::record_start(sel),
        SessionKind::Portal => portal::record_start(sel),
        SessionKind::X11 => {
            // X11 has no push frame API — the poll worker doubles as the
            // producer and needs no session teardown.
            let p = crate::poll::start(sel)?;
            Ok(LiveStream {
                logical: p.logical,
                scale: p.scale,
                rx: p.rx,
                stop: p.stop,
                handle: p.handle,
                on_stop: None,
            })
        }
    }
}
