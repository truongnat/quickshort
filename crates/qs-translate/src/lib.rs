//! Text translation over free, key-less endpoints — tried in order until one
//! answers with real text:
//!
//! 1. Google Translate `gtx` (the web UI's own endpoint; best quality when
//!    not rate-limited)
//! 2. MyMemory (`translated.net`; ~10k chars/day per IP anonymous)
//!
//! `source`/`target` are BCP-47 codes; `source` may be "auto". Blocking —
//! callers run on a worker thread (the resident app already dispatches
//! actions off the event loop).

use anyhow::{Context, Result};

const GTX: &str = "https://translate.googleapis.com/translate_a/single";
const MYMEMORY: &str = "https://api.mymemory.translated.net/get";

/// Translate `text` into `target`. Empty/whitespace input is a no-op.
/// Returns the last backend's error if every backend fails.
pub fn translate(text: &str, source: &str, target: &str) -> Result<String> {
    if text.trim().is_empty() {
        return Ok(String::new());
    }
    let mut errs = Vec::new();
    for f in [google_gtx, mymemory] {
        match f(text, source, target) {
            Ok(t) if !t.is_empty() => return Ok(t),
            Ok(_) => errs.push(anyhow::anyhow!("empty translation")),
            Err(e) => errs.push(e),
        }
    }
    Err(errs
        .into_iter()
        .last()
        .unwrap_or_else(|| anyhow::anyhow!("no backend")))
    .context("all translate backends failed")
}

/// `translate_a/single` replies `[[["translated","source",...],...],null,"en",..]`
/// — concatenate every chunk's first element.
fn google_gtx(text: &str, source: &str, target: &str) -> Result<String> {
    let url = format!(
        "{GTX}?client=gtx&sl={}&tl={}&dt=t&q={}",
        urlencode(source),
        urlencode(target),
        urlencode(text)
    );
    let body: serde_json::Value = ureq::get(&url)
        .header("User-Agent", "Mozilla/5.0")
        .call()
        .context("gtx request failed")?
        .body_mut()
        .read_json()
        .context("gtx reply is not JSON")?;
    let segs = body
        .get(0)
        .and_then(|v| v.as_array())
        .context("gtx reply: unexpected shape")?;
    let mut out = String::new();
    for seg in segs {
        if let Some(s) = seg.get(0).and_then(|v| v.as_str()) {
            out.push_str(s);
        }
    }
    Ok(out)
}

/// MyMemory wants an explicit `src|dst` pair; "auto" maps to its
/// `Autodetect` pseudo-language.
fn mymemory(text: &str, source: &str, target: &str) -> Result<String> {
    let src = if source == "auto" {
        "Autodetect"
    } else {
        source
    };
    let url = format!(
        "{MYMEMORY}?q={}&langpair={}%7C{}",
        urlencode(text),
        urlencode(src),
        urlencode(target)
    );
    let body: serde_json::Value = ureq::get(&url)
        .call()
        .context("mymemory request failed")?
        .body_mut()
        .read_json()
        .context("mymemory reply is not JSON")?;
    if body.get("responseStatus").and_then(|v| v.as_u64()) != Some(200) {
        let detail = body
            .get("responseDetails")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        anyhow::bail!("mymemory rejected the request: {detail}");
    }
    body.get("responseData")
        .and_then(|d| d.get("translatedText"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .context("mymemory reply: no translatedText")
}

/// RFC 3986 percent-encoding (UTF-8 bytes, unreserved set kept verbatim).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_reserved_and_utf8() {
        assert_eq!(urlencode("a b&c"), "a%20b%26c");
        assert_eq!(urlencode("xin chào"), "xin%20ch%C3%A0o");
        assert_eq!(urlencode("~ok"), "~ok");
    }

    #[test]
    fn empty_is_noop() {
        assert_eq!(translate("   ", "auto", "vi").unwrap(), "");
    }

    /// Live network check — run explicitly:
    /// `cargo test -p qs-translate -- --ignored`
    #[test]
    #[ignore]
    fn live_en_to_vi() {
        let t = translate("hello world", "auto", "vi").unwrap();
        assert!(t.to_lowercase().contains("chào"), "got {t:?}");
    }
}
