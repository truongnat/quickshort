//! Preferences window — macOS System Settings language: grouped inset cards,
//! keycap glyphs, a real sliding switch. Same stack as the overlay: winit
//! window, softbuffer pixels, SDF widgets, fontdue text (system font looked
//! up at runtime, no bundled assets).

use anyhow::{Context, Result};
use fontdue::{Font, FontSettings};
use qs_overlay::sdf::{ellipse_sdf, fill_cov, glass_card, rrect_sdf, seg_sdf, stroke_cov, Canvas};
use softbuffer::{Context as SbContext, Surface};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Icon, Window, WindowId};

use crate::actions;
use crate::config::Config;
use crate::tray;

const W: f64 = 480.0;
const H: f64 = 468.0;

// macOS-consistent dark palette (matches the overlay toolbar)
const BG_TOP: u32 = 0x212123; // window gradient, top — lifted like Tahoe chrome
const BG_BOT: u32 = 0x17171A;
const CARD_TOP: u32 = 0x303035; // glass card — light hits the top edge
const CARD_BOT: u32 = 0x26262B;
const ROW_HOV: u32 = 0x343438;
const KEYCAP: u32 = 0x3A3A3D; // raised key top
const KEYCAP_EDGE: u32 = 0x545456; // key bottom edge
const ACCENT: u32 = 0x0A84FF;
const FG: u32 = 0xF2F2F7;
const FG_DIM: u32 = 0x98989D;
const FG_FAINT: u32 = 0x636366;
const SWITCH_OFF: u32 = 0x39393D;

/// What the host needs to know after a settings event.
pub enum SettingsEvent {
    /// Config changed + saved — re-register the hotkey, rebuild the menu.
    Changed,
    /// Hotkey recording toggled on/off — host must suspend the global
    /// grab while recording so the chord reaches this window instead of
    /// firing the capture handler.
    HotkeyRecord,
    /// User closed the window.
    Closed,
}

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    Record,
    Browse,
    LoginToggle,
    Recent(usize),
}

type LRect = (f64, f64, f64, f64); // logical x, y, w, h

// layout -------------------------------------------------------------------

const MARGIN: f64 = 20.0;
const CARD_RAD: f64 = 14.0;
const ROW_H: f64 = 44.0;
const COL_LBL_X: f64 = MARGIN + 14.0;
const RECENT_ROW_H: f64 = 34.0;
const RECENT_N: usize = 4;

// vertical rhythm: header, then (section label, card) groups
const SEC1_LBL: f64 = 92.0; // "Capture" baseline
const CARD1_Y: f64 = SEC1_LBL + 8.0;
const SEC2_LBL: f64 = CARD1_Y + ROW_H * 2.0 + 30.0; // "General"
const CARD2_Y: f64 = SEC2_LBL + 8.0;
const SEC3_LBL: f64 = CARD2_Y + ROW_H + 30.0; // "Recent captures"
const CARD3_Y: f64 = SEC3_LBL + 8.0;

fn card_capture() -> LRect {
    (MARGIN, CARD1_Y, W - MARGIN * 2.0, ROW_H * 2.0)
}
fn card_general() -> LRect {
    (MARGIN, CARD2_Y, W - MARGIN * 2.0, ROW_H)
}
fn card_recents(n: usize) -> LRect {
    (MARGIN, CARD3_Y, W - MARGIN * 2.0, 12.0 + n.max(1) as f64 * RECENT_ROW_H)
}
fn recent_row(i: usize) -> LRect {
    (MARGIN + 6.0, CARD3_Y + 6.0 + i as f64 * RECENT_ROW_H, W - MARGIN * 2.0 - 12.0, RECENT_ROW_H)
}
fn record_btn() -> LRect {
    (W - MARGIN - 14.0 - 68.0, CARD1_Y + 7.0, 68.0, 30.0)
}
fn browse_btn() -> LRect {
    (W - MARGIN - 14.0 - 68.0, CARD1_Y + ROW_H + 7.0, 68.0, 30.0)
}
fn switch_rect() -> LRect {
    (W - MARGIN - 14.0 - 44.0, CARD2_Y + 9.0, 44.0, 26.0)
}

