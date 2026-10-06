//! quickshort — region screenshot tool.
//!
//! `quickshort`            resident tray app (menu + global hotkey)
//! `quickshort --capture`  one-shot: capture -> overlay -> action -> exit

// GUI subsystem on Windows — no console window when launched from Explorer,
// the Start Menu, or the installer "Launch" checkbox.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod actions;
mod config;
mod gallery;
mod host;
#[cfg(unix)]
mod ipc;
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

/// quickshort [--capture] [--delay SECS] [--out FILE.png] [--settings]
struct Args {
    capture_now: bool,
    /// Ask the resident host to open the capture gallery.
    gallery: bool,
    settings_only: bool,
    toast_demo: bool,
    card_demo: bool,
    delay_secs: f64,
    out: Option<std::path::PathBuf>,
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        capture_now: false,
        gallery: false,
        settings_only: false,
        toast_demo: false,
        card_demo: false,
        delay_secs: 0.0,
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--capture" => args.capture_now = true,
            "--gallery" => args.gallery = true,
            "--settings" => args.settings_only = true,
            "--toast" => args.toast_demo = true,
            "--card" => args.card_demo = true,
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
                    "usage: quickshort [--capture [--delay SECS] [--out FILE.png]]\n\
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
    if args.gallery {
        // gallery lives in the resident host — nothing to fall back to
        if ipc::delegate("gallery") {
            return Ok(());
        }
        anyhow::bail!("no resident quickshort running");
    }
    if !args.capture_now {
        return host::run(config::Config::load());
    }
    run_one_shot(args)
}

/// flock-held for the whole capture: dropped (released) when the process
/// exits, even on SIGKILL.
#[cfg(target_os = "linux")]
fn capture_lock() -> Result<Option<std::fs::File>> {
    use std::os::unix::io::AsRawFd;
    let dir = dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("quickshort");
    std::fs::create_dir_all(&dir)?;
    let f = std::fs::File::create(dir.join("capture.lock"))?;
    // flock fails EWOULDBLOCK iff another quickshort holds it
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        eprintln!("a capture is already in progress");
        return Ok(None);
    }
    Ok(Some(f))
}

/// One-shot mode has no toast — action outcomes would die silently in
/// the journal. `notify-send` is the only user-visible channel we have.
#[cfg(target_os = "linux")]
fn notify(msg: &str) {
    let _ = std::process::Command::new("notify-send")
        .args(["-a", "quickshort", "quickshort", msg])
        .status();
}

#[cfg(not(target_os = "linux"))]
fn notify(msg: &str) {
    // no libnotify elsewhere — macOS/Windows still get the journal line
    eprintln!("{msg}");
}

/// One-shot pipeline: capture all monitors to RAM -> overlay selection ->
/// worker-thread stitch -> action.
fn run_one_shot(args: Args) -> Result<()> {
    // a running resident owns the tray and the full UX — hand plain
    // --capture calls to it so keybindings get the resident flow;
    // explicit --out/--delay invocations keep the standalone path
    #[cfg(unix)]
    if args.out.is_none() && args.delay_secs == 0.0 && ipc::delegate("capture") {
        return Ok(());
    }
    // one capture at a time — a second --capture while an overlay is up
    // exits quietly instead of stacking another fullscreen layer
    #[cfg(target_os = "linux")]
    let _lock = match capture_lock() {
        Ok(Some(l)) => l,
        Ok(None) => return Ok(()),
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

    // winit permits one event loop per process, so overlay + action
    // workers + result card all share the oneshot loop; the capture
    // lock rides along and drops when the overlay closes
    let target = match args.out {
        Some(p) => SaveTarget::File(p),
        None => SaveTarget::Dir(cfg.save_dir()),
    };
    #[cfg(target_os = "linux")]
    let lock = Some(_lock);
    #[cfg(not(target_os = "linux"))]
    let lock = None;
    oneshot::run(frames, bounds, target, &cfg, lock)
}
