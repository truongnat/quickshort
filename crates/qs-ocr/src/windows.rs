//! Windows OCR via the built-in WinRT Windows.Media.Ocr engine — no bundled
//! model. "auto" uses the user's profile languages; an explicit Settings pick
//! resolves to a language pack via TryCreateFromLanguage.

use anyhow::{Context, Result};
use image::RgbaImage;
use windows::core::HSTRING;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;

/// tesseract-style codes -> BCP-47 tags the OS language packs use.
/// Mirrors macos.rs; unknown codes are skipped.
fn bcp47(code: &str) -> Option<&'static str> {
    Some(match code.trim() {
        "eng" | "en" | "en-US" => "en-US",
        "vie" | "vi" | "vi-VN" => "vi-VN",
        "jpn" | "ja" | "ja-JP" => "ja-JP",
        "kor" | "ko" | "ko-KR" => "ko-KR",
        "chi_sim" | "zh-Hans" | "zh-CN" => "zh-Hans",
        "chi_tra" | "zh-Hant" | "zh-TW" => "zh-Hant",
        "fra" | "fr" | "fr-FR" => "fr-FR",
        "deu" | "de" | "de-DE" => "de-DE",
        "spa" | "es" | "es-ES" => "es-ES",
        "tha" | "th" | "th-TH" => "th-TH",
        "ind" | "id" | "id-ID" => "id-ID",
        _ => return None,
    })
}

pub fn extract(img: &RgbaImage, lang: &str) -> Result<String> {
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

    // OcrEngine is single-language — the first listed code wins
    let tag = lang
        .split(|c| c == '+' || c == ',' || c == ' ')
        .find_map(bcp47);
    let engine = match tag {
        Some(tag) => {
            let l = Language::CreateLanguage(&HSTRING::from(tag))?;
            OcrEngine::TryCreateFromLanguage(&l).with_context(|| {
                format!("OCR language {tag} is not installed — Settings > Language & region")
            })?
        }
        None => OcrEngine::TryCreateFromUserProfileLanguages()
            .context("no OCR language pack installed")?,
    };
    let res = engine.RecognizeAsync(&bitmap)?.join()?;
    Ok(res.Text()?.to_string_lossy())
}
