//! Screen recording: monitor frames (PipeWire push on Wayland, re-capture
//! polling on macOS/Windows) → crop → rawvideo stdin → ffmpeg mp4. Runs on
//! a worker thread; the UI only holds the stop flag and a RecStop window.

use anyhow::{bail, Context, Result};
use image::RgbaImage;
use qs_core::Rect;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

const FPS: u64 = 15;

/// Frames → mp4 in `dir` until `stop` flips. Returns (first cropped
/// frame, path) so the UI can show a thumbnail without re-reading.
pub fn run(sel: Rect, dir: &Path, stop: Arc<AtomicBool>) -> Result<(RgbaImage, PathBuf)> {
    let live = qs_capture::record_start(sel)?;
    let crop = crop_px(sel, live.logical, live.scale);
    let enc = Encoder::spawn(dir, crop.2, crop.3)?;
    let rx = live.frames();

    let mut first = None;
    let mut last = None::<RgbaImage>;
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(1000 / FPS));
        // drain the queue so only the newest frame counts — stale frames
        // are worse than a repeat
        let mut newest = None;
        while let Ok(img) = rx.try_recv() {
            newest = Some(img);
        }
        let frame = match newest {
            Some(img) => {
                let f = crop_frame(&img, crop);
                // a short frame would desync the rawvideo stream — ffmpeg
                // was told exactly crop.2×crop.3, so bail instead of
                // writing a corrupt packet
                if f.width() != crop.2 || f.height() != crop.3 {
                    bail!(
                        "frame is {}x{} but crop needs {}x{}",
                        f.width(),
                        f.height(),
                        crop.2,
                        crop.3
                    );
                }
                if first.is_none() {
                    first = Some(f.clone());
                }
                last = Some(f.clone());
                f
            }
            // no fresh frame — repeat the previous one to keep wall time
            None => match &last {
                Some(f) => f.clone(),
                None => continue,
            },
        };
        if enc.send(frame).is_err() {
            break; // encoder died — finish() below surfaces the cause
        }
    }
    drop(live); // stops the producer and joins its thread before encode-wrap
    let path = enc.finish()?;
    match first {
        Some(f) => Ok((f, path)),
        None => bail!("no frames captured before stop"),
    }
}

/// Selection (logical) → pixel rect inside the monitor's frames.
fn crop_px(sel: Rect, mon: Rect, scale: f64) -> (u32, u32, u32, u32) {
    let i = sel
        .intersection(&mon)
        .unwrap_or(Rect::new(mon.x, mon.y, 1, 1));
    let w = ((i.w as f64 * scale) as u32).max(2) & !1;
    let h = ((i.h as f64 * scale) as u32).max(2) & !1;
    (
        ((i.x - mon.x).max(0) as f64 * scale) as u32,
        ((i.y - mon.y).max(0) as f64 * scale) as u32,
        w,
        h,
    )
}

/// Fixed-size crop — ffmpeg declared exactly (w,h), so an overhang near
/// the monitor edge shifts the origin instead of shrinking the frame.
fn crop_frame(img: &RgbaImage, (x, y, w, h): (u32, u32, u32, u32)) -> RgbaImage {
    let x = x.min(img.width().saturating_sub(w));
    let y = y.min(img.height().saturating_sub(h));
    image::imageops::crop_imm(img, x, y, w.min(img.width()), h.min(img.height())).to_image()
}

/// ffmpeg runs on a worker behind a bounded queue: the tick loop stays at
/// 15fps and drops frames when the encode lags — wallclock input pts keep
/// the mp4's duration honest regardless.
struct Encoder {
    tx: SyncSender<RgbaImage>,
    worker: JoinHandle<Result<PathBuf>>,
}

impl Encoder {
    fn spawn(dir: &Path, w: u32, h: u32) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!(
            "quickshort-{}.mp4",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs()
        ));
        // no input -r: wallclock pts stamps each packet when ffmpeg reads
        // it, so mp4 duration tracks real time no matter the feed rate;
        // fps=15 resamples to CFR for a normal container
        let mut child = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-use_wallclock_as_timestamps",
                "1",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-s",
                &format!("{w}x{h}"),
                "-i",
                "pipe:0",
                "-vf",
                &format!("fps={FPS}"),
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-crf",
                "23",
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "+faststart",
            ])
            .arg(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn ffmpeg (is it installed?)")?;
        let (tx, rx) = sync_channel::<RgbaImage>(2);
        let worker = thread::spawn(move || {
            let mut stdin = child.stdin.take().context("ffmpeg stdin")?;
            while let Ok(frame) = rx.recv() {
                stdin
                    .write_all(frame.as_raw())
                    .context("write frame to ffmpeg — encoder died?")?;
            }
            drop(stdin); // EOF -> ffmpeg finalizes the mp4
            let st = child.wait().context("wait ffmpeg")?;
            if !st.success() {
                let _ = std::fs::remove_file(&path);
                bail!("ffmpeg exited with {st}");
            }
            eprintln!("recorded {}", path.display());
            Ok(path)
        });
        Ok(Self { tx, worker })
    }

    /// Queue one frame — a full queue drops it, which is harmless: the
    /// wallclock pts + fps filter stretch the previous frame across the
    /// gap, so duration and freshness both survive a slow encode. Err
    /// means the worker is gone; `finish` has the cause.
    fn send(&self, frame: RgbaImage) -> Result<()> {
        match self.tx.try_send(frame) {
            Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
            Err(TrySendError::Disconnected(_)) => bail!("encoder thread is gone"),
        }
    }

    /// Close the queue (worker EOFs ffmpeg), then take the worker's result.
    fn finish(self) -> Result<PathBuf> {
        let Self { tx, worker } = self;
        drop(tx);
        worker
            .join()
            .map_err(|_| anyhow::anyhow!("encoder thread panicked"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real capture → ffmpeg round-trip: ~3s of the primary monitor into
    /// /tmp, then ffprobe validates the container. Needs a live session,
    /// screen-recording permission and ffmpeg on PATH:
    /// `cargo test -p qs-app records_a_real_mp4 -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn records_a_real_mp4() {
        let mon = qs_capture::backend()
            .unwrap()
            .capture_all()
            .unwrap()
            .remove(0)
            .geometry
            .logical_rect();
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(3));
            s2.store(true, Ordering::Relaxed);
        });
        let (_thumb, path) = run(mon, Path::new("/tmp"), stop).unwrap();
        let out = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "csv=p=0",
            ])
            .arg(&path)
            .output()
            .unwrap();
        eprintln!("ffprobe: {}", String::from_utf8_lossy(&out.stdout));
        assert!(out.status.success() && out.stdout[0].is_ascii_digit());
    }
}