// ---------------------------------------------------------------------------

pub struct Settings {
    win: Rc<Window>,
    surf: Surface<Rc<Window>, Rc<Window>>,
    font: Font,
    recording: bool,
    hover: Option<Hit>,
    mods: ModifiersState,
    recents: Vec<PathBuf>,
    last_pos: (f64, f64),
    /// switch knob position 0..1, eases toward `switch_target`
    switch_t: f64,
    switch_target: f64,
    /// recording pulse phase (seconds-ish, advanced per frame)
    pulse: f64,
    /// a bare key was pressed while recording — hint that a modifier is required
    want_mod: bool,
    /// folder picker is open — guard against re-entry (a queued second
    /// click would stack another modal on top)
    dialog_open: bool,
    /// app logo at header size (physical px)
    logo: Option<image::RgbaImage>,
    dirty: bool,
    closed: bool,
}

fn app_logo() -> Option<image::RgbaImage> {
    image::load_from_memory(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/icon-256.png"
    )))
    .ok()
    .map(|i| i.to_rgba8())
}

impl Settings {
    pub fn open(el: &ActiveEventLoop, cfg: &Config) -> Result<Self> {
        let logo = app_logo();
        let mut attrs = Window::default_attributes()
            .with_title("Quickshort Settings")
            .with_inner_size(winit::dpi::LogicalSize::new(W, H))
            .with_resizable(false)
            .with_window_level(winit::window::WindowLevel::AlwaysOnTop);
        if let Some(img) = &logo {
            if let Ok(ic) = Icon::from_rgba(img.as_raw().clone(), img.width(), img.height()) {
                attrs = attrs.with_window_icon(Some(ic));
            }
        }
        let win = Rc::new(el.create_window(attrs).context("settings window")?);
        let scale = win.scale_factor();
        let logo_px = (36.0 * scale) as u32;
        let logo = logo.map(|i| {
            image::imageops::resize(&i, logo_px, logo_px, image::imageops::FilterType::Lanczos3)
        });
        win.set_cursor(CursorIcon::Default);
        let surf = SbContext::new(win.clone())
            .and_then(|c| Surface::new(&c, win.clone()))
            .map_err(|e| anyhow::anyhow!("settings surface: {e}"))?;
        Ok(Self {
            win,
            surf,
            font: system_font()?,
            recording: false,
            hover: None,
            mods: ModifiersState::empty(),
            recents: actions::recent_captures(&cfg.save_dir(), RECENT_N),
            last_pos: (0.0, 0.0),
            switch_t: cfg.launch_at_login as u8 as f64,
            switch_target: cfg.launch_at_login as u8 as f64,
            pulse: 0.0,
            want_mod: false,
            dialog_open: false,
            logo,
            dirty: true,
            closed: false,
        })
    }

    pub fn win_id(&self) -> WindowId {
        self.win.id()
    }

    pub fn win(&self) -> &Window {
        &self.win
    }

    /// True while the Record button is armed — the host uses this to
    /// suspend the global hotkey grab so the chord reaches this window.
    pub fn is_recording(&self) -> bool {
        self.recording
    }

    pub fn about_to_wait(&self) {
        if self.dirty {
            self.win.request_redraw();
        }
    }

