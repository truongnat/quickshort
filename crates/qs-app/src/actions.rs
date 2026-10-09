//! Post-selection action dispatch — shared by the one-shot CLI path and the
//! resident tray app.

use anyhow::{Context, Result};
use image::RgbaImage;
use qs_overlay::{Action, Annotation};
use std::path::{Path, PathBuf};

use crate::config::Config;

/// Where a `Save` lands: an explicit output file (`--out`) or the default
/// screenshots dir with an auto-generated timestamped name.
#[derive(Clone)]
pub enum SaveTarget {
    File(PathBuf),
    Dir(PathBuf),
}

/// What an action produced — the resident app mirrors this into its toast.
#[derive(Debug)]
pub enum Outcome {
    Saved(PathBuf),
    Copied,
    /// public URL of the shared file, already on the clipboard
    Shared(crate::share::SharedLink),
    /// chars of extracted text put on the clipboard
    TextCopied(usize),
    /// OCR ran but found no text
    NoText,
}

/// `dispatch` output: the outcome plus the image it was produced from —
/// the resident app's post-capture UI (chrome stack, or the quick-access
/// card fallback) needs the pixels for its thumbnail, the selection for
/// the fly-in animation, and follow-up actions (copy/save/share/pin).
pub struct Dispatch {
    pub outcome: Outcome,
    pub img: RgbaImage,
    pub sel: qs_core::Rect,
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
    wait_clipboard: bool,
    share: Option<std::sync::Arc<crate::share::ShareManager>>,
    copy_after_save: bool,
    ocr_lang: &str,
) -> Result<Dispatch> {
    eprintln!("dispatch {action:?}");
    let img = std::thread::spawn(move || qs_core::stitch(&frames, sel))
        .join()
        .expect("stitch worker panicked");
    let Some((mut img, scale)) = img else {
        anyhow::bail!("selection {sel:?} covers no monitor");
    };
    qs_overlay::annotate_image(&mut img, sel, annots, scale);

    let outcome = match action {
        Action::Save => {
            let p = save(&img, target)?;
            if copy_after_save {
                copy_image_to_clipboard(&img, wait_clipboard);
            }
            Outcome::Saved(p)
        }
        Action::Share => {
            let sm = share.context("link sharing needs the resident app")?;
            // shared files live in their own subdir so they don't spam
            // Recent Captures; the URL's token path is what keeps them
            // private
            let base = match target {
                SaveTarget::Dir(d) => d.clone(),
                SaveTarget::File(p) => p.parent().unwrap_or_else(|| Path::new(".")).to_path_buf(),
            };
            let dir = base.join("shared");
            std::fs::create_dir_all(&dir)?;
            let name = format!("quickshort-{}-{}.png", unix_ts()?, rand_hex()?);
            save(&img, &SaveTarget::File(dir.join(name)))?;
            let link = sm.share(&img)?;
            copy_text_to_clipboard(&link.url, wait_clipboard);
            Outcome::Shared(link)
        }
        Action::Copy => {
            copy_image_to_clipboard(&img, wait_clipboard);
            Outcome::Copied
        }
        Action::Scroll | Action::Record => {
            // the caller handles it — these need a live capture session /
            // stream and a stop channel, neither of which dispatch owns
            anyhow::bail!("{action:?} is driven by host/main, not dispatch")
        }
        Action::Ocr => {
            let t = qs_ocr::extract_text(&img, ocr_lang)?;
            if t.is_empty() {
                Outcome::NoText
            } else {
                let n = t.chars().count();
                copy_text_to_clipboard(&t, wait_clipboard);
                eprintln!("copied {n} chars of text to clipboard");
                Outcome::TextCopied(n)
            }
        }
    };
    Ok(Dispatch { outcome, img, sel })
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

/// 48-bit hex — the unguessable suffix of a shared file's public name.
fn rand_hex() -> Result<String> {
    let mut b = [0u8; 6];
    getrandom::fill(&mut b).map_err(|e| anyhow::anyhow!("os entropy: {e}"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
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

/// (count, total bytes) of the top-level `quickshort-*.png` captures in
/// `dir` — the Storage page reads this; `shared/` is a subdir so it's
/// naturally excluded.
pub fn storage_stats(dir: &Path) -> (usize, u64) {
    let mut n = 0usize;
    let mut bytes = 0u64;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "png")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("quickshort-"))
            {
                n += 1;
                bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    (n, bytes)
}

/// (total, available) bytes of the volume holding `dir` — `df -Pk` is
/// POSIX and saves a libc/Foundation dep just for statfs.
pub fn disk_stats(dir: &Path) -> Option<(u64, u64)> {
    let out = std::process::Command::new("df")
        .arg("-Pk")
        .arg(dir)
        .output()
        .ok()?;
    let line = String::from_utf8_lossy(&out.stdout);
    let f: Vec<&str> = line.lines().nth(1)?.split_whitespace().collect();
    Some((
        f.get(1)?.parse::<u64>().ok()? * 1024,
        f.get(3)?.parse::<u64>().ok()? * 1024,
    ))
}

/// Delete captures older than `keep_days`; 0 disables the sweep. Returns
/// how many files were removed. Runs on startup and when the retention
/// setting changes.
pub fn cleanup_captures(dir: &Path, keep_days: u32) -> usize {
    if keep_days == 0 {
        return 0;
    }
    let cutoff = std::time::Duration::from_secs(keep_days as u64 * 86_400);
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !(p.extension().is_some_and(|x| x == "png")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("quickshort-")))
            {
                continue;
            }
            let too_old = std::fs::metadata(&p)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > cutoff);
            if too_old && std::fs::remove_file(&p).is_ok() {
                n += 1;
            }
        }
    }
    n
}

