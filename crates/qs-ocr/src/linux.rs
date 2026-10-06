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

    let lang = if lang.is_empty() || lang == "auto" {
        detect_lang(&png)?
    } else {
        lang.to_string()
    };
    run_tesseract(&png, &["-l", &lang])
}

/// Run tesseract with extra args between `stdin stdout`; returns stdout.
/// `timeout` wrapper so a pathological frame can never hang the action.
fn run_tesseract(png: &[u8], extra: &[&str]) -> Result<String> {
    let mut args = vec!["60", "tesseract"];
    args.extend_from_slice(extra);
    args.extend_from_slice(&["stdin", "stdout"]);
    let mut child = Command::new("timeout")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("tesseract not found — `sudo apt install tesseract-ocr`")?;

    // Write from a thread: the pipe buffer is smaller than a PNG frame and
    // wait_with_output() can't run until stdin is closed.
    let png = png.to_vec();
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

/// `tesseract --list-langs` → e.g. ["eng", "jpn"] ("osd" filtered out).
/// Also feeds the Settings badges via qs_ocr::installed_langs.
pub fn installed() -> Vec<String> {
    Command::new("tesseract")
        .arg("--list-langs")
        .stderr(Stdio::null())
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .skip(1) // "List of available languages..."
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty() && l != "osd")
                .collect()
        })
        .unwrap_or_default()
}

/// One OSD pass: returns the dominant script name ("Latin", "Han",
/// "Japanese", ...) or None when the crop is too small to tell.
fn detect_script(png: &[u8]) -> Option<String> {
    let out = run_tesseract(png, &["-l", "osd", "--psm", "0"]).ok()?;
    out.lines().find_map(|l| {
        l.strip_prefix("Script:")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && *s != "NULL")
    })
}

/// Pick a `-l` value from the detected script and the installed packs.
/// Script names come from tesseract's osd traineddata.
fn detect_lang(png: &[u8]) -> Result<String> {
    let installed = installed();
    let has = |l: &str| installed.iter().any(|i| i == l);

    let wanted = match detect_script(png).as_deref() {
        // osd reports "Japanese" only when kana is present; pure kanji is
        // "Han" — prefer jpn (its traineddata covers kanji) over chi_sim.
        Some("Han") => ["jpn", "chi_sim", "chi_tra"]
            .into_iter()
            .find(|l| has(l))
            .unwrap_or("jpn"),
        Some("Japanese") => "jpn",
        Some("Hangul") | Some("Korean") => "kor",
        Some("Cyrillic") => "rus",
        Some("Arabic") => "ara",
        Some("Devanagari") => "hin",
        Some("Thai") => "tha",
        Some("Greek") => "ell",
        Some("Hebrew") => "heb",
        Some("Latin") => "eng",
        // OSD failed (tiny crop) or unknown script: use everything
        // installed so a user who added jpn gets it automatically.
        _ => {
            return Ok(if installed.is_empty() {
                "eng".to_string()
            } else {
                installed.join("+")
            });
        }
    };

    if wanted != "eng" && !has(wanted) {
        anyhow::bail!(
            "detected {wanted} text but its language pack is missing — \
             sudo apt install tesseract-ocr-{wanted}"
        );
    }
    // mix with eng so UI text/labels inside CJK still read correctly
    Ok(if wanted != "eng" && has("eng") {
        format!("{wanted}+eng")
    } else {
        wanted.to_string()
    })
}
