//! Pipeline: capture all monitors to RAM -> overlay selection -> worker-thread
//! stitch -> clipboard + PNG file.

use anyhow::{Context, Result};

/// quickshort [--delay SECS] [--out FILE.png]
struct Args {
    delay_secs: f64,
    out: Option<std::path::PathBuf>,
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        delay_secs: 0.0,
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
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
                eprintln!("usage: quickshort [--delay SECS] [--out FILE.png]");
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

    // 1. background capture into RAM
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

    // 2. overlay + 3. coordinate/DPI mapping happens inside select_region
    let Some((sel, action)) = qs_overlay::select_region(&frames, bounds)? else {
        eprintln!("cancelled");
        return Ok(());
    };

    // 4. overlay is gone; crop/stitch on a worker thread
    let img = std::thread::spawn(move || qs_core::stitch(&frames, sel))
        .join()
        .expect("stitch worker panicked");
    let Some(img) = img else {
        anyhow::bail!("selection {sel:?} covers no monitor");
    };

    match action {
        qs_overlay::Action::Save => {
            let path = match args.out {
                Some(p) => p,
                // default: ~/Pictures/quickshort/quickshort-<ts>.png
                None => {
                    let dir = dirs::picture_dir()
                        .or_else(dirs::home_dir)
                        .unwrap_or_else(|| std::path::PathBuf::from("."))
                        .join("quickshort");
                    std::fs::create_dir_all(&dir)?;
                    dir.join(format!(
                        "quickshort-{}.png",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)?
                            .as_secs()
                    ))
                }
            };
            img.save(&path)?;
            eprintln!("saved {}", path.display());
        }
        // Last: this blocks serving paste requests until the clipboard is
        // replaced — Wayland clipboard dies with the owning process otherwise.
        qs_overlay::Action::Copy => copy_to_clipboard(&img),
        qs_overlay::Action::Ocr => match qs_ocr::extract_text(&img) {
            Ok(t) if t.is_empty() => eprintln!("ocr: no text found"),
            Ok(t) => {
                eprintln!("copied {} chars of text", t.chars().count());
                copy_text_to_clipboard(&t);
            }
            Err(e) => eprintln!("ocr failed: {e:#}"),
        },
    }
    Ok(())
}

fn copy_to_clipboard(img: &image::RgbaImage) {
    let data = arboard::ImageData {
        width: img.width() as usize,
        height: img.height() as usize,
        bytes: img.as_raw().into(),
    };
    let r = arboard::Clipboard::new().map(|mut c| {
        let set = c.set();
        #[cfg(target_os = "linux")]
        let set = {
            use arboard::SetExtLinux;
            set.wait()
        };
        set.image(data)
    });
    match r {
        Ok(Ok(())) => eprintln!("copied to clipboard"),
        Ok(Err(e)) => eprintln!("clipboard set failed: {e}"),
        Err(e) => eprintln!("clipboard unavailable: {e}"),
    }
}

fn copy_text_to_clipboard(text: &str) {
    match arboard::Clipboard::new().map(|mut c| c.set_text(text)) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => eprintln!("clipboard set failed: {e}"),
        Err(e) => eprintln!("clipboard unavailable: {e}"),
    }
}
