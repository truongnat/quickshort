//! Linux global-hotkey registration — per-compositor, because
//! `global-hotkey`'s X11 grabs never reach Wayland (and report success
//! anyway, so the failure isn't detectable). Each desktop gets the
//! mechanism it actually honors:
//!
//!   GNOME(-like)  gsettings custom keybindings → spawns `quickshort --capture`
//!   KDE Plasma    org.freedesktop.portal.GlobalShortcuts → in-process signal
//!   sway          `swaymsg bindsym`          → spawns `quickshort --capture`
//!   Hyprland      `hyprctl keyword bind`     → spawns `quickshort --capture`
//!
//! Anything else returns Ok(None) so host.rs keeps the manual-bind hint.

use anyhow::{bail, Context, Result};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use std::process::Command;
use winit::event_loop::EventLoopProxy;

use crate::host::HostEvent;

/// A Wayland session is up — the X11-only `global-hotkey` manager is
/// dead weight there (and silently no-ops).
pub fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

#[derive(Clone, Copy)]
enum De {
    /// GNOME and mutter-adjacent shells sharing the custom-keybinding schema.
    Gnome,
    Kde,
    Sway,
    Hyprland,
}

/// XDG_CURRENT_DESKTOP is ':'-separated (e.g. "ubuntu:GNOME",
/// "Budgie:GNOME") — first recognizable token wins.
fn desktop() -> Option<De> {
    let raw = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    for tok in raw.split(':') {
        match tok.to_ascii_lowercase().as_str() {
            "gnome" | "gnome-flashback" | "unity" | "x-cinnamon" | "cinnamon" | "pantheon"
            | "budgie" => return Some(De::Gnome),
            "kde" | "plasma" => return Some(De::Kde),
            "sway" => return Some(De::Sway),
            "hyprland" => return Some(De::Hyprland),
            _ => {}
        }
    }
    None
}

/// Owns whatever keeps the registration alive and rebindable. Only KDE's
/// portal session is a real resource (a dbus connection + listener
/// thread); the spawned-command desktops just need the chords remembered
/// for suspend/rebind.
pub struct Guard {
    de: De,
    /// The `cfg.hotkey` (region) and `cfg.hotkey_full` strings currently
    /// bound — the latter may be empty (no fullscreen binding).
    chord: String,
    chord_full: String,
    kde: Option<kde::KdeHotkey>,
}

/// Register both chords (global-hotkey syntax, e.g. "Ctrl+Shift+S").
/// `full` = the no-overlay fullscreen chord; empty disables it.
/// Ok(None) = desktop has no programmatic mechanism — caller falls back
/// to the manual-bind hint (+ the X11 manager, as before).
pub fn register(
    proxy: EventLoopProxy<HostEvent>,
    chord: &str,
    full: &str,
) -> Result<Option<Guard>> {
    let Some(de) = desktop() else {
        return Ok(None);
    };
    let hk: HotKey = chord
        .parse()
        .with_context(|| format!("bad hotkey {chord:?}"))?;
    let hk_full: Option<HotKey> = if full.trim().is_empty() {
        None
    } else {
        Some(
            full.parse()
                .with_context(|| format!("bad fullscreen hotkey {full:?}"))?,
        )
    };
    let kde = match de {
        De::Gnome => {
            gnome::bind(&hk)?;
            if let Some(h) = hk_full {
                if let Err(e) = gnome::bind_full(&h) {
                    eprintln!("fullscreen gsettings bind: {e:#}");
                }
            }
            None
        }
        De::Sway => {
            sway::bind(&hk)?;
            if let Some(h) = hk_full {
                if let Err(e) = sway::bind_full(&h) {
                    eprintln!("fullscreen swaymsg bind: {e:#}");
                }
            }
            None
        }
        De::Hyprland => {
            hypr::bind(&hk)?;
            if let Some(h) = hk_full {
                if let Err(e) = hypr::bind_full(&h) {
                    eprintln!("fullscreen hyprctl bind: {e:#}");
                }
            }
            None
        }
        De::Kde => Some(kde::KdeHotkey::start(proxy, &hk, hk_full.as_ref())?),
    };
    Ok(Some(Guard {
        de,
        chord: chord.to_owned(),
        chord_full: full.to_owned(),
        kde,
    }))
}

