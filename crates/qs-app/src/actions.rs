//! Post-selection action dispatch — shared by the one-shot CLI path and the
//! resident tray app.

use anyhow::{Context, Result};
use image::RgbaImage;
use qs_overlay::{Action, Annotation};
use std::path::{Path, PathBuf};

/// Where a `Save` lands: an explicit output file (`--out`) or the default
/// screenshots dir with an auto-generated timestamped name.
pub enum SaveTarget {
    File(PathBuf),
    Dir(PathBuf),
}

/// What an action produced — the resident app mirrors this into its toast.
pub enum Outcome {
    Saved(PathBuf),
    Copied,
    /// chars of extracted text put on the clipboard
    TextCopied(usize),
    /// chars of translated text put on the clipboard
    Translated(usize),
    /// translation failed — the raw OCR text was copied instead
    TranslateFailed(usize),
    /// OCR ran but found no text
    NoText,
}

/// Stitch the selection, bake annotations, run the action. `wait_clipboard`:
/// on Linux/Wayland the clipboard dies with its owner — the one-shot binary
/// must block (`true`); the resident app stays alive so it passes `false`.
pub fn dispatch(
    frames: Vec<qs_core::CapturedFrame>,
    sel: qs_core::Rect,
    action: Action,
    annots: &[Annotation],
    target: &SaveTarget,
    cfg: &crate::config::Config,
    wait_clipboard: bool,
) -> Result<Outcome> {
    let img = std::thread::spawn(move || qs_core::stitch(&frames, sel))
        .join()
        .expect("stitch worker panicked");
    let Some(mut img) = img else {
        anyhow::bail!("selection {sel:?} covers no monitor");
    };
    qs_overlay::annotate_image(&mut img, sel, annots);

    match action {
        Action::Save => save(&img, target).map(Outcome::Saved),
        Action::Copy => {
            copy_image_to_clipboard(&img, wait_clipboard);
            Ok(Outcome::Copied)
        }
        Action::Ocr => {
            let t = qs_ocr::extract_text(&img)?;
            if t.is_empty() {
                return Ok(Outcome::NoText);
            }
            let n = t.chars().count();
            copy_text_to_clipboard(&t, wait_clipboard);
            eprintln!("copied {n} chars of text to clipboard");
            Ok(Outcome::TextCopied(n))
        }
        Action::Translate => {
            let t = qs_ocr::extract_text(&img)?;
            if t.is_empty() {
                return Ok(Outcome::NoText);
            }
            match qs_translate::translate(&t, &cfg.translate_source, &cfg.translate_target) {
                Ok(tr) => {
                    let n = tr.chars().count();
                    copy_text_to_clipboard(&tr, wait_clipboard);
                    eprintln!("copied {n} chars of translated text");
                    Ok(Outcome::Translated(n))
                }
                // a translation failure still hands the user the raw text —
                // clipboard with something beats a dead end
                Err(e) => {
                    eprintln!("translate failed ({e:#}), copied raw text");
                    let n = t.chars().count();
                    copy_text_to_clipboard(&t, wait_clipboard);
                    Ok(Outcome::TranslateFailed(n))
                }
            }
        }
    }
}

fn save(img: &RgbaImage, target: &SaveTarget) -> Result<PathBuf> {
    let path = match target {
        SaveTarget::File(p) => p.clone(),
        SaveTarget::Dir(dir) => {
            std::fs::create_dir_all(dir)?;
            dir.join(format!("quickshort-{}.png", unix_ts()?))
        }
    };
    img.save(&path)
        .with_context(|| format!("save {}", path.display()))?;
    eprintln!("saved {}", path.display());
    Ok(path)
}

fn unix_ts() -> Result<u64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs())
}

/// Newest-first `*.png` listing of `dir`, capped at `n` — the tray's
/// "Recent Captures" menu reads this.
pub fn recent_captures(dir: &Path, n: usize) -> Vec<PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().is_some_and(|x| x == "png")
                        && p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with("quickshort-"))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|p| std::cmp::Reverse(std::fs::metadata(p).and_then(|m| m.modified()).ok()));
    v.truncate(n);
    v
}

/// Reveal `p` in the OS file manager / image viewer.
pub fn open_path(p: &Path) {
    let r = {
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open").arg(p).spawn()
        }
        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("explorer").arg(p).spawn()
        }
        #[cfg(target_os = "linux")]
        {
            std::process::Command::new("xdg-open").arg(p).spawn()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = p;
            Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "open"))
        }
    };
    if let Err(e) = r {
        eprintln!("open {}: {e}", p.display());
    }
}

/// On Wayland the clipboard dies with the owning process — `set().wait()`
/// keeps us alive serving paste requests until another app takes over.
fn copy_image_to_clipboard(img: &RgbaImage, wait: bool) {
    let data = arboard::ImageData {
        width: img.width() as usize,
        height: img.height() as usize,
        bytes: img.as_raw().into(),
    };
    #[cfg(target_os = "linux")]
    let r = arboard::Clipboard::new().map(|mut c| {
        use arboard::SetExtLinux;
        // `set` and `set.wait()` are distinct types — branch the whole call
        if wait {
            c.set().wait().image(data)
        } else {
            c.set().image(data)
        }
    });
    #[cfg(not(target_os = "linux"))]
    let r = {
        let _ = wait;
        arboard::Clipboard::new().map(|mut c| c.set().image(data))
    };
    report_clipboard(r);
}

fn copy_text_to_clipboard(text: &str, wait: bool) {
    #[cfg(target_os = "linux")]
    let r = arboard::Clipboard::new().map(|mut c| {
        use arboard::SetExtLinux;
        if wait {
            c.set().wait().text(text.to_string())
        } else {
            c.set().text(text.to_string())
        }
    });
    #[cfg(not(target_os = "linux"))]
    let r = {
        let _ = wait;
        arboard::Clipboard::new().map(|mut c| c.set().text(text.to_string()))
    };
    report_clipboard(r);
}

fn report_clipboard(r: Result<Result<(), arboard::Error>, arboard::Error>) {
    match r {
        Ok(Ok(())) => eprintln!("copied to clipboard"),
        Ok(Err(e)) => eprintln!("clipboard set failed: {e}"),
        Err(e) => eprintln!("clipboard unavailable: {e}"),
    }
}