    /// Handle one event; `Some` means the host should act.
    pub fn window_event(&mut self, cfg: &mut Config, ev: WindowEvent) -> Option<SettingsEvent> {
        match ev {
            WindowEvent::CloseRequested => self.closed = true,
            WindowEvent::RedrawRequested => self.draw(cfg),
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::CursorMoved { position, .. } => {
                let s = self.win.scale_factor();
                let p = (position.x / s, position.y / s);
                self.last_pos = p;
                let h = self.hit(p);
                if h != self.hover {
                    self.hover = h;
                    self.win.set_cursor(if h.is_some() {
                        CursorIcon::Pointer
                    } else {
                        CursorIcon::Default
                    });
                    self.dirty = true;
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => match self.hit(self.last_pos) {
                Some(Hit::Record) => {
                    self.recording = !self.recording;
                    self.want_mod = false;
                    self.dirty = true;
                    return Some(SettingsEvent::HotkeyRecord);
                }
                Some(Hit::Browse) => {
                    if !self.dialog_open {
                        self.dialog_open = true;
                        let picked = rfd::FileDialog::new()
                            .set_directory(cfg.save_dir())
                            .set_parent(&*self.win)
                            .pick_folder();
                        self.dialog_open = false;
                        // an app-modal panel deactivates the accessory app —
                        // pull focus back or the window looks dead
                        refocus(&self.win);
                        self.dirty = true;
                        if let Some(d) = picked {
                            cfg.save_dir = Some(d);
                            self.recents = actions::recent_captures(&cfg.save_dir(), RECENT_N);
                            return Some(SettingsEvent::Changed);
                        }
                    }
                }
                Some(Hit::LoginToggle) => {
                    let want = !cfg.launch_at_login;
                    match tray::set_launch_at_login(want) {
                        Ok(()) => {
                            cfg.launch_at_login = want;
                            self.switch_target = want as u8 as f64;
                            self.dirty = true;
                            return Some(SettingsEvent::Changed);
                        }
                        Err(e) => eprintln!("launch-at-login: {e:#}"),
                    }
                }
                Some(Hit::Recent(i)) => {
                    if let Some(p) = self.recents.get(i) {
                        actions::open_path(p);
                    }
                }
                None => {
                    if self.recording {
                        self.recording = false;
                        self.dirty = true;
                        return Some(SettingsEvent::HotkeyRecord);
                    }
                }
            },
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
                    if self.recording {
                        self.recording = false;
                        self.dirty = true;
                        return Some(SettingsEvent::HotkeyRecord);
                    }
                    self.closed = true;
                    return None;
                }
                if self.recording {
                    if let Some(hk) = self.chord_string(&event.logical_key) {
                        if hk.parse::<global_hotkey::hotkey::HotKey>().is_err() {
                            return None; // unparsable — keep recording
                        }
                        cfg.hotkey = hk;
                        self.recording = false;
                        self.dirty = true;
                        return Some(SettingsEvent::Changed);
                    }
                    // a real key but no modifier — tell the user why nothing happened
                    if key_token(&event.logical_key).is_some() {
                        self.want_mod = true;
                        self.dirty = true;
                    }
                }
            }
            _ => {}
        }
        self.closed.then_some(SettingsEvent::Closed)
    }

    /// Compose a `global-hotkey` string from current modifiers + key.
    /// Modifier-only presses keep recording (wait for the real key).
    fn chord_string(&self, key: &Key) -> Option<String> {
        let token = key_token(key)?;
        let mut s = String::new();
        if self.mods.super_key() {
            s.push_str("CmdOrCtrl+");
        }
        if self.mods.control_key() {
            s.push_str("Ctrl+");
        }
        if self.mods.alt_key() {
            s.push_str("Alt+");
        }
        if self.mods.shift_key() {
            s.push_str("Shift+");
        }
        if s.is_empty() {
            return None; // a bare key is a bad global hotkey — keep waiting
        }
        Some(format!("{s}{token}"))
    }

    fn hit(&self, p: (f64, f64)) -> Option<Hit> {
        let (x, y) = p;
        let inside = |r: LRect| x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3;
        if inside(record_btn()) {
            return Some(Hit::Record);
        }
        if inside(browse_btn()) {
            return Some(Hit::Browse);
        }
        // generous toggle hit area: the whole row
        let g = card_general();
        if inside((g.0, g.1, g.2, g.3)) {
            return Some(Hit::LoginToggle);
        }
        for i in 0..self.recents.len() {
            if inside(recent_row(i)) {
                return Some(Hit::Recent(i));
            }
        }
        None
    }