impl Guard {
    /// Rebind to new chords — settings recorded different hotkeys.
    /// gsettings/swaymsg/hyprctl bindings simply re-issue; KDE re-sends
    /// BindShortcuts on the live portal session. `full` empty disables
    /// the fullscreen binding.
    pub fn rebind(&mut self, chord: &str, full: &str) -> Result<()> {
        if chord == self.chord && full == self.chord_full {
            return Ok(());
        }
        let hk: HotKey = chord
            .parse()
            .with_context(|| format!("bad hotkey {chord:?}"))?;
        let hk_full: Option<HotKey> = if full.trim().is_empty() {
            None
        } else {
            Some(
                full.parse()
                    .with_context(|| format!("bad fullscreen hotkey {full:?}"))?,
            )
        };
        // a cleared fullscreen chord must drop its stale grab — each
        // desktop's disable differs: sway/hypr unbind the old chord,
        // GNOME blanks the binding, KDE's BindShortcuts replaces all rows
        if hk_full.is_none() && !self.chord_full.trim().is_empty() {
            if let Ok(old) = self.chord_full.parse::<HotKey>() {
                let _ = match self.de {
                    De::Gnome => gnome::set_binding_full(""),
                    De::Sway => sway::unbind(&old),
                    De::Hyprland => hypr::unbind(&old),
                    De::Kde => Ok(()), // set_bound rebuilds without it
                };
            }
        }
        match self.de {
            De::Gnome => {
                gnome::bind(&hk)?;
                if let Some(h) = hk_full {
                    gnome::bind_full(&h)?;
                }
            }
            De::Sway => {
                sway::bind(&hk)?;
                if let Some(h) = hk_full {
                    sway::bind_full(&h)?;
                }
            }
            De::Hyprland => {
                hypr::bind(&hk)?;
                if let Some(h) = hk_full {
                    hypr::bind_full(&h)?;
                }
            }
            De::Kde => self
                .kde
                .as_mut()
                .context("kde hotkey session lost")?
                .bind(&hk, hk_full.as_ref())?,
        }
        self.chord = chord.to_owned();
        self.chord_full = full.to_owned();
        Ok(())
    }

    /// Drop the compositor grabs while the settings recorder is armed so
    /// the chords reach the settings window — mirrors what
    /// `GlobalHotKeyManager::unregister` does on X11.
    pub fn suspend(&mut self, suspended: bool) {
        if let Err(e) = self.suspend_inner(suspended) {
            eprintln!("hotkey suspend({suspended}): {e:#}");
        }
    }

    fn suspend_inner(&mut self, suspended: bool) -> Result<()> {
        let hk: HotKey = self.chord.parse().context("stored chord")?;
        let hk_full: Option<HotKey> = if self.chord_full.trim().is_empty() {
            None
        } else {
            Some(self.chord_full.parse().context("stored full chord")?)
        };
        match (self.de, suspended) {
            // the only per-key switch gsd offers without removing the
            // entry is writing an empty binding
            (De::Gnome, true) => {
                gnome::set_binding("")?;
                if hk_full.is_some() {
                    gnome::set_binding_full("")?;
                }
                Ok(())
            }
            (De::Gnome, false) => {
                gnome::set_binding(&gnome_chord(&hk)?)?;
                if let Some(h) = hk_full {
                    gnome::set_binding_full(&gnome_chord(&h)?)?;
                }
                Ok(())
            }
            (De::Sway, true) => {
                sway::unbind(&hk)?;
                if let Some(h) = hk_full {
                    sway::unbind(&h)?;
                }
                Ok(())
            }
            (De::Sway, false) => {
                sway::bind(&hk)?;
                if let Some(h) = hk_full {
                    sway::bind_full(&h)?;
                }
                Ok(())
            }
            (De::Hyprland, true) => {
                hypr::unbind(&hk)?;
                if let Some(h) = hk_full {
                    hypr::unbind(&h)?;
                }
                Ok(())
            }
            (De::Hyprland, false) => {
                hypr::bind(&hk)?;
                if let Some(h) = hk_full {
                    hypr::bind_full(&h)?;
                }
                Ok(())
            }
            // BindShortcuts([]) unbinds all — the closest the portal
            // has to suspend/resume
            (De::Kde, s) => self
                .kde
                .as_mut()
                .context("kde hotkey session lost")?
                .set_bound(!s),
        }
    }
}

// ------------------------------------------------------------- chords

fn cmd() -> Result<String> {
    let exe = std::env::current_exe().context("current_exe")?;
    Ok(format!("\"{}\" --capture", exe.display()))
}