/// Bug-report relay (Cloudflare Worker, see `relay/report/`) — files the
/// issue under the maintainer's account so users don't need a GitHub
/// login. QS_REPORT_URL overrides it for testing a fresh deploy.
const REPORT_ENDPOINT: &str = "https://quickshort-report.tx001-dev.workers.dev";

/// Collect diagnostics and file a GitHub issue via the relay worker —
/// no GitHub login required on the user's side. Runs off-thread so the
/// settings UI doesn't stall on the network call; on success the new
/// issue opens in the browser, on failure we fall back to the prefilled
/// `issues/new` page.
pub fn report_issue(cfg: &Config) {
    let diagnostics = diagnostics(cfg);
    let endpoint = std::env::var("QS_REPORT_URL").unwrap_or_else(|_| REPORT_ENDPOINT.into());
    std::thread::spawn(move || {
        let payload = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "os": format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
            "diagnostics": diagnostics,
        });
        let issue_url = ureq::post(&endpoint)
            .send_json(&payload)
            .ok()
            .and_then(|mut r| r.body_mut().read_json::<serde_json::Value>().ok())
            .and_then(|v| v["url"].as_str().map(str::to_owned));
        match issue_url {
            Some(url) => open_url(&url),
            None => {
                let diag = payload["diagnostics"].as_str().unwrap_or_default();
                let url = format!(
                    "https://github.com/truongnat/quickshort/issues/new?labels=bug&body={}",
                    url_encode(diag)
                );
                open_url(&url);
            }
        }
    });
}

