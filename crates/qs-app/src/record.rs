//! Screen recording: PipeWire frames → crop → rawvideo stdin → ffmpeg
//! mp4. Runs on a worker thread; the UI only holds the stop flag and a
//! RecStop window.

use anyhow::{bail, Result};
use image::RgbaImage;
use qs_core::Rect;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[cfg(target_os = "linux")]
use anyhow::Context;
#[cfg(target_os = "linux")]
use std::io::Write;
#[cfg(target_os = "linux")]
use std::process::{Child, Command, Stdio};
#[cfg(target_os = "linux")]
use std::sync::atomic::Ordering;
#[cfg(target_os = "linux")]
use std::sync::Mutex;
#[cfg(target_os = "linux")]
use std::thread;
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
const FPS: u64 = 15;

/// Frames → mp4 in `dir` until `stop` flips. Returns (first cropped
/// frame, path) so the UI can show a thumbnail without re-reading.
#[cfg(target_os = "linux")]
pub fn run(sel: Rect, dir: &Path, stop: Arc<AtomicBool>) -> Result<(RgbaImage, PathBuf)> {
    let live = qs_capture::record_start(sel)?;
    let crop = crop_px(sel, live.logical, live.scale);
    let mut enc = Encoder::spawn(dir, crop.2, crop.3)?;
    let latest = Arc::new(Mutex::new(None::<RgbaImage>));
    let fwd = {
        let latest = latest.clone();
        // channel stays drained so the pw thread never backs up; the
        // encoder only ever sees the newest frame
        thread::spawn(move || {
            while let Ok(img) = live.frames().recv() {
                *latest.lock().unwrap() = Some(img);
            }
        })
    };

    let mut first = None;
    let mut last = None::<RgbaImage>;
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(1000 / FPS));
        let img = latest.lock().unwrap().take().or(last.clone());
        let Some(img) = img else { continue };
        let frame = crop_frame(&img, crop);
        if first.is_none() {
            first = Some(frame.clone());
        }
        enc.write(&frame)?;
        last = Some(frame);
    }
    let path = enc.finish()?;
    drop(fwd);
    match first {
        Some(f) => Ok((f, path)),
        None => bail!("no frames captured before stop"),
    }
}

#[cfg(not(target_os = "linux"))]
pub fn run(_sel: Rect, _dir: &Path, _stop: Arc<AtomicBool>) -> Result<(RgbaImage, PathBuf)> {
    bail!("screen recording is only implemented on Linux/Wayland for now")
}

/// Selection (logical) → pixel rect inside the monitor's frames.
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
fn crop_frame(img: &RgbaImage, (x, y, w, h): (u32, u32, u32, u32)) -> RgbaImage {
    let x = x.min(img.width().saturating_sub(w));
    let y = y.min(img.height().saturating_sub(h));
    image::imageops::crop_imm(img, x, y, w.min(img.width()), h.min(img.height())).to_image()
}

#[cfg(target_os = "linux")]
#[cfg(target_os = "linux")]
struct Encoder {
    child: Child,
    path: PathBuf,
}

#[cfg(target_os = "linux")]
impl Encoder {
    fn spawn(dir: &Path, w: u32, h: u32) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!(
            "quickshort-{}.mp4",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs()
        ));
        let child = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-s",
                &format!("{w}x{h}"),
                "-r",
                &FPS.to_string(),
                "-i",
                "pipe:0",
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
        Ok(Self { child, path })
    }

    fn write(&mut self, frame: &RgbaImage) -> Result<()> {
        self.child
            .stdin
            .as_mut()
            .context("ffmpeg stdin closed")?
            .write_all(frame.as_raw())
            .context("write frame to ffmpeg — encoder died?")
    }

    /// Close stdin so ffmpeg can finalize, then require success.
    fn finish(mut self) -> Result<PathBuf> {
        drop(self.child.stdin.take());
        let st = self.child.wait().context("wait ffmpeg")?;
        if !st.success() {
            let _ = std::fs::remove_file(&self.path);
            bail!("ffmpeg exited with {st}");
        }
        eprintln!("recorded {}", self.path.display());
        Ok(self.path)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// Real PipeWire → ffmpeg round-trip: ~3s of the primary monitor into
    /// /tmp, then ffprobe validates the container. Needs a live session.
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
            .args(["-v", "error", "-show_entries", "format=duration", "-of",
                   "csv=p=0"])
            .arg(&path)
            .output()
            .unwrap();
        eprintln!("ffprobe: {}", String::from_utf8_lossy(&out.stdout));
        assert!(out.status.success() && out.stdout[0].is_ascii_digit());
    }
}
