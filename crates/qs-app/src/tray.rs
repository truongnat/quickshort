//! Tray icon + menu plumbing: generated RGBA icon, menu construction,
//! launch-at-login toggling per OS.

use anyhow::{Context, Result};
use muda::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use std::path::PathBuf;
use tray_icon::Icon;

use crate::config::Config;

pub const ID_CAPTURE: &str = "capture";
/// Saves the whole virtual screen — no region overlay.
pub const ID_CAPTURE_FULL: &str = "capture_full";
pub const ID_OPEN_DIR: &str = "open_dir";
pub const ID_SETTINGS: &str = "settings";
pub const ID_LOGIN: &str = "launch_at_login";
pub const ID_QUIT: &str = "quit";
/// Recent-capture items carry `recent:<abs path>` as their id.
pub const ID_RECENT_PREFIX: &str = "recent:";
pub const ID_UPDATE: &str = "update";
pub const ID_DONATE: &str = "donate";

/// Rebuild the tray menu. `recents` = newest-first saved PNGs;
/// `pending_update` = version waiting to be installed (changes the
/// "Check for Updates…" item into a one-click updater).
pub fn build_menu(cfg: &Config, recents: &[PathBuf], pending_update: Option<&str>) -> Menu {
    let menu = Menu::new();
    let capture = MenuItem::with_id(
        ID_CAPTURE,
        format!("Capture Region    {}", cfg.hotkey),
        true,
        None,
    );
    let capture_full = MenuItem::with_id(
        ID_CAPTURE_FULL,
        format!("Capture Full Screen    {}", cfg.hotkey_full),
        true,
        None,
    );
    let recent = Submenu::new("Recent Captures", !recents.is_empty());
    for p in recents {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.display().to_string());
        let item = MenuItem::with_id(
            format!("{ID_RECENT_PREFIX}{}", p.display()),
            name,
            true,
            None,
        );
        let _ = recent.append(&item);
    }
    let open = MenuItem::with_id(ID_OPEN_DIR, "Open Screenshots Folder", true, None);
    let login =
        CheckMenuItem::with_id(ID_LOGIN, "Launch at Login", true, cfg.launch_at_login, None);
    let settings = MenuItem::with_id(ID_SETTINGS, "Settings…", true, None);
    let update = MenuItem::with_id(
        ID_UPDATE,
        match pending_update {
            Some(v) => format!("Update to v{v} — Restart to Install"),
            None => "Check for Updates…".to_string(),
        },
        true,
        None,
    );
    let donate = MenuItem::with_id(ID_DONATE, "Support Quickshort", true, None);
    let quit = MenuItem::with_id(ID_QUIT, "Quit Quickshort", true, None);
    let _ = menu.append_items(&[
        &capture,
        &capture_full,
        &recent,
        &PredefinedMenuItem::separator(),
        &open,
        &settings,
        &login,
        &PredefinedMenuItem::separator(),
        &update,
        &donate,
        &quit,
    ]);
    menu
}

/// Crop-corners tray glyph, drawn directly into RGBA — same SDF coverage math
/// as the overlay icons, no asset files. `size` in px (44 = 22pt @2x).
pub fn tray_icon(size: u32) -> Icon {
    let s = size as f64;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let put = |rgba: &mut Vec<u8>, x: i32, y: i32, a: f64| {
        if x < 0 || y < 0 || x >= size as i32 || y >= size as i32 {
            return;
        }
        let i = ((y * size as i32 + x) * 4) as usize;
        rgba[i] = (255.0 * a) as u8;
        rgba[i + 1] = rgba[i];
        rgba[i + 2] = rgba[i];
        rgba[i + 3] = rgba[i];
    };
    // corner brackets ⌜ ⌝ ⌞ ⌟ — keep them short; adjacent ends must leave a
    // visible gap or the glyph reads as a plain square at menu-bar size
    let st = s * 0.115;
    let ins = s * 0.20;
    let len = s * 0.15;
    let (x0, y0, x1, y1) = (ins, ins, s - ins, s - ins);
    let segs = [
        (x0, y0 + len, x0, y0),
        (x0, y0, x0 + len, y0), // top-left
        (x1 - len, y0, x1, y0),
        (x1, y0, x1, y0 + len), // top-right
        (x0, y1 - len, x0, y1),
        (x0, y1, x0 + len, y1), // bottom-left
        (x1 - len, y1, x1, y1),
        (x1, y1 - len, x1, y1), // bottom-right
    ];
    for y in 0..size as i32 {
        for x in 0..size as i32 {
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            let mut d = f64::INFINITY;
            for &(ax, ay, bx, by) in &segs {
                // point-segment distance
                let (vx, vy) = (bx - ax, by - ay);
                let t = (((px - ax) * vx + (py - ay) * vy) / (vx * vx + vy * vy)).clamp(0.0, 1.0);
                let (dx, dy) = (px - (ax + t * vx), py - (ay + t * vy));
                d = d.min((dx * dx + dy * dy).sqrt());
            }
            let a = (st / 2.0 + 0.75 - d).clamp(0.0, 1.0);
            if a > 0.0 {
                put(&mut rgba, x, y, a);
            }
        }
    }
    Icon::from_rgba(rgba, size, size).expect("tray icon rgba")
}

// ---------- Linux: the tray lives on a dedicated gtk thread ----------
//
// tray-icon/muda call gtk, which is single-threaded but not bound to
// "the" main thread — so a dedicated thread runs gtk::init() +
// gtk::main() and owns the TrayIcon, while the winit thread talks to it
// through a channel. `MainContext::invoke` wakes gtk::main() exactly
// when a message arrives, so no polling is needed.
#[cfg(target_os = "linux")]
pub mod linux_tray {
    use super::*;
    use gtk::glib;
    use std::cell::RefCell;
    use std::sync::{mpsc, OnceLock};

