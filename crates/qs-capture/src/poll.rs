//! Polling screencast — the `record_start` fallback for sessions with no
//! push frame API wired in (macOS, Windows, X11). A worker thread
//! re-captures the monitor holding `sel` and ships each grab as an
//! RgbaImage. The encoder ticks at a fixed rate and repeats the newest
//! frame, so capture latency only lowers the effective fps, never the
//! wall-clock length.

use anyhow::{Context, Result};
use image::RgbaImage;
use qs_core::Rect;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Poll cadence — matches the encoder's 15fps tick.
const PERIOD: Duration = Duration::from_millis(66);

/// The running poll pump, minus lifecycle glue — `PollStream`
/// (macOS/Windows) and `LiveStream` (Linux/X11) each wrap it with their
/// own Drop teardown, so these fields move out freely (no Drop here).
pub(crate) struct Parts {
    /// monitor rect in virtual logical coords
    pub logical: Rect,
    /// physical px per logical px
    pub scale: f64,
    pub rx: Receiver<RgbaImage>,
    pub stop: Arc<AtomicBool>,
    pub handle: Option<JoinHandle<()>>,
}

/// The first grab resolves the target monitor (and fails fast before the
/// UI shows REC); the worker then polls until `stop` or a capture error.
pub(crate) fn start(sel: Rect) -> Result<Parts> {
    let first = crate::backend()?
        .capture_monitor(sel)
        .context("record: first frame")?;
    let logical = first.geometry.logical_rect();
    let scale = first.geometry.scale;
    let (tx, rx) = sync_channel::<RgbaImage>(2);
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let handle = std::thread::spawn(move || {
        let _ = tx.send(first.pixels);
        // the trait object isn't Send — build a backend inside the thread
        let backend = match crate::backend() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("record poll: {e:#}");
                return;
            }
        };
        while !stop2.load(Ordering::Relaxed) {
            let t0 = Instant::now();
            match backend.capture_monitor(sel) {
                // a full queue drops the frame — realtime beats completeness
                Ok(f) => {
                    let _ = tx.try_send(f.pixels);
                }
                Err(e) => {
                    eprintln!("record poll: {e:#}");
                    break;
                }
            }
            if let Some(rest) = PERIOD.checked_sub(t0.elapsed()) {
                std::thread::sleep(rest);
            }
        }
    });
    Ok(Parts {
        logical,
        scale,
        rx,
        stop,
        handle: Some(handle),
    })
}

/// Same contract as the Wayland `LiveStream`: full-monitor frames arrive
/// on `rx`, `logical`+`scale` describe the rect the app crops `sel` from.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub struct PollStream {
    /// monitor rect in virtual logical coords
    pub logical: Rect,
    /// physical px per logical px
    pub scale: f64,
    rx: Receiver<RgbaImage>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl PollStream {
    pub fn frames(&self) -> &Receiver<RgbaImage> {
        &self.rx
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Drop for PollStream {
    fn drop(&mut self) {
        self.stop();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// The first grab resolves the target monitor (and fails fast before the
/// UI shows REC); the worker then polls until `stop` or a capture error.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn record_start(sel: Rect) -> Result<PollStream> {
    let p = start(sel)?;
    Ok(PollStream {
        logical: p.logical,
        scale: p.scale,
        rx: p.rx,
        stop: p.stop,
        handle: p.handle,
    })
}