/// Version + environment + config snapshot + tail of the app log —
/// enough context that a bug report doesn't start at zero.
fn diagnostics(cfg: &Config) -> String {
    let macos = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "?".into());
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let log = Config::path()
        .parent()
        .map(|d| d.join("quickshort.log"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| {
            s.lines()
                .rev()
                .take(30)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "(no log)".into());
    format!(
        "## What happened\n\n\
         <!-- describe the bug, what you expected, and steps to reproduce -->\n\n\
         ## Diagnostics\n\n\
         - Version: v{ver}\n\
         - macOS: {macos} ({arch})\n\
         - Binary: `{exe}`{bundled}\n\
         - Hotkey: `{hotkey}`\n\
         - Save dir: `{save_dir}`\n\
         - Config: accent={accent} keep_days={keep} recent_max={recent} \
         launch_at_login={login} auto_update={auto} copy_after_save={copy}\n\
         - Keys: save={ks} copy={kc} ocr={ko} share={kh} undo={ku}\n\n\
         <details><summary>Recent log</summary>\n\n```\n{log}\n```\n</details>\n",
        ver = env!("CARGO_PKG_VERSION"),
        arch = std::env::consts::ARCH,
        bundled = if exe.contains(".app/") {
            " (app bundle)"
        } else {
            " (raw binary)"
        },
        hotkey = cfg.hotkey,
        save_dir = cfg.save_dir().display(),
        accent = cfg.accent,
        keep = cfg.keep_days,
        recent = cfg.recent_max,
        login = cfg.launch_at_login,
        auto = cfg.auto_update,
        copy = cfg.copy_after_save,
        ks = cfg.key_save,
        kc = cfg.key_copy,
        ko = cfg.key_ocr,
        kh = cfg.key_share,
        ku = cfg.key_undo,
    )
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
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

/// Open `url` in the default browser. Kept separate from `open_path`:
/// explorer.exe treats URLs containing ? or & as file paths and opens
/// File Explorer instead of the browser.
pub fn open_url(url: &str) {
    let r = {
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open").arg(url).spawn()
        }
        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("rundll32")
                .args(["url.dll,FileProtocolHandler", url])
                .spawn()
        }
        #[cfg(target_os = "linux")]
        {
            std::process::Command::new("xdg-open").arg(url).spawn()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = url;
            Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "open"))
        }
    };
    if let Err(e) = r {
        eprintln!("open {url}: {e}");
    }
}

/// Follow-up actions for the quick-access card — the same primitives
/// `dispatch` uses. `wait` is the clipboard-survival flag: the resident
/// app passes false (it stays alive), one-shot mode passes true so the
/// copy outlives the process.
pub fn copy_image(img: &RgbaImage, wait: bool) {
    copy_image_to_clipboard(img, wait);
}

pub fn copy_text(text: &str, wait: bool) {
    copy_text_to_clipboard(text, wait);
}

pub fn save_to_dir(img: &RgbaImage, dir: &Path) -> Result<PathBuf> {
    save(img, &SaveTarget::Dir(dir.to_path_buf()))
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

/// One-shot mode exits right after dispatch; on Wayland that kills the
/// clipboard we just set. `wl-copy` forks a helper that keeps serving it
/// — use it when present instead of parking this process on `wait()`.
#[cfg(target_os = "linux")]
fn wl_copy(mime: &str, data: &[u8]) -> bool {
    use std::io::Write;
    let mut c = match std::process::Command::new("wl-copy")
        .args(["-t", mime])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let ok = c
        .stdin
        .take()
        .map(|mut s| s.write_all(data).is_ok())
        .unwrap_or(false);
    ok && c.wait().map(|s| s.success()).unwrap_or(false)
}

/// On Wayland the clipboard dies with the owning process — `set().wait()`
/// keeps us alive serving paste requests until another app takes over.
fn copy_image_to_clipboard(img: &RgbaImage, wait: bool) {
    #[cfg(target_os = "linux")]
    if wait {
        let mut png = std::io::Cursor::new(Vec::new());
        if img.write_to(&mut png, image::ImageFormat::Png).is_ok()
            && wl_copy("image/png", &png.into_inner())
        {
            eprintln!("copied to clipboard via wl-copy");
            return;
        }
    }
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
    if wait && wl_copy("text/plain;charset=utf-8", text.as_bytes()) {
        eprintln!("copied to clipboard via wl-copy");
        return;
    }
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