/// One-shot fullscreen command — the spawned `--capture-full` saves the
/// whole virtual screen without the overlay.
fn cmd_full() -> Result<String> {
    let exe = std::env::current_exe().context("current_exe")?;
    Ok(format!("\"{}\" --capture-full", exe.display()))
}

/// X keysym name — what gsettings bindings, `swaymsg bindsym` and
/// hyprland's `bind` all consume.
fn xkey(code: Code) -> Option<String> {
    let dbg = format!("{code:?}");
    // KeyA..KeyZ -> "a".. ; Digit0..Digit9 -> "0".. ; F1..F24 -> "F1"..
    if let Some(k) = dbg.strip_prefix("Key") {
        return Some(k.to_ascii_lowercase());
    }
    if let Some(d) = dbg.strip_prefix("Digit") {
        return Some(d.to_owned());
    }
    if let Some(f) = dbg.strip_prefix('F') {
        if !f.is_empty() && f.chars().all(|c| c.is_ascii_digit()) {
            return Some(format!("F{f}"));
        }
    }
    Some(
        match code {
            Code::Backquote => "grave",
            Code::Backslash => "backslash",
            Code::Backspace => "BackSpace",
            Code::BracketLeft => "bracketleft",
            Code::BracketRight => "bracketright",
            Code::CapsLock => "Caps_Lock",
            Code::Comma => "comma",
            Code::Delete => "Delete",
            Code::ArrowDown => "Down",
            Code::End => "End",
            Code::Enter => "Return",
            Code::Equal => "equal",
            Code::Escape => "Escape",
            Code::Home => "Home",
            Code::Insert => "Insert",
            Code::ArrowLeft => "Left",
            Code::Minus => "minus",
            Code::NumLock => "Num_Lock",
            Code::PageDown => "Page_Down",
            Code::PageUp => "Page_Up",
            Code::Period => "period",
            Code::PrintScreen => "Print",
            Code::Quote => "apostrophe",
            Code::ArrowRight => "Right",
            Code::ScrollLock => "Scroll_Lock",
            Code::Semicolon => "semicolon",
            Code::Slash => "slash",
            Code::Space => "space",
            Code::Tab => "Tab",
            Code::ArrowUp => "Up",
            _ => return None,
        }
        .to_owned(),
    )
}

/// Qt/QKeySequence-style name — the GlobalShortcuts portal's
/// `preferred_trigger` format on KDE (kglobalaccel).
fn qtkey(code: Code) -> Option<String> {
    let dbg = format!("{code:?}");
    if let Some(k) = dbg.strip_prefix("Key") {
        return Some(k.to_owned());
    }
    if let Some(d) = dbg.strip_prefix("Digit") {
        return Some(d.to_owned());
    }
    if let Some(f) = dbg.strip_prefix('F') {
        if !f.is_empty() && f.chars().all(|c| c.is_ascii_digit()) {
            return Some(format!("F{f}"));
        }
    }
    Some(
        match code {
            Code::Backquote => "`",
            Code::Backslash => "\\",
            Code::Backspace => "Backspace",
            Code::BracketLeft => "[",
            Code::BracketRight => "]",
            Code::CapsLock => "CapsLock",
            Code::Comma => ",",
            Code::Delete => "Del",
            Code::ArrowDown => "Down",
            Code::End => "End",
            Code::Enter => "Return",
            Code::Equal => "=",
            Code::Escape => "Esc",
            Code::Home => "Home",
            Code::Insert => "Ins",
            Code::ArrowLeft => "Left",
            Code::Minus => "-",
            Code::NumLock => "NumLock",
            Code::PageDown => "PgDown",
            Code::PageUp => "PgUp",
            Code::Period => ".",
            Code::PrintScreen => "Print",
            Code::Quote => "'",
            Code::ArrowRight => "Right",
            Code::ScrollLock => "ScrollLock",
            Code::Semicolon => ";",
            Code::Slash => "/",
            Code::Space => "Space",
            Code::Tab => "Tab",
            Code::ArrowUp => "Up",
            _ => return None,
        }
        .to_owned(),
    )
}

fn mods_of(hk: &HotKey) -> (bool, bool, bool, bool) {
    let m = hk.mods;
    (
        m.contains(Modifiers::CONTROL),
        m.contains(Modifiers::ALT),
        m.contains(Modifiers::SHIFT),
        m.contains(Modifiers::SUPER),
    )
}