    fn draw(&mut self, cfg: &Config) {
        // advance animations; keep repainting while anything is moving
        let animating = (self.switch_t - self.switch_target).abs() > 0.001 || self.recording;
        self.switch_t += (self.switch_target - self.switch_t) * 0.28;
        self.pulse += 0.08;
        self.dirty = animating;

        let size = self.win.inner_size();
        let (sw, sh) = match (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            (Some(w), Some(h)) => (w.get() as usize, h.get() as usize),
            _ => return,
        };
        if self
            .surf
            .resize(
                NonZeroU32::new(sw as u32).unwrap(),
                NonZeroU32::new(sh as u32).unwrap(),
            )
            .is_err()
        {
            return;
        }
        // hoist state — buffer_mut holds &mut self.surf for the rest of the fn
        let (hover, recording, pulse, switch_t, want_mod) = (
            self.hover,
            self.recording,
            self.pulse,
            self.switch_t,
            self.want_mod,
        );
        let recents = self.recents.clone();
        let Ok(mut buf) = self.surf.buffer_mut() else {
            return;
        };
        // window backdrop: faint vertical gradient, brighter at the top
        for (row, px) in buf.iter_mut().enumerate() {
            let t = (row / sw) as f64 / sh.max(1) as f64;
            *px = mix(BG_TOP, BG_BOT, t);
        }
        let scale = self.win.scale_factor();
        let mut cv = Canvas {
            buf: &mut buf,
            w: sw,
            h: sh,
            scale,
        };
        let p = Painter {
            font: &self.font,
            scale,
        };

        // header: app logo + name + version
        if let Some(logo) = &self.logo {
            let (ox, oy) = ((MARGIN * scale) as i32, (22.0 * scale) as i32);
            for (px, py, c) in logo.enumerate_pixels() {
                let a = c[3] as f64 / 255.0;
                if a <= 0.0 {
                    continue;
                }
                let rgb = ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32;
                cv.blend(ox + px as i32, oy + py as i32, rgb, a);
            }
        } else {
            p.icon(&mut cv, MARGIN, 22.0, 36.0);
        }
        p.text(&mut cv, MARGIN + 48.0, 48.0, 17.0, "Quickshort", FG);
        let ver = concat!("v", env!("CARGO_PKG_VERSION"));
        p.text(
            &mut cv,
            MARGIN + 48.0 + p.text_w("Quickshort", 17.0) + 8.0,
            48.0,
            11.5,
            ver,
            FG_FAINT,
        );

        // ── capture ─────────────────────────────────────────────────────
        p.section(&mut cv, MARGIN + 2.0, SEC1_LBL, "Capture");
        let c1 = card_capture();
        p.card(&mut cv, c1);
        let (r1y, r2y) = (c1.1, c1.1 + ROW_H);
        // row 1: hotkey
        p.text(&mut cv, COL_LBL_X, r1y + 28.0, 13.0, "Global hotkey", FG);
        let rb = record_btn();
        if recording {
            let a = 0.55 + 0.45 * pulse.sin().abs();
            p.stroke(
                &mut cv,
                (c1.0 + 8.0, r1y + 7.0, c1.2 - 16.0, 30.0),
                7.0,
                ACCENT,
                a,
                1.0,
            );
            let (msg, col) = if want_mod {
                ("Needs a modifier, e.g. ⌘ ⌥ ⌃", FG_DIM)
            } else {
                ("Press shortcut", ACCENT)
            };
            let tw = p.text_w(msg, 12.0);
            p.text(&mut cv, rb.0 - tw - 12.0, r1y + 27.0, 12.0, msg, col);
        } else {
            p.keycaps(&mut cv, &cfg.hotkey, rb.0 - keycaps_w(&p, &cfg.hotkey) - 10.0, r1y + 7.0);
        }
        p.ghost_button(&mut cv, rb, "Record", hover == Some(Hit::Record));
        // hairline
        p.line(&mut cv, COL_LBL_X, c1.1 + ROW_H, c1.0 + c1.2 - 14.0);
        // row 2: save folder
        p.text(&mut cv, COL_LBL_X, r2y + 28.0, 13.0, "Save folder", FG);
        let dir = short_path(&cfg.save_dir().display().to_string());
        let bb = browse_btn();
        let tw = p.text_w(&tail(&dir, 20), 11.5);
        p.text(&mut cv, bb.0 - tw - 12.0, r2y + 27.0, 11.5, &tail(&dir, 20), FG_DIM);
        p.ghost_button(&mut cv, bb, "Browse", hover == Some(Hit::Browse));

        // ── general ─────────────────────────────────────────────────────
        p.section(&mut cv, MARGIN + 2.0, SEC2_LBL, "General");
        let c2 = card_general();
        if hover == Some(Hit::LoginToggle) {
            p.fill(&mut cv, c2, CARD_RAD, ROW_HOV, 1.0);
        } else {
            p.card(&mut cv, c2);
        }
        p.text(&mut cv, COL_LBL_X, c2.1 + 28.0, 13.0, "Launch at login", FG);
        p.switch(&mut cv, switch_rect(), switch_t);

        // ── recent captures ─────────────────────────────────────────────
        p.section(&mut cv, MARGIN + 2.0, SEC3_LBL, "Recent captures");
        let c3 = card_recents(recents.len());
        p.card(&mut cv, c3);
        if recents.is_empty() {
            p.text(&mut cv, COL_LBL_X, c3.1 + 36.0, 12.5, "No captures yet", FG_DIM);
        }
        for (i, r) in recents.iter().enumerate() {
            let rr = recent_row(i);
            if hover == Some(Hit::Recent(i)) {
                p.fill(&mut cv, rr, 7.0, ROW_HOV, 1.0);
            }
            let name = r
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            p.text(&mut cv, rr.0 + 8.0, rr.1 + 22.0, 12.5, &name, FG);
            // right-aligned age; on hover the ↗ affordance takes its slot
            let right = if hover == Some(Hit::Recent(i)) {
                p.text(&mut cv, rr.0 + rr.2 - 16.0, rr.1 + 22.0, 12.0, "↗", FG_DIM);
                28.0
            } else {
                8.0
            };
            let age = age_label(r);
            let tw = p.text_w(&age, 10.5);
            p.text(&mut cv, rr.0 + rr.2 - tw - right, rr.1 + 22.0, 10.5, &age, FG_FAINT);
        }

        let _ = buf.present();
    }
}

