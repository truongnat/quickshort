//! Linux OCR via the `tesseract` binary (package `tesseract-ocr`).
//! No FFI — pipe a PNG to its stdin, read text from stdout.

use anyhow::{Context, Result};
use image::RgbaImage;
use std::io::Write;
use std::process::{Command, Stdio};

pub fn extract(img: &RgbaImage) -> Result<String> {
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png)
        .context("encode png for tesseract")?;
    let png = png.into_inner();

    let mut child = Command::new("tesseract")
        .args(["stdin", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("tesseract not found — `sudo apt install tesseract-ocr`")?;

    // Write from a thread: the pipe buffer is smaller than a PNG frame and
    // wait_with_output() can't run until stdin is closed.
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(&png));
    let out = child.wait_with_output()?;
    let _ = writer.join();
    if !out.status.success() {
        anyhow::bail!("tesseract exited {}", out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