/// GTK accelerator syntax: `<Control><Shift>s`.
fn gnome_chord(hk: &HotKey) -> Result<String> {
    let (ctrl, alt, shift, sup) = mods_of(hk);
    let mut s = String::new();
    if ctrl {
        s.push_str("<Control>");
    }
    if alt {
        s.push_str("<Alt>");
    }
    if shift {
        s.push_str("<Shift>");
    }
    if sup {
        s.push_str("<Super>");
    }
    s.push_str(&xkey(hk.key).context("no GTK keysym for key")?);
    Ok(s)
}

/// `bindsym` syntax: `Ctrl+Shift+s` (Mod4 = super).
fn sway_chord(hk: &HotKey) -> Result<String> {
    let (ctrl, alt, shift, sup) = mods_of(hk);
    let mut parts = Vec::new();
    if ctrl {
        parts.push("Ctrl");
    }
    if alt {
        parts.push("Alt");
    }
    if shift {
        parts.push("Shift");
    }
    if sup {
        parts.push("Mod4");
    }
    let key = xkey(hk.key).context("no keysym for key")?;
    parts.push(&key);
    Ok(parts.join("+"))
}

/// `hyprctl keyword bind` LHS: `CTRL SHIFT, s`.
fn hypr_chord(hk: &HotKey) -> Result<String> {
    let (ctrl, alt, shift, sup) = mods_of(hk);
    let mut mods = Vec::new();
    if ctrl {
        mods.push("CTRL");
    }
    if alt {
        mods.push("ALT");
    }
    if shift {
        mods.push("SHIFT");
    }
    if sup {
        mods.push("SUPER");
    }
    let key = xkey(hk.key).context("no keysym for key")?;
    Ok(format!("{}, {}", mods.join(" "), key))
}

/// kglobalaccel trigger: `CTRL+SHIFT+S`.
fn kde_trigger(hk: &HotKey) -> Result<String> {
    let (ctrl, alt, shift, sup) = mods_of(hk);
    let mut parts = Vec::new();
    if ctrl {
        parts.push("CTRL");
    }
    if alt {
        parts.push("ALT");
    }
    if shift {
        parts.push("SHIFT");
    }
    if sup {
        parts.push("META");
    }
    let key = qtkey(hk.key).context("no Qt key name")?;
    parts.push(&key);
    Ok(parts.join("+"))
}

// ------------------------------------------------------------- gsettings

mod gnome {
    use super::*;

    const SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";
    const BIND_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
    const PATH: &str =
        "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/quickshort0/";
    const PATH_FULL: &str =
        "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/quickshort1/";