/// Restore app activation + window focus after a modal panel closes —
/// without this an accessory app stays deactivated and the settings
/// window stops receiving keyboard input.
fn refocus(win: &Window) {
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::NSApplication;
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).activate();
        }
    }
    win.focus_window();
    win.request_redraw();
}

/// right-trimmed path tail for display
fn tail(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let keep: String = s.chars().rev().take(n).collect::<String>();
        format!("…{}", keep.chars().rev().collect::<String>())
    }
}

/// "/Users/x/…" -> "~/…" for display
fn short_path(s: &str) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let h = home.to_string_lossy();
        if s.starts_with(h.as_ref()) {
            return format!("~{}", &s[h.len()..]);
        }
    }
    s.to_string()
}

/// "just now" / "5m ago" / "3h ago" / "2d ago" / "6w ago" from file mtime
fn age_label(p: &PathBuf) -> String {
    let Ok(m) = std::fs::metadata(p).and_then(|m| m.modified()) else {
        return String::new();
    };
    let Ok(d) = m.elapsed() else {
        return String::new();
    };
    let s = d.as_secs();
    if s < 60 {
        "just now".into()
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else if s < 86_400 {
        format!("{}h ago", s / 3600)
    } else if s < 7 * 86_400 {
        format!("{}d ago", s / 86_400)
    } else {
        format!("{}w ago", s / (7 * 86_400))
    }
}

/// total width of a keycap row for `hotkey` (for right-alignment)
fn keycaps_w(p: &Painter, hotkey: &str) -> f64 {
    hotkey_tokens(hotkey)
        .iter()
        .map(|t| (p.text_w(t, 11.5) + 14.0).max(24.0) + 4.0)
        .sum::<f64>()
        - 4.0
}

/// "CmdOrCtrl+Shift+S" -> per-keycap tokens ("⌘", "⇧", "S" on macOS).
fn hotkey_tokens(s: &str) -> Vec<String> {
    s.split('+')
        .map(|t| {
            let t = t.trim();
            #[cfg(target_os = "macos")]
            {
                match t {
                    "CmdOrCtrl" | "Cmd" | "Super" | "Command" => "⌘".to_string(),
                    "Ctrl" | "Control" => "⌃".to_string(),
                    "Alt" | "Option" => "⌥".to_string(),
                    "Shift" => "⇧".to_string(),
                    other => other.to_string(),
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                t.to_string()
            }
        })
        .collect()
}

/// winit `Key` -> global-hotkey key token.
fn key_token(key: &Key) -> Option<String> {
    match key {
        Key::Character(c) => {
            let u = c.to_uppercase();
            (u.chars().count() == 1).then_some(u)
        }
        Key::Named(n) => Some(match n {
            NamedKey::Space => "Space".into(),
            NamedKey::Enter => "Enter".into(),
            NamedKey::Tab => "Tab".into(),
            NamedKey::Backspace => "Backspace".into(),
            NamedKey::Delete => "Delete".into(),
            NamedKey::Insert => "Insert".into(),
            NamedKey::Home => "Home".into(),
            NamedKey::End => "End".into(),
            NamedKey::PageUp => "PageUp".into(),
            NamedKey::PageDown => "PageDown".into(),
            NamedKey::ArrowUp => "Up".into(),
            NamedKey::ArrowDown => "Down".into(),
            NamedKey::ArrowLeft => "Left".into(),
            NamedKey::ArrowRight => "Right".into(),
            NamedKey::PrintScreen => "PrintScreen".into(),
            other => {
                let d = format!("{other:?}");
                if d.starts_with('F') && d[1..].chars().all(|c| c.is_ascii_digit()) {
                    d
                } else {
                    return None;
                }
            }
        }),
        _ => None,
    }
}

/// First usable system font (fontdue reads TTF/OTF) — shared with the toast.
pub(crate) fn system_font() -> Result<Font> {
    const CANDIDATES: &[&str] = &[
        #[cfg(target_os = "macos")]
        "/System/Library/Fonts/SFNS.ttf",
        #[cfg(target_os = "macos")]
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        #[cfg(target_os = "macos")]
        "/System/Library/Fonts/Supplemental/Helvetica.ttf",
        #[cfg(target_os = "windows")]
        "C:\\Windows\\Fonts\\segoeui.ttf",
        #[cfg(target_os = "windows")]
        "C:\\Windows\\Fonts\\arial.ttf",
        #[cfg(target_os = "linux")]
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        #[cfg(target_os = "linux")]
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    ];
    for p in CANDIDATES {
        if let Ok(bytes) = std::fs::read(p) {
            if let Ok(f) = Font::from_bytes(bytes.as_slice(), FontSettings::default()) {
                return Ok(f);
            }
        }
    }
    anyhow::bail!("no usable system font found");
}

/// Dev harness: `quickshort --settings` opens just the preferences window.
pub fn standalone() -> Result<()> {
    struct Run {
        cfg: Config,
        s: Option<Settings>,
    }
    impl winit::application::ApplicationHandler for Run {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            match Settings::open(el, &self.cfg) {
                Ok(s) => self.s = Some(s),
                Err(e) => {
                    eprintln!("settings: {e:#}");
                    el.exit();
                }
            }
        }
        fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, ev: WindowEvent) {
            let Some(s) = &mut self.s else { return };
            if s.win_id() != id {
                return;
            }
            if let Some(e) = s.window_event(&mut self.cfg, ev) {
                match e {
                    SettingsEvent::Changed => self.cfg.save(),
                    SettingsEvent::HotkeyRecord => {} // standalone: no grab to suspend
                    SettingsEvent::Closed => el.exit(),
                }
            }
        }
        fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
            if let Some(s) = &self.s {
                s.about_to_wait();
            }
        }
    }
    let el = winit::event_loop::EventLoop::new()?;
    let mut run = Run {
        cfg: Config::load(),
        s: None,
    };
    el.run_app(&mut run)?;
    Ok(())
}

