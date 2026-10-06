use anyhow::Result;
use image::RgbaImage;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// OCR languages offered on the Settings page — (tessdata code, display
/// name, lowercase subtag prefix matching the OS engines' BCP-47 tags
/// back to this code). `Config.ocr_lang` stores codes joined by "+",
/// or "auto" for engine auto-detection.
pub const LANGS: &[(&str, &str, &str)] = &[
    ("eng", "English", "en"),
    ("vie", "Vietnamese", "vi"),
    ("jpn", "Japanese", "ja"),
    ("kor", "Korean", "ko"),
    ("chi_sim", "Chinese (Simplified)", "zh-hans"),
    ("chi_tra", "Chinese (Traditional)", "zh-hant"),
    ("fra", "French", "fr"),
    ("deu", "German", "de"),
    ("spa", "Spanish", "es"),
    ("por", "Portuguese", "pt"),
    ("rus", "Russian", "ru"),
    ("tha", "Thai", "th"),
];

/// Parse an ocr_lang value ("auto" or "eng+jpn") into tessdata codes.
/// An empty vec means auto-detection.
pub fn lang_codes(ocr_lang: &str) -> Vec<String> {
    ocr_lang
        .split('+')
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "auto")
        .map(str::to_owned)
        .collect()
}

/// tessdata codes the platform engine can use today — feeds the
/// "not installed" badges on the Settings OCR page.
pub fn installed_langs() -> Vec<String> {
    #[cfg(target_os = "linux")]
    {
        linux::installed()
    }
    #[cfg(target_os = "macos")]
    {
        canonicalize(macos::installed())
    }
    #[cfg(target_os = "windows")]
    {
        canonicalize(windows::installed())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Vec::new()
    }
}

/// OS language tags (BCP-47) → canonical tessdata codes from LANGS.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn canonicalize(tags: Vec<String>) -> Vec<String> {
    let mut v: Vec<String> = tags
        .iter()
        .filter_map(|t| canonical(t).map(str::to_owned))
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// A tag covers a LANGS prefix on exact match or as a leading subtag
/// ("en-GB" → en, "zh-Hant-HK" → zh-hant).
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn canonical(tag: &str) -> Option<&'static str> {
    let t = tag.to_lowercase().replace('_', "-");
    LANGS
        .iter()
        .find(|e| t == e.2 || t.starts_with(&format!("{}-", e.2)))
        .map(|e| e.0)
}

/// Extract text from an image using the platform-native OCR engine.
/// Returns recognized text with lines separated by '\n'.
/// `lang` uses tesseract's `eng+vie` syntax; "auto"/"" lets the engine
/// detect. Windows.Media.Ocr is single-language — the first code wins.
pub fn extract_text(img: &RgbaImage, lang: &str) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        macos::extract(img, lang)
    }
    #[cfg(target_os = "windows")]
    {
        windows::extract(img, lang)
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