    fn gsettings(args: &[&str]) -> Result<String> {
        let out = Command::new("gsettings")
            .args(args)
            .output()
            .context("spawn gsettings")?;
        if !out.status.success() {
            bail!(
                "gsettings {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    /// "['/a/', '/b/']" (or "@as []") -> the quoted members.
    pub fn parse_str_list(s: &str) -> Vec<String> {
        s.split('\'')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect()
    }

    /// Write one relocatable custom-keybinding entry: make sure its path
    /// is registered under custom-keybindings, then set name/binding/
    /// command. `PATH`/`PATH_FULL` are our two slots.
    fn write_entry(path: &str, name: &str, binding: &str, command: &str) -> Result<()> {
        let raw = gsettings(&["get", SCHEMA, "custom-keybindings"])?;
        let mut paths = parse_str_list(&raw);
        if !paths.iter().any(|p| p == path) {
            paths.push(path.to_owned());
            let list = format!(
                "[{}]",
                paths
                    .iter()
                    .map(|p| format!("'{p}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            gsettings(&["set", SCHEMA, "custom-keybindings", &list])?;
        }
        let path_arg = format!(":{path}");
        for (key, val) in [
            ("name", format!("'{name}'")),
            ("binding", format!("'{binding}'")),
            ("command", format!("'{command}'")),
        ] {
            gsettings(&["set", BIND_SCHEMA, &path_arg, key, &val])?;
        }
        Ok(())
    }

    pub fn bind(hk: &HotKey) -> Result<()> {
        write_entry(PATH, "Quickshort capture", &gnome_chord(hk)?, &cmd()?)
    }

    /// Fullscreen chord — its own keybinding slot so the two stay
    /// independent (suspend blanks each separately).
    pub fn bind_full(hk: &HotKey) -> Result<()> {
        write_entry(
            PATH_FULL,
            "Quickshort fullscreen",
            &gnome_chord(hk)?,
            &cmd_full()?,
        )
    }

    fn set_binding_at(path: &str, binding: &str) -> Result<()> {
        let path_arg = format!(":{path}");
        gsettings(&[
            "set",
            BIND_SCHEMA,
            &path_arg,
            "binding",
            &format!("'{binding}'"),
        ])?;
        Ok(())
    }

    pub fn set_binding(binding: &str) -> Result<()> {
        set_binding_at(PATH, binding)
    }

    pub fn set_binding_full(binding: &str) -> Result<()> {
        set_binding_at(PATH_FULL, binding)
    }
}

// ------------------------------------------------------------- sway / hyprland

fn run(cmd: &mut Command) -> Result<()> {
    let out = cmd
        .output()
        .with_context(|| format!("spawn {:?}", cmd.get_program()))?;
    if out.status.success() {
        Ok(())
    } else {
        bail!(
            "{:?}: {}",
            cmd.get_program(),
            String::from_utf8_lossy(&out.stderr).trim()
        )
    }
}

mod sway {
    use super::*;

    pub fn bind(hk: &HotKey) -> Result<()> {
        let chord = sway_chord(hk)?;
        let c = cmd()?;
        // unbindsym first — a stale bindsym on the same combo would stack
        let _ = unbind(hk);
        run(Command::new("swaymsg")
            .arg("bindsym")
            .arg(&chord)
            .arg("exec")
            .arg(&c))
    }

    pub fn unbind(hk: &HotKey) -> Result<()> {
        run(Command::new("swaymsg")
            .arg("unbindsym")
            .arg(sway_chord(hk)?))
    }

    /// Fullscreen chord → `quickshort --capture-full`.
    pub fn bind_full(hk: &HotKey) -> Result<()> {
        let chord = sway_chord(hk)?;
        let c = cmd_full()?;
        let _ = unbind(hk);
        run(Command::new("swaymsg")
            .arg("bindsym")
            .arg(&chord)
            .arg("exec")
            .arg(&c))
    }
}

mod hypr {
    use super::*;

    /// `hyprctl keyword bind 'CTRL SHIFT, s, exec, "<exe>" --capture'`
    pub fn bind(hk: &HotKey) -> Result<()> {
        run(Command::new("hyprctl")
            .arg("keyword")
            .arg("bind")
            .arg(format!("{}, exec, {}", hypr_chord(hk)?, cmd()?)))
    }

    pub fn unbind(hk: &HotKey) -> Result<()> {
        run(Command::new("hyprctl")
            .arg("keyword")
            .arg("unbind")
            .arg(hypr_chord(hk)?))
    }

    /// Fullscreen chord → `quickshort --capture-full`.
    pub fn bind_full(hk: &HotKey) -> Result<()> {
        run(Command::new("hyprctl")
            .arg("keyword")
            .arg("bind")
            .arg(format!("{}, exec, {}", hypr_chord(hk)?, cmd_full()?)))
    }
}

// ------------------------------------------------------------- KDE portal

mod kde {
    use super::*;
    use futures_lite::StreamExt;
    use std::collections::HashMap;
    use zbus::blocking::{Connection, Proxy};
    use zbus::proxy::SignalStream;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    const DESKTOP: &str = "org.freedesktop.portal.Desktop";
    const ROOT: &str = "/org/freedesktop/portal/desktop";
    const GS: &str = "org.freedesktop.portal.GlobalShortcuts";
    const SESSION: &str = "org.freedesktop.portal.Session";
    const REQUEST: &str = "org.freedesktop.portal.Request";
    const RESPONSE_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

    type Dict = HashMap<String, OwnedValue>;
    type Opts = HashMap<String, Value<'static>>;
    /// a(sa{sv}) — one row per shortcut.
    type ShortcutRow = (String, Opts);

    /// Keeps the portal session + its Activated listener thread alive;
    /// the thread exits with the connection when this drops.
    pub struct KdeHotkey {
        conn: Connection,
        session: OwnedObjectPath,
        /// last chords bound — BindShortcuts([]) unbinds, so resume
        /// needs them. `hk_full` = the fullscreen shortcut (if any).
        hk: HotKey,
        hk_full: Option<HotKey>,
    }

    impl KdeHotkey {
        pub fn start(
            proxy: EventLoopProxy<HostEvent>,
            hk: &HotKey,
            hk_full: Option<&HotKey>,
        ) -> Result<Self> {
            let conn = Connection::session().context("session dbus")?;
            let gs = Proxy::new(&conn, DESKTOP, ROOT, GS)?;

            let tok = token();
            let mut rx = response_rx(&conn, &request_path(&conn, &tok))?;
            let mut opts = Opts::new();
            opts.insert("handle_token".into(), vstr(&tok));
            opts.insert("session_handle_token".into(), vstr(&token()));
            let _: OwnedObjectPath = gs
                .call("CreateSession", &(opts,))
                .context("GlobalShortcuts CreateSession")?;
            let res = wait_response(&mut rx, "CreateSession")?;
            let session = OwnedObjectPath::try_from(
                res.get("session_handle")
                    .and_then(|v| String::try_from(v.clone()).ok())
                    .context("CreateSession gave no session_handle")?,
            )
            .context("bad session_handle")?;

            // Activated fires on the session object — one listener
            // thread for the app's lifetime; the signal's shortcut_id
            // picks between "capture" (region) and "capture_full".
            let mut sig = async_io::block_on(async {
                zbus::Proxy::new(conn.inner(), DESKTOP, session.as_str(), GS)
                    .await?
                    .receive_signal("Activated")
                    .await
            })
            .context("subscribe Activated")?;
            std::thread::Builder::new()
                .name("qs-hotkey".into())
                .spawn(move || {
                    while let Some(msg) = async_io::block_on(sig.next()) {
                        // (shortcut_id, timestamp, options) — the session
                        // path leads the spec signature but kglobalaccel
                        // omits it; default to the region trigger so an
                        // unparsed signal still captures something
                        let id = msg
                            .body()
                            .deserialize::<(String, u64, HashMap<String, OwnedValue>)>()
                            .map(|(id, _, _)| id)
                            .unwrap_or_else(|_| "capture".to_owned());
                        let ev = if id == "capture_full" {
                            HostEvent::TriggerFull
                        } else {
                            HostEvent::Trigger
                        };
                        let _ = proxy.send_event(ev);
                    }
                })
                .context("spawn hotkey listener")?;

            let mut k = Self {
                conn,
                session,
                hk: *hk,
                hk_full: hk_full.copied(),
            };
            k.bind(hk, hk_full)?;
            Ok(k)
        }

        /// preferred_trigger is a hint — Plasma honors it when free,
        /// and remembers it under our shortcut ids either way.
        pub fn bind(&mut self, hk: &HotKey, hk_full: Option<&HotKey>) -> Result<()> {
            self.hk = *hk;
            self.hk_full = hk_full.copied();
            self.set_bound(true)
        }

        /// BindShortcuts([]) drops all rows — the closest the portal
        /// has to suspend/resume.
        pub fn set_bound(&mut self, bound: bool) -> Result<()> {
            let rows: Vec<ShortcutRow> = if bound {
                let mut rows = Vec::new();
                let mut m = Opts::new();
                m.insert("description".into(), vstr("Capture region"));
                m.insert("preferred_trigger".into(), vstr(&kde_trigger(&self.hk)?));
                rows.push(("capture".to_owned(), m));
                if let Some(h) = self.hk_full {
                    let mut m = Opts::new();
                    m.insert("description".into(), vstr("Capture full screen"));
                    m.insert("preferred_trigger".into(), vstr(&kde_trigger(&h)?));
                    rows.push(("capture_full".to_owned(), m));
                }
                rows
            } else {
                Vec::new()
            };
            let gs = Proxy::new(&self.conn, DESKTOP, ROOT, GS)?;
            let tok = token();
            let mut rx = response_rx(&self.conn, &request_path(&self.conn, &tok))?;
            let mut opts = Opts::new();
            opts.insert("handle_token".into(), vstr(&tok));
            let _: OwnedObjectPath = gs
                .call("BindShortcuts", &(self.session.clone(), rows, "", opts))
                .context("GlobalShortcuts BindShortcuts")?;
            wait_response(&mut rx, "BindShortcuts")?;
            Ok(())
        }
    }

    impl Drop for KdeHotkey {
        fn drop(&mut self) {
            // best-effort — the portal reaps the session with the
            // connection anyway
            let _ = Proxy::new(&self.conn, DESKTOP, self.session.as_str(), SESSION)
                .and_then(|p| p.call::<_, _, ()>("Close", &()));
        }
    }

    // --- request/response plumbing (same protocol as qs-capture's
    //     screencast portal: predict the Request path from handle_token,
    //     subscribe BEFORE the call, wait for its Response signal)

    fn token() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!(
            "qs{nanos:x}{:x}",
            std::process::id() as u64 * 1000 + N.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn vstr(s: &str) -> Value<'static> {
        Value::from(s.to_owned())
    }

    fn request_path(conn: &Connection, tok: &str) -> String {
        let sender = conn
            .unique_name()
            .map(|n| n.as_str().trim_start_matches(':').replace('.', "_"))
            .unwrap_or_default();
        format!("{ROOT}/request/{sender}/{tok}")
    }

    fn response_rx(conn: &Connection, path: &str) -> Result<SignalStream<'static>> {
        Ok(async_io::block_on(async {
            let p = zbus::Proxy::new(conn.inner(), DESKTOP, path, REQUEST).await?;
            p.receive_signal("Response").await
        })?)
    }

    fn wait_response(rx: &mut SignalStream<'static>, what: &str) -> Result<Dict> {
        let msg = async_io::block_on(futures_lite::future::or(
            async move { rx.next().await },
            async {
                async_io::Timer::after(RESPONSE_WAIT).await;
                None
            },
        ))
        .with_context(|| format!("portal {what}: no Response (timeout)"))?;
        let (code, results): (u32, Dict) = msg.body().deserialize()?;
        match code {
            0 => Ok(results),
            1 => bail!("portal {what}: user cancelled"),
            c => bail!("portal {what}: response code {c}"),
        }
    }
}

// ------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn hk(s: &str) -> HotKey {
        s.parse().unwrap()
    }

    #[test]
    fn gnome_syntax() {
        assert_eq!(
            gnome_chord(&hk("Ctrl+Shift+S")).unwrap(),
            "<Control><Shift>s"
        );
        assert_eq!(
            gnome_chord(&hk("Ctrl+Alt+Super+F5")).unwrap(),
            "<Control><Alt><Super>F5"
        );
        assert_eq!(gnome_chord(&hk("Ctrl+Up")).unwrap(), "<Control>Up");
        assert_eq!(
            gnome_chord(&hk("Shift+PageDown")).unwrap(),
            "<Shift>Page_Down"
        );
        assert_eq!(gnome_chord(&hk("Ctrl+Space")).unwrap(), "<Control>space");
        assert_eq!(gnome_chord(&hk("PrintScreen")).unwrap(), "Print");
        // numpad keys have no mapping — caller bails to the manual hint
        assert!(gnome_chord(&hk("Ctrl+NumpadAdd")).is_err());
    }

    #[test]
    fn sway_syntax() {
        assert_eq!(sway_chord(&hk("Ctrl+Shift+S")).unwrap(), "Ctrl+Shift+s");
        assert_eq!(sway_chord(&hk("Super+Alt+F2")).unwrap(), "Alt+Mod4+F2");
        assert_eq!(sway_chord(&hk("Ctrl+Minus")).unwrap(), "Ctrl+minus");
    }

    #[test]
    fn hypr_syntax() {
        assert_eq!(hypr_chord(&hk("Ctrl+Shift+S")).unwrap(), "CTRL SHIFT, s");
        assert_eq!(hypr_chord(&hk("Super+Q")).unwrap(), "SUPER, q");
        assert_eq!(hypr_chord(&hk("Ctrl+Enter")).unwrap(), "CTRL, Return");
    }

    #[test]
    fn kde_syntax() {
        assert_eq!(kde_trigger(&hk("Ctrl+Shift+S")).unwrap(), "CTRL+SHIFT+S");
        assert_eq!(kde_trigger(&hk("Ctrl+Alt+Delete")).unwrap(), "CTRL+ALT+Del");
        assert_eq!(kde_trigger(&hk("Shift+PageUp")).unwrap(), "SHIFT+PgUp");
        assert_eq!(kde_trigger(&hk("Ctrl+F12")).unwrap(), "CTRL+F12");
    }

    #[test]
    fn gvariant_list_parsing() {
        assert_eq!(
            gnome::parse_str_list("['/a/', '/b/']"),
            vec!["/a/".to_owned(), "/b/".to_owned()]
        );
        assert!(gnome::parse_str_list("@as []").is_empty());
        assert!(gnome::parse_str_list("[]").is_empty());
    }
}
