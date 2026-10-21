//! quickshort — region screenshot tool.
//!
//! `quickshort`            resident tray app (menu + global hotkey)
//! `quickshort --capture`  one-shot: capture -> overlay -> action -> exit

// GUI subsystem on Windows — no console window when launched from Explorer,
// the Start Menu, or the installer "Launch" checkbox.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod actions;
mod chrome;
mod config;
mod host;
#[cfg(target_os = "linux")]
mod hotkey_linux;
mod oneshot;
mod pin;
mod quick_access;
mod record;
mod recstop;
mod scroll;
mod settings;
mod share;
mod toast;
mod tray;
mod update;
mod widgets;

use actions::SaveTarget;
use anyhow::{Context, Result};

/// quickshort [--capture|--capture-full] [--delay SECS] [--out FILE.png]
struct Args {
    capture_now: bool,
    /// stitch the whole virtual screen and save — no overlay
    capture_full: bool,
    settings_only: bool,
    toast_demo: bool,
    card_demo: bool,
    chrome_demo: bool,
    delay_secs: f64,
    out: Option<std::path::PathBuf>,
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        capture_now: false,
        capture_full: false,
        settings_only: false,
        toast_demo: false,
        card_demo: false,
        chrome_demo: false,
        delay_secs: 0.0,
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--capture" => args.capture_now = true,
            "--capture-full" => args.capture_full = true,
            "--settings" => args.settings_only = true,
            "--toast" => args.toast_demo = true,
            "--card" => args.card_demo = true,
            "--chrome" => args.chrome_demo = true,
            "--delay" => {
                let v = it.next().context("--delay needs SECS")?;
                args.delay_secs = v.parse().context("--delay: not a number")?;
            }
            "--out" => {
                args.out = Some(std::path::PathBuf::from(
                    it.next().context("--out needs FILE.png")?,
                ));
            }
            "-h" | "--help" => {
                eprintln!(
                    "usage: quickshort [--capture|--capture-full [--delay SECS] [--out FILE.png]]\n\
                     no args: resident tray app (menu + global hotkey)"
                );
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown arg {other:?} (see --help)"),
        }
    }
    Ok(args)
}

fn main() -> Result<()> {
    qs_capture::platform_init();
    let args = parse_args()?;
    if args.settings_only {
        return settings::standalone();
    }
    if args.toast_demo {
        return toast::standalone();
    }
    if args.card_demo {
        let img = image::RgbaImage::from_fn(800, 500, |x, y| {
            image::Rgba([(x % 255) as u8, (y % 255) as u8, 160, 255])
        });
        return quick_access::standalone(
            img,
            "Saved quickshort-demo.png",
            None,
            std::path::PathBuf::from("/tmp"),
            0x1677DF,
            false,
        );
    }
    if args.chrome_demo {
        let cfg = config::Config::load();
        return chrome::standalone(&cfg.popup_pos, cfg.accent_rgb());
    }
    if !(args.capture_now || args.capture_full) {
        // one resident app per user — a second launch (LaunchAgent plus a
        // manual `open`, etc.) would double-register the global hotkey and
        // stack two overlays on every press
        let Some(_guard) = app_lock()? else {
            return Ok(());
        };
        return host::run(config::Config::load());
    }
    run_one_shot(args)
}

/// Whole-file lock in the runtime dir: released on process exit, even on
/// SIGKILL. `try_lock` is flock on Unix, LockFileEx on Windows.
fn lock_file(name: &str, busy_msg: &str) -> Result<Option<std::fs::File>> {
    let dir = dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("quickshort");
    std::fs::create_dir_all(&dir)?;
    let f = std::fs::File::create(dir.join(name))?;
    match f.try_lock() {
        Ok(()) => Ok(Some(f)),
        Err(std::fs::TryLockError::WouldBlock) => {
            eprintln!("{busy_msg}");
            Ok(None)
        }
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

/// Singleton for the resident tray app, held for the process lifetime.
fn app_lock() -> Result<Option<std::fs::File>> {
    lock_file("app.lock", "quickshort is already running")
}

/// Held for the duration of a capture — a second --capture while an
/// overlay is up exits quietly instead of stacking another layer.
fn capture_lock() -> Result<Option<std::fs::File>> {
    lock_file("capture.lock", "a capture is already in progress")
}

/// One-shot pipeline: capture all monitors to RAM -> overlay selection ->
/// worker-thread stitch -> action.
fn run_one_shot(args: Args) -> Result<()> {
    // one capture at a time — a second --capture while an overlay is up
    // exits quietly instead of stacking another fullscreen layer
    let lock = match capture_lock() {
        Ok(l) => l,
        Err(e) => return Err(e),
    };
    if args.delay_secs > 0.0 {
        std::thread::sleep(std::time::Duration::from_secs_f64(args.delay_secs));
    }
    let backend = qs_capture::backend()?;
    let frames = backend.capture_all()?;
    if frames.is_empty() {
        anyhow::bail!("no monitors captured");
    }
    let bounds = qs_core::virtual_bounds(&frames);
    eprintln!(
        "captured {} monitor(s), virtual bounds {}x{} at ({},{})",
        frames.len(),
        bounds.w,
        bounds.h,
        bounds.x,
        bounds.y
    );

    let cfg = config::Config::load();

    let target = match args.out {
        Some(p) => SaveTarget::File(p),
        None => SaveTarget::Dir(cfg.save_dir()),
    };

    if args.capture_full {
        // no overlay: stitch the whole virtual screen and save — what
        // the compositor bindings spawn for the fullscreen hotkey. No
        // event loop needed; wait_clipboard covers copy_after_save.
        let d = actions::dispatch(
            frames,
            bounds,
            qs_overlay::Action::Save,
            &[],
            &target,
            true,
            None,
            cfg.copy_after_save,
            &cfg.ocr_lang,
        )?;
        eprintln!("fullscreen saved, {:?} outcome", d.outcome);
        return Ok(());
    }

    // winit permits one event loop per process, so overlay + action
    // workers + result card all share the oneshot loop; the capture
    // lock rides along and drops when the overlay closes
    oneshot::run(frames, bounds, target, &cfg, lock)
}
