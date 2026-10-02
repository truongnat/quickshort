//! Windows OCR via the built-in WinRT Windows.Media.Ocr engine — no bundled
//! model, uses the user's installed profile languages.

use anyhow::{Context, Result};
use image::RgbaImage;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;

pub fn extract(img: &RgbaImage) -> Result<String> {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let mut bgra = img.as_raw().clone();
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }

    let writer = DataWriter::new()?;
    writer.WriteBytes(&bgra)?;
    let buf = writer.DetachBuffer()?;

    let bitmap = SoftwareBitmap::Create(BitmapPixelFormat::Bgra8, w, h)?;
    bitmap.CopyToBuffer(&buf)?;

    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .context("no OCR language pack installed")?;
    let res = engine.RecognizeAsync(&bitmap)?.join()?;
    Ok(res.Text()?.to_string_lossy())
}
