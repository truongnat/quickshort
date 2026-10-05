//! quickshort — region screenshot tool.
//!
//! `quickshort`            resident tray app (menu + global hotkey)
//! `quickshort --capture`  one-shot: capture -> overlay -> action -> exit

// GUI subsystem on Windows — no console window when launched from Explorer,
// the Start Menu, or the installer "Launch" checkbox.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod actions;
mod config;
mod host;
mod settings;
mod share;
mod toast;
mod tray;
mod update;

use actions::SaveTarget;
use anyhow::{Context, Result};

/// quickshort [--capture] [--delay SECS] [--out FILE.png] [--settings]
struct Args {
    capture_now: bool,
    settings_only: bool,
    toast_demo: bool,
    delay_secs: f64,
    out: Option<std::path::PathBuf>,
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        capture_now: false,
        settings_only: false,
        toast_demo: false,
        delay_secs: 0.0,
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--capture" => args.capture_now = true,
            "--settings" => args.settings_only = true,
            "--toast" => args.toast_demo = true,
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

/// One-shot pipeline: capture all monitors to RAM -> overlay selection ->
/// worker-thread stitch -> action.
fn run_one_shot(args: Args) -> Result<()> {
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
    let Some((sel, action, annots)) =
        qs_overlay::select_region(&frames, bounds, &cfg.overlay_prefs())?
    else {
        eprintln!("cancelled");
        return Ok(());
    };

    // selection done — release before dispatch: the Copy action may keep
    // this process alive serving the clipboard, and that must not block
    // the next hotkey press
    #[cfg(target_os = "linux")]
    drop(_lock);

    let target = match args.out {
        Some(p) => SaveTarget::File(p),
        None => SaveTarget::Dir(cfg.save_dir()),
    };
    actions::dispatch(
        frames,
        sel,
        action,
        &annots,
        &target,
        true,
        None,
        cfg.copy_after_save,
    )?;
    Ok(())
}