    pub enum TrayMsg {
        Menu {
            cfg: Config,
            recents: Vec<PathBuf>,
            update: Option<String>,
        },
        Quit,
    }

    thread_local! {
        static TRAY: RefCell<Option<tray_icon::TrayIcon>> = const { RefCell::new(None) };
        static RX: RefCell<Option<mpsc::Receiver<TrayMsg>>> = const { RefCell::new(None) };
    }
    static CTX: OnceLock<glib::MainContext> = OnceLock::new();

    /// Handle held by the host thread — the TrayIcon itself is pinned
    /// inside the tray thread's thread-local.
    pub struct TrayHandle {
        tx: mpsc::Sender<TrayMsg>,
    }

    pub fn spawn(cfg: &Config, recents: &[PathBuf]) -> Result<TrayHandle> {
        let (tx, rx) = mpsc::channel::<TrayMsg>();
        let (init_tx, init_rx) = mpsc::channel::<Result<()>>();
        let (cfg, recents) = (cfg.clone(), recents.to_vec());
        std::thread::spawn(move || {
            if let Err(e) = gtk::init() {
                let _ = init_tx.send(Err(anyhow::anyhow!("gtk::init: {e}")));
                return;
            }
            let built = tray_icon::TrayIcon::builder()
                .with_tooltip("Quickshort — region screenshot")
                .with_menu(Box::new(build_menu(&cfg, &recents, None)))
                .with_icon(tray_icon(44))
                .build();
            match built {
                Ok(t) => TRAY.with(|s| *s.borrow_mut() = Some(t)),
                Err(e) => {
                    let _ = init_tx.send(Err(anyhow::anyhow!("tray icon: {e}")));
                    return;
                }
            }
            RX.with(|s| *s.borrow_mut() = Some(rx));
            // gtk::main iterates the default context — publish it so other
            // threads can wake the loop via invoke()
            let _ = CTX.set(glib::MainContext::default());
            let _ = init_tx.send(Ok(()));
            gtk::main();
        });
        init_rx.recv().context("tray thread died")??;
        Ok(TrayHandle { tx })
    }

    /// Runs on the tray thread via MainContext::invoke.
    fn drain() {
        RX.with(|rx| {
            let rx = rx.borrow();
            let Some(rx) = rx.as_ref() else { return };
            while let Ok(m) = rx.try_recv() {
                match m {
                    TrayMsg::Menu {
                        cfg,
                        recents,
                        update,
                    } => TRAY.with(|t| {
                        if let Some(t) = t.borrow().as_ref() {
                            t.set_menu(Some(Box::new(build_menu(
                                &cfg,
                                &recents,
                                update.as_deref(),
                            ))));
                        }
                    }),
                    TrayMsg::Quit => {
                        TRAY.with(|t| *t.borrow_mut() = None);
                        gtk::main_quit();
                    }
                }
            }
        });
    }

    fn wake() {
        if let Some(ctx) = CTX.get() {
            ctx.invoke(drain);
        }
    }

    impl TrayHandle {
        pub fn set_menu(&self, cfg: Config, recents: Vec<PathBuf>, update: Option<String>) {
            let _ = self.tx.send(TrayMsg::Menu {
                cfg,
                recents,
                update,
            });
            wake();
        }
    }

    impl Drop for TrayHandle {
        fn drop(&mut self) {
            let _ = self.tx.send(TrayMsg::Quit);
            wake();
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux_tray::TrayHandle;

/// Toggle "launch at login" for the current executable.
pub fn set_launch_at_login(enable: bool) -> Result<()> {
    let exe = std::env::current_exe().context("current_exe")?;
    #[cfg(target_os = "macos")]
    {
        // LaunchAgent — works for an unbundled binary; a future .app can
        // switch to SMAppService without touching callers.
        let dir = dirs::home_dir()
            .context("no home dir")?
            .join("Library/LaunchAgents");
        let plist = dir.join("ai.quickshort.plist");
        if enable {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(
                &plist,
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                     <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\"\
                     \x20\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
                     <plist version=\"1.0\"><dict>\n\
                     <key>Label</key><string>ai.quickshort</string>\n\
                     <key>ProgramArguments</key><array><string>{}</string></array>\n\
                     <key>RunAtLoad</key><true/>\n\
                     </dict></plist>\n",
                    exe.display()
                ),
            )?;
        } else {
            let _ = std::fs::remove_file(&plist);
        }
    }
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let run = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(
                "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                winreg::enums::KEY_SET_VALUE,
            )
            .context("open Run key")?;
        if enable {
            run.set_value("quickshort", &exe.display().to_string())?;
        } else {
            let _ = run.delete_value("quickshort");
        }
    }
    #[cfg(target_os = "linux")]
    {
        let dir = dirs::config_dir()
            .context("no config dir")?
            .join("autostart");
        let desk = dir.join("quickshort.desktop");
        if enable {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(
                &desk,
                format!(
                    "[Desktop Entry]\nType=Application\nName=Quickshort\n\
                     Exec={}\nX-GNOME-Autostart-enabled=true\n",
                    exe.display()
                ),
            )?;
        } else {
            let _ = std::fs::remove_file(&desk);
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = (enable, exe);
        anyhow::bail!("launch at login unsupported on this platform");
    }
    Ok(())
}
