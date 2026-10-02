use anyhow::Result;
use image::RgbaImage;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// Extract text from an image using the platform-native OCR engine.
/// Returns recognized text with lines separated by '\n'.
pub fn extract_text(img: &RgbaImage) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        macos::extract(img)
    }
    #[cfg(target_os = "windows")]
    {
        windows::extract(img)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = img;
        anyhow::bail!("ocr not implemented for this platform")
    }
}
