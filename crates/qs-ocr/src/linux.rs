//! Linux OCR via the `tesseract` binary (package `tesseract-ocr`).
//! No FFI — pipe a PNG to its stdin, read text from stdout.

use anyhow::{Context, Result};
use image::RgbaImage;
use std::io::Write;
use std::process::{Command, Stdio};

pub fn extract(img: &RgbaImage, lang: &str) -> Result<String> {
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png)
        .context("encode png for tesseract")?;
    let png = png.into_inner();

    // `timeout` so a pathological frame can never hang the action
    let mut child = Command::new("timeout")
        .args([
            "60",
            "tesseract",
            "-l",
            if lang.is_empty() { "eng" } else { lang },
            "stdin",
            "stdout",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("tesseract not found — `sudo apt install tesseract-ocr`")?;

    // Write from a thread: the pipe buffer is smaller than a PNG frame and
    // wait_with_output() can't run until stdin is closed.
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(&png));
    let out = child.wait_with_output()?;
    let _ = writer.join();
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stderr = stderr.trim();
        anyhow::bail!(
            "tesseract failed{}",
            if stderr.is_empty() {
                format!(" ({})", out.status)
            } else {
                format!(": {stderr}")
            }
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
