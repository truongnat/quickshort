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
/// `lang` uses tesseract's `eng+vie` syntax; "auto"/"" lets the engine
/// detect. Windows.Media.Ocr always auto-detects and ignores the hint.
pub fn extract_text(img: &RgbaImage, lang: &str) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        macos::extract(img, lang)
    }
    #[cfg(target_os = "windows")]
    {
        let _ = lang;
        windows::extract(img)
    }
    #[cfg(target_os = "linux")]
    {
        linux::extract(img, lang).map(|t| tidy(&t))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = img;
        anyhow::bail!("ocr not implemented for this platform")
    }
}

/// Tesseract tokenizes CJK into space-separated "words", but real CJK
/// text has no spaces — drop a space only when BOTH neighbours are
/// non-ASCII, so Latin lines and mixed words ('café bar') stay intact.
#[cfg(target_os = "linux")]
fn tidy(text: &str) -> String {
    text.lines()
        .map(|l| {
            let c: Vec<char> = l.chars().collect();
            let mut out = String::with_capacity(l.len());
            for (i, ch) in c.iter().enumerate() {
                let squash = *ch == ' '
                    && i > 0
                    && !c[i - 1].is_ascii()
                    && i + 1 < c.len()
                    && !c[i + 1].is_ascii();
                if !squash {
                    out.push(*ch);
                }
            }
            out
        })
        .collect::<Vec<_>>()
        .join("\n")
}