/// SDF + fontdue drawing helpers over the softbuffer surface.
struct Painter<'a> {
    font: &'a Font,
    scale: f64,
}

impl Painter<'_> {
    fn fill(&self, cv: &mut Canvas, r: LRect, rad: f64, rgb: u32, a: f64) {
        let (x, y, w, h) = r;
        let s = self.scale;
        let (x0, y0) = ((x * s) as i32, (y * s) as i32);
        let (x1, y1) = (((x + w) * s) as i32, ((y + h) * s) as i32);
        for py in y0..y1 {
            for px in x0..x1 {
                let lx = px as f64 / s + 0.5 / s;
                let ly = py as f64 / s + 0.5 / s;
                let d = rrect_sdf(lx, ly, x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0, rad);
                let c = fill_cov(d, s) * a;
                if c > 0.0 {
                    cv.blend(px, py, rgb, c);
                }
            }
        }
    }

    /// Outline stroke of width `w` (logical px) around a rounded rect.
    fn stroke(&self, cv: &mut Canvas, r: LRect, rad: f64, rgb: u32, a: f64, w: f64) {
        let (x, y, rw, rh) = r;
        let s = self.scale;
        let (x0, y0) = ((x * s) as i32 - 2, (y * s) as i32 - 2);
        let (x1, y1) = (((x + rw) * s) as i32 + 2, ((y + rh) * s) as i32 + 2);
        for py in y0.max(0)..y1.min(cv.h as i32) {
            for px in x0.max(0)..x1.min(cv.w as i32) {
                let lx = px as f64 / s + 0.5 / s;
                let ly = py as f64 / s + 0.5 / s;
                let d = rrect_sdf(lx, ly, x + rw / 2.0, y + rh / 2.0, rw / 2.0, rh / 2.0, rad);
                let c = stroke_cov(d, w, s) * a;
                if c > 0.0 {
                    cv.blend(px, py, rgb, c);
                }
            }
        }
    }

    /// Grouped inset card: glass gradient + soft shadow + lit top edge.
    fn card(&self, cv: &mut Canvas, r: LRect) {
        glass_card(cv, r, CARD_RAD, CARD_TOP, CARD_BOT);
    }

    /// Small dim section label above a card.
    fn section(&self, cv: &mut Canvas, x: f64, y: f64, s: &str) {
        self.text(cv, x, y, 11.0, s, FG_DIM);
    }

    /// Text button — hairline border, fills accent on hover.
    fn ghost_button(&self, cv: &mut Canvas, r: LRect, label: &str, hover: bool) {
        if hover {
            self.fill(cv, r, r.3 / 2.0, ACCENT, 1.0);
        } else {
            self.stroke(cv, r, r.3 / 2.0, 0xFFFFFF, 0.25, 0.75);
        }
        let tw = self.text_w(label, 11.5);
        self.text(
            cv,
            r.0 + (r.2 - tw) / 2.0,
            r.1 + r.3 / 2.0 + 4.0,
            11.5,
            label,
            FG,
        );
    }

    /// Keyboard-style keycaps, one per hotkey token.
    fn keycaps(&self, cv: &mut Canvas, hotkey: &str, x: f64, y: f64) {
        let mut px = x;
        for t in hotkey_tokens(hotkey) {
            let tw = self.text_w(&t, 11.5);
            let kw = (tw + 14.0).max(24.0);
            let kr = (px, y, kw, 28.0);
            self.fill(cv, kr, 6.0, KEYCAP, 1.0);
            // key bottom edge — makes it read as a physical key
            self.stroke(
                cv,
                (kr.0, kr.1 + 1.0, kr.2, kr.3 - 1.0),
                6.0,
                KEYCAP_EDGE,
                0.9,
                0.75,
            );
            self.text(cv, px + (kw - tw) / 2.0, y + 19.0, 11.5, &t, FG);
            px += kw + 4.0;
        }
    }

    /// macOS-style sliding switch. `t` = knob position 0..1.
    fn switch(&self, cv: &mut Canvas, r: LRect, t: f64) {
        let track = mix(SWITCH_OFF, ACCENT, t);
        self.fill(cv, r, r.3 / 2.0, track, 1.0);
        let kr = r.3 / 2.0 - 1.5; // knob radius
        let kx = r.0 + r.3 / 2.0 + t * (r.2 - r.3);
        let ky = r.1 + r.3 / 2.0;
        let s = self.scale;
        let (x0, y0) = (((kx - kr) * s) as i32, ((ky - kr) * s) as i32);
        let (x1, y1) = (((kx + kr) * s) as i32, ((ky + kr) * s) as i32);
        for py in y0..y1 {
            for px in x0..x1 {
                let lx = px as f64 / s + 0.5 / s;
                let ly = py as f64 / s + 0.5 / s;
                let c = fill_cov(ellipse_sdf(lx, ly, kx, ky, kr, kr), s);
                if c > 0.0 {
                    cv.blend(px, py, 0xF5F5F7, c);
                }
            }
        }
    }

    /// App glyph — the tray's crop-corners mark in an accent squircle.
    fn icon(&self, cv: &mut Canvas, x: f64, y: f64, size: f64) {
        self.fill(cv, (x, y, size, size), 9.0, ACCENT, 1.0);
        let st = size * 0.10;
        let ins = size * 0.24;
        let len = size * 0.17;
        let (x0, y0, x1, y1) = (x + ins, y + ins, x + size - ins, y + size - ins);
        let segs = [
            (x0, y0 + len, x0, y0),
            (x0, y0, x0 + len, y0),
            (x1 - len, y0, x1, y0),
            (x1, y0, x1, y0 + len),
            (x0, y1 - len, x0, y1),
            (x0, y1, x0 + len, y1),
            (x1 - len, y1, x1, y1),
            (x1, y1 - len, x1, y1),
        ];
        let s = self.scale;
        let (px0, py0) = ((x * s) as i32, (y * s) as i32);
        let (px1, py1) = (((x + size) * s) as i32, ((y + size) * s) as i32);
        for py in py0..py1 {
            for px in px0..px1 {
                let lx = px as f64 / s + 0.5 / s;
                let ly = py as f64 / s + 0.5 / s;
                let mut d = f64::INFINITY;
                for &(ax, ay, bx, by) in &segs {
                    d = d.min(seg_sdf(lx, ly, ax, ay, bx, by));
                }
                let c = stroke_cov(d, st, s);
                if c > 0.0 {
                    cv.blend(px, py, 0xFFFFFF, c);
                }
            }
        }
    }

    fn line(&self, cv: &mut Canvas, x0: f64, y: f64, x1: f64) {
        let s = self.scale;
        for px in (x0 * s) as i32..(x1 * s) as i32 {
            cv.blend(px, (y * s) as i32, 0xFFFFFF, 0.09);
        }
    }

    /// Draw `s` with baseline at logical (x, y); returns advance width.
    fn text(&self, cv: &mut Canvas, x: f64, y: f64, size: f64, s: &str, rgb: u32) -> f64 {
        let scale = self.scale;
        let px = size * scale;
        let mut pen = x * scale;
        let baseline = y * scale;
        for ch in s.chars() {
            let (m, bmp) = self.font.rasterize(ch, px as f32);
            let gx = (pen + m.xmin as f64) as i32;
            let gy = (baseline - m.height as f64 - m.ymin as f64) as i32;
            for row in 0..m.height {
                for col in 0..m.width {
                    let a = bmp[row * m.width + col] as f64 / 255.0;
                    if a > 0.0 {
                        cv.blend(gx + col as i32, gy + row as i32, rgb, a);
                    }
                }
            }
            pen += m.advance_width as f64;
        }
        (pen - x * scale) / scale
    }

    fn text_w(&self, s: &str, size: f64) -> f64 {
        s.chars()
            .map(|c| {
                let (m, _) = self.font.rasterize(c, (size * self.scale) as f32);
                m.advance_width as f64
            })
            .sum::<f64>()
            / self.scale
    }
}

fn mix(a: u32, b: u32, t: f64) -> u32 {
    let f = |sh: u32| ((a >> sh) & 0xFF) as f64 * (1.0 - t) + ((b >> sh) & 0xFF) as f64 * t;
    (f(16) as u32) << 16 | (f(8) as u32) << 8 | f(0) as u32
}
