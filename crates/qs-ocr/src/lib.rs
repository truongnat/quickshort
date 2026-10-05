use anyhow::Result;
use image::RgbaImage;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// Extract text from an image using the platform-native OCR engine.
/// Returns recognized text with lines separated by '\n'.
/// `lang` is a hint for engines that take a language list (tesseract's
/// `eng+jpn` syntax); native engines auto-detect and ignore it.
pub fn extract_text(img: &RgbaImage, lang: &str) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        let _ = lang;
        macos::extract(img)
    }
    #[cfg(target_os = "windows")]
    {
        let _ = lang;
        windows::extract(img)
    }
    #[cfg(target_os = "linux")]
    {
        linux::extract(img, lang)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = img;
        anyhow::bail!("ocr not implemented for this platform")
    }
}
