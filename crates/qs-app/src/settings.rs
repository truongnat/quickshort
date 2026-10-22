//! Preferences window — macOS System Settings language: a sidebar of pages,
//! grouped inset cards, keycap glyphs, sliding switches. Same stack as the
//! overlay: winit window, softbuffer pixels, SDF widgets, fontdue text
//! (system font looked up at runtime, no bundled assets).

use anyhow::{Context, Result};
use fontdue::{Font, FontSettings};
use qs_overlay::sdf::{ellipse_sdf, fill_cov, rrect_sdf, seg_sdf, stroke_cov, Canvas};
use softbuffer::{Context as SbContext, Surface};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Icon, Window, WindowId};

use crate::actions;
use crate::config::{accent_deep, Config, ACCENTS, POPUP_POS};
use crate::tray;

const W: f64 = 820.0;
const H: f64 = 620.0;
const SIDE_W: f64 = 200.0;
/// content column: x origin and width
const CX: f64 = SIDE_W + 24.0;
const CW: f64 = W - CX - 24.0;
const ROW: f64 = 52.0;

// light palette — matches the settings mockup (white surface, #f8f9fb
// sidebar, hairline borders, configurable accent)
const BG_TOP: u32 = 0xFFFFFF;
const BG_BOT: u32 = 0xFAFBFC;
const SIDE_BG: u32 = 0xF8F9FB;
const CARD_TOP: u32 = 0xFFFFFF;
const BORDER: u32 = 0xE1E4E9;
const BORDER_STRONG: u32 = 0xCBD0D8;
const ROW_HOV: u32 = 0xECEEF2;
const TILE: u32 = 0xF0F2F5; // row-icon tile background
const TILE_GLYPH: u32 = 0x555D68;
const KEYCAP: u32 = 0xF3F4F6; // raised key top
const KEYCAP_EDGE: u32 = 0xD4D8DF; // key bottom edge
const FG: u32 = 0x24272D;
const FG_DIM: u32 = 0x535A66;
/// helper/metadata text — 5.0:1 on white; the old #8A919C was 3.2:1
const FG_FAINT: u32 = 0x667085;
const SWITCH_OFF: u32 = 0xE1E4E9;
const DANGER: u32 = 0xB4232D;

/// What the host needs to know after a settings event.
pub enum SettingsEvent {
    /// Config changed + saved — re-register the hotkey, rebuild the menu.
    Changed,
    /// Hotkey recording toggled on/off — host must suspend the global
    /// grab while recording so the chord reaches this window instead of
    /// firing the capture handler.
    HotkeyRecord,
    /// About page wants a release check.
    CheckUpdates,
    /// About page wants to install the pending update.
    InstallUpdate,
    /// User closed the window.
    Closed,
}

#[derive(Clone, Copy, PartialEq)]
enum Glyph {
    Crop,
    Gear,
    Keys,
    Drop,
    Folder,
    Info,
    Bolt,
    Text,
}

/// Sidebar pages in order.
const PAGES: &[(&str, Glyph)] = &[
    ("Capture", Glyph::Crop),
    ("General", Glyph::Gear),
    ("Shortcuts", Glyph::Keys),
    ("Appearance", Glyph::Drop),
    ("OCR", Glyph::Text),
    ("Storage", Glyph::Folder),
    ("About", Glyph::Info),
];

/// Which recordable row is armed. `Global` = the app-wide capture hotkey;
/// the rest are ⌘+key overlay actions.
#[derive(Clone, Copy, PartialEq)]
enum Rec {
    Global,
    /// app-wide fullscreen hotkey — same chord recorder as `Global`
    GlobalFull,
    Save,
    Copy,
    Ocr,
    Share,
    Scroll,
    Record,
    Undo,
}

/// Shortcuts page rows — label + recordable slot.
const KEY_ROWS: &[(&str, Rec)] = &[
    ("Save", Rec::Save),
    ("Copy to clipboard", Rec::Copy),
    ("Extract text (OCR)", Rec::Ocr),
    ("Share link", Rec::Share),
    ("Scrolling capture", Rec::Scroll),
    ("Screen recording", Rec::Record),
    ("Undo / Redo", Rec::Undo),
];

#[derive(Clone, Copy, PartialEq)]
enum Drop {
    RecentN,
    KeepDays,
    PopupPos,
}

#[derive(Clone, Copy, PartialEq)]
enum Sw {
    Login,
    AutoUpd,
    CopySave,
}

impl Sw {
    fn idx(self) -> usize {
        match self {
            Sw::Login => 0,
            Sw::AutoUpd => 1,
            Sw::CopySave => 2,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    Nav(usize),
    Rec(Rec),
    Drop(Drop),
    /// index into the open dropdown's option list
    DropOpt(usize),
    Browse,
    OpenDir,
    Sw(Sw),
    Swatch(usize),
    Recent(usize),
    /// OCR page: auto-detect row
    OcrAuto,
    /// OCR page: index into qs_ocr::LANGS
    Lang(usize),
    /// OCR page: install the missing packs (linux)
    OcrInstall,
    ViewAll,
    /// check for updates, or install when `update_ver` is set
    Update,
    ResetKeys,
    ResetAll,
    Clean,
    Link(usize),
}

type LRect = (f64, f64, f64, f64); // logical x, y, w, h

const LINKS: &[(&str, &str)] = &[
    ("GitHub", "https://github.com/truongnat/quickshort"),
    (
        "Report an issue",
        "https://github.com/truongnat/quickshort/issues",
    ),
    (
        "Request a feature",
        "https://truongnat.github.io/quickshort/#request",
    ),
    ("Support", "https://truongnat.github.io/quickshort/#donate"),
];

const RECENT_N: usize = 4;
const THUMB_W: f64 = 132.0;
const THUMB_H: f64 = 82.0;

pub struct Settings {
    win: Rc<Window>,
    surf: Surface<Rc<Window>, Rc<Window>>,
    font: Font,
    page: usize,
    /// recordable row waiting for input (global chord or ⌘+key)
    recording: Option<Rec>,
    /// open dropdown, if any
    drop_open: Option<Drop>,
    /// destructive action awaiting a second confirming click
    confirm: Option<Hit>,
    hover: Option<Hit>,
    /// keyboard focus target — drawn as a ring only after Tab navigation
    focus: Option<Hit>,
    focus_visible: bool,
    /// outcome of the last "Clean up now" run
    clean_msg: Option<String>,
    /// clickable rects registered during the last draw — hit() scans them
    /// topmost-first, so popups drawn last naturally win
    hits: Vec<(LRect, Hit)>,
    mods: ModifiersState,
    recents: Vec<PathBuf>,
    thumbs: Vec<Option<image::RgbaImage>>,
    /// (count, bytes) of captures in the save dir
    stats: (usize, u64),
    /// (total, free) bytes of the save dir's volume, for the usage chart
    disk: Option<(u64, u64)>,
    /// pending release version the About page can offer to install
    update_ver: Option<String>,
    last_pos: (f64, f64),
    /// switch knob positions 0..1, eased toward the config values
    sw_t: [f64; 3],
    /// recording pulse phase (advanced per frame)
    pulse: f64,
    /// a bare key was pressed while recording — hint a modifier is required
    want_mod: bool,
    /// folder picker is open — guard against re-entry
    dialog_open: bool,
    /// app logo at sidebar size (physical px)
    logo: Option<image::RgbaImage>,
    /// packs the OCR engine can use — feeds the "not installed" badges
    ocr_langs: Vec<String>,
    /// apt install running on a worker thread (pkexec pops the auth UI)
    install: Option<Arc<Mutex<InstallState>>>,
    /// last install's outcome line, shown until the next action
    install_msg: Option<String>,
    dirty: bool,
    closed: bool,
}

/// Shared with the install worker — about_to_wait polls it each frame.
#[derive(Clone)]
enum InstallState {
    Running,
    Ok(String),
    Failed(String),
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
            .with_resizable(false);
        if let Some(img) = &logo {
            if let Ok(ic) = Icon::from_rgba(img.as_raw().clone(), img.width(), img.height()) {
                attrs = attrs.with_window_icon(Some(ic));
            }
        }
        let win = Rc::new(el.create_window(attrs).context("settings window")?);
        force_light_appearance(&win);
        // an accessory app opened from the tray stays deactivated — without
        // activation the window never gets KeyboardInput, so the hotkey
        // recorder looks dead
        refocus(&win);
        let scale = win.scale_factor();
        let logo_px = (64.0 * scale) as u32;
        let logo = logo.map(|i| {
            image::imageops::resize(&i, logo_px, logo_px, image::imageops::FilterType::Lanczos3)
        });
        win.set_cursor(CursorIcon::Default);
        let surf = SbContext::new(win.clone())
            .and_then(|c| Surface::new(&c, win.clone()))
            .map_err(|e| anyhow::anyhow!("settings surface: {e}"))?;
        let recents = actions::recent_captures(&cfg.save_dir(), RECENT_N);
        let thumbs = load_thumbs(&recents, scale);
        Ok(Self {
            win,
            surf,
            font: system_font()?,
            // QS_PAGE=i jumps straight to a page — dev/screenshot harness
            page: std::env::var("QS_PAGE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            recording: None,
            drop_open: None,
            confirm: None,
            hover: None,
            focus: None,
            focus_visible: false,
            clean_msg: None,
            hits: Vec::new(),
            mods: ModifiersState::empty(),
            stats: actions::storage_stats(&cfg.save_dir()),
            disk: actions::disk_stats(&cfg.save_dir()),
            recents,
            thumbs,
            update_ver: None,
            last_pos: (0.0, 0.0),
            sw_t: [
                cfg.launch_at_login as u8 as f64,
                cfg.auto_update as u8 as f64,
                cfg.copy_after_save as u8 as f64,
            ],
            pulse: 0.0,
            want_mod: false,
            dialog_open: false,
            logo,
            ocr_langs: qs_ocr::installed_langs(),
            install: None,
            install_msg: None,
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

    /// Bring the app + window to the front (see `refocus`).
    pub fn focus(&self) {
        refocus(&self.win);
    }

    /// The About page shows "Update to vX" once the host has a pending
    /// release; `None` restores the plain check button.
    pub fn set_update(&mut self, ver: Option<String>) {
        if self.update_ver != ver {
            self.update_ver = ver;
            self.dirty = true;
        }
    }

    /// True while any record button is armed — the host suspends the
    /// global hotkey grab so the chord reaches this window.
    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// True while a pack install is running — the host should wake the
    /// loop periodically so the page polls the worker's result.
    pub fn poll_needed(&self) -> bool {
        self.install.is_some()
    }

    pub fn about_to_wait(&mut self) {
        if let Some(arc) = &self.install {
            let state = arc.lock().unwrap().clone();
            match state {
                InstallState::Running => self.dirty = true,
                InstallState::Ok(m) | InstallState::Failed(m) => {
                    self.install = None;
                    self.install_msg = Some(m);
                    self.ocr_langs = qs_ocr::installed_langs();
                    self.dirty = true;
                }
            }
        }
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
                // pointer use retires the keyboard ring; focus itself stays
                if self.focus_visible {
                    self.focus_visible = false;
                    self.dirty = true;
                }
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
            } => {
                let h = self.hit(self.last_pos);
                self.focus = h;
                self.focus_visible = false;
                return self.click(cfg, h);
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
                    if self.drop_open.take().is_some() {
                        self.dirty = true;
                        return None;
                    }
                    if self.recording.take().is_some() {
                        self.dirty = true;
                        return Some(SettingsEvent::HotkeyRecord);
                    }
                    if self.focus_visible {
                        self.focus_visible = false;
                        self.dirty = true;
                        return None;
                    }
                    self.closed = true;
                    return None;
                }
                // keyboard navigation only while not recording a hotkey
                if self.recording.is_none() {
                    if self.drop_open.is_some() {
                        let dir = match event.logical_key {
                            Key::Named(NamedKey::ArrowDown) => 1i64,
                            Key::Named(NamedKey::ArrowUp) => -1,
                            _ => 0,
                        };
                        if dir != 0 {
                            self.step_option(dir);
                            return None;
                        }
                    }
                    match event.logical_key {
                        Key::Named(NamedKey::Tab) => {
                            self.move_focus(self.mods.shift_key());
                            return None;
                        }
                        Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space)
                            if self.focus_visible =>
                        {
                            if let Some(f) = self.focus {
                                return self.click(cfg, Some(f));
                            }
                        }
                        _ => {}
                    }
                }
                match self.recording {
                    Some(Rec::Global) | Some(Rec::GlobalFull) => {
                        if let Some(hk) = self.chord_string(&event.logical_key) {
                            if hk.parse::<global_hotkey::hotkey::HotKey>().is_err() {
                                return None; // unparsable — keep recording
                            }
                            if self.recording == Some(Rec::GlobalFull) {
                                cfg.hotkey_full = hk;
                            } else {
                                cfg.hotkey = hk;
                            }
                            self.recording = None;
                            self.dirty = true;
                            return Some(SettingsEvent::Changed);
                        }
                        // a real key but no modifier — say why nothing happened
                        if key_token(&event.logical_key).is_some() {
                            self.want_mod = true;
                            self.dirty = true;
                        }
                    }
                    Some(slot) => {
                        if let Key::Character(c) = &event.logical_key {
                            if !(self.mods.super_key() || self.mods.control_key()) {
                                self.want_mod = true;
                                self.dirty = true;
                                return None;
                            }
                            if let Some(ch) = c.chars().next() {
                                let ch = ch.to_ascii_lowercase();
                                if ch.is_ascii_alphanumeric() {
                                    set_key(cfg, slot, ch);
                                    self.recording = None;
                                    self.dirty = true;
                                    return Some(SettingsEvent::Changed);
                                }
                            }
                        } else if key_token(&event.logical_key).is_some() {
                            self.want_mod = true;
                            self.dirty = true;
                        }
                    }
                    None => {}
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
            // a bare key is a bad global hotkey — except PrintScreen, the
            // de-facto screenshot key, which carries no text meaning
            return (token == "PrintScreen").then_some(token);
        }
        Some(format!("{s}{token}"))
    }

    fn click(&mut self, cfg: &mut Config, h: Option<Hit>) -> Option<SettingsEvent> {
        // a click anywhere disarms a pending confirm unless it *is* the
        // confirm target (checked per arm below)
        let armed = self.confirm.take();
        self.confirm = None;
        match h {
            Some(Hit::Nav(i)) => {
                self.page = i;
                self.drop_open = None;
                self.dirty = true;
            }
            Some(Hit::Rec(r)) => {
                self.recording = if self.recording == Some(r) {
                    None
                } else {
                    Some(r)
                };
                self.drop_open = None;
                self.want_mod = false;
                self.dirty = true;
                return Some(SettingsEvent::HotkeyRecord);
            }
            Some(Hit::Drop(d)) => {
                self.drop_open = (self.drop_open != Some(d)).then_some(d);
                self.dirty = true;
            }
            Some(Hit::DropOpt(i)) => {
                if let Some(d) = self.drop_open.take() {
                    drop_apply(cfg, d, i);
                    if d == Drop::KeepDays {
                        self.clean_msg = None;
                    }
                    // the chosen row vanishes with the popup — park focus
                    // back on the button that owns it
                    self.focus = Some(Hit::Drop(d));
                    self.refresh_files(cfg);
                    self.dirty = true;
                    return Some(SettingsEvent::Changed);
                }
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
                        self.refresh_files(cfg);
                        return Some(SettingsEvent::Changed);
                    }
                }
            }
            Some(Hit::OpenDir) | Some(Hit::ViewAll) => {
                actions::open_path(&cfg.save_dir());
            }
            Some(Hit::OcrAuto) => {
                // a checked checkbox must be uncheckable: leaving auto
                // starts a manual set from the first available language
                cfg.ocr_lang = if cfg.ocr_lang.trim() == "auto" {
                    self.ocr_langs
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "eng".into())
                } else {
                    "auto".into()
                };
                self.dirty = true;
                return Some(SettingsEvent::Changed);
            }
            Some(Hit::Lang(i)) => {
                if let Some(e) = qs_ocr::LANGS.get(i) {
                    let mut sel = qs_ocr::lang_codes(&cfg.ocr_lang);
                    if let Some(pos) = sel.iter().position(|c| c == e.0) {
                        sel.remove(pos);
                    } else {
                        sel.push(e.0.to_owned());
                    }
                    // an empty manual set means "scan everything" — auto
                    cfg.ocr_lang = if sel.is_empty() {
                        "auto".into()
                    } else {
                        sel.join("+")
                    };
                    self.dirty = true;
                    return Some(SettingsEvent::Changed);
                }
            }
            Some(Hit::OcrInstall) => {
                if self.install.is_some() {
                    return None;
                }
                let pkgs: Vec<String> = qs_ocr::LANGS
                    .iter()
                    .map(|e| e.0)
                    .filter(|c| !self.ocr_langs.iter().any(|l| l == c))
                    .map(|c| format!("tesseract-ocr-{c}"))
                    .collect();
                if pkgs.is_empty() {
                    return None;
                }
                let st = Arc::new(Mutex::new(InstallState::Running));
                let st2 = st.clone();
                std::thread::spawn(move || {
                    *st2.lock().unwrap() = install_packs(&pkgs);
                });
                self.install = Some(st);
                self.install_msg = None;
                self.dirty = true;
            }
            Some(Hit::Sw(s)) => {
                match s {
                    Sw::Login => {
                        let want = !cfg.launch_at_login;
                        match tray::set_launch_at_login(want) {
                            Ok(()) => cfg.launch_at_login = want,
                            Err(e) => {
                                eprintln!("launch-at-login: {e:#}");
                                return None;
                            }
                        }
                    }
                    Sw::AutoUpd => cfg.auto_update = !cfg.auto_update,
                    Sw::CopySave => cfg.copy_after_save = !cfg.copy_after_save,
                }
                self.dirty = true;
                return Some(SettingsEvent::Changed);
            }
            Some(Hit::Swatch(i)) => {
                if let Some((name, _)) = ACCENTS.get(i) {
                    cfg.accent = (*name).into();
                    self.dirty = true;
                    return Some(SettingsEvent::Changed);
                }
            }
            Some(Hit::Recent(i)) => {
                if let Some(p) = self.recents.get(i) {
                    actions::open_path(p);
                }
            }
            Some(Hit::Update) => {
                return Some(if self.update_ver.is_some() {
                    SettingsEvent::InstallUpdate
                } else {
                    SettingsEvent::CheckUpdates
                });
            }
            Some(Hit::ResetKeys) => {
                if armed == Some(Hit::ResetKeys) {
                    let d = Config::default();
                    cfg.key_save = d.key_save;
                    cfg.key_copy = d.key_copy;
                    cfg.key_ocr = d.key_ocr;
                    cfg.key_share = d.key_share;
                    cfg.key_scroll = d.key_scroll;
                    cfg.key_record = d.key_record;
                    cfg.key_undo = d.key_undo;
                    self.dirty = true;
                    return Some(SettingsEvent::Changed);
                }
                self.confirm = Some(Hit::ResetKeys);
                self.dirty = true;
            }
            Some(Hit::ResetAll) => {
                if armed == Some(Hit::ResetAll) {
                    let keep = (cfg.save_dir.clone(), cfg.last_update_check);
                    *cfg = Config::default();
                    cfg.save_dir = keep.0;
                    cfg.last_update_check = keep.1;
                    self.refresh_files(cfg);
                    self.dirty = true;
                    return Some(SettingsEvent::Changed);
                }
                self.confirm = Some(Hit::ResetAll);
                self.dirty = true;
            }
            Some(Hit::Clean) => {
                if cfg.keep_days == 0 {
                    self.clean_msg = Some("Retention is off — choose a period first.".into());
                    self.dirty = true;
                    return None;
                }
                if armed == Some(Hit::Clean) {
                    let n = actions::cleanup_captures(&cfg.save_dir(), cfg.keep_days);
                    self.clean_msg = Some(match n {
                        0 => "Nothing was old enough to remove.".into(),
                        1 => "Removed 1 old capture.".into(),
                        n => format!("Removed {n} old captures."),
                    });
                    self.refresh_files(cfg);
                    self.dirty = true;
                    return Some(SettingsEvent::Changed);
                }
                self.confirm = Some(Hit::Clean);
                self.dirty = true;
            }
            Some(Hit::Link(i)) => {
                if i == 1 {
                    // Report an issue — bundle diagnostics into the body
                    actions::report_issue(cfg);
                } else if let Some((_, url)) = LINKS.get(i) {
                    actions::open_url(url);
                }
            }
            None => {
                self.drop_open = None;
                if self.recording.take().is_some() {
                    self.dirty = true;
                    return Some(SettingsEvent::HotkeyRecord);
                }
                self.dirty = true;
            }
        }
        None
    }

    /// Re-read captures + stats after the save dir or retention changed.
    fn refresh_files(&mut self, cfg: &Config) {
        self.recents = actions::recent_captures(&cfg.save_dir(), RECENT_N);
        self.thumbs = load_thumbs(&self.recents, self.win.scale_factor());
        self.stats = actions::storage_stats(&cfg.save_dir());
        self.disk = actions::disk_stats(&cfg.save_dir());
    }

    fn hit(&self, p: (f64, f64)) -> Option<Hit> {
        let (x, y) = p;
        self.hits
            .iter()
            .rev()
            .find(|(r, _)| x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3)
            .map(|&(_, h)| h)
    }

    /// Tab order = registration order from the last draw (sidebar, then
    /// page controls). An open dropdown is modal, so only its rows cycle.
    fn focusables(&self) -> Vec<Hit> {
        let mut v = Vec::new();
        for &(_, h) in &self.hits {
            if self.drop_open.is_some() && !matches!(h, Hit::DropOpt(_)) {
                continue;
            }
            if !v.contains(&h) {
                v.push(h);
            }
        }
        v
    }

    fn move_focus(&mut self, back: bool) {
        let list = self.focusables();
        if list.is_empty() {
            return;
        }
        let cur = self.focus.and_then(|f| list.iter().position(|h| *h == f));
        let next = match cur {
            Some(i) if back => (i + list.len() - 1) % list.len(),
            Some(i) => (i + 1) % list.len(),
            None if back => list.len() - 1,
            None => 0,
        };
        self.focus = Some(list[next]);
        self.focus_visible = true;
        self.dirty = true;
    }

    /// Arrow keys inside an open dropdown walk its rows.
    fn step_option(&mut self, dir: i64) {
        let opts: Vec<Hit> = self
            .hits
            .iter()
            .filter_map(|&(_, h)| matches!(h, Hit::DropOpt(_)).then_some(h))
            .collect();
        if opts.is_empty() {
            return;
        }
        let cur = self.focus.and_then(|f| opts.iter().position(|h| *h == f));
        let i = match cur {
            Some(i) => (i as i64 + dir).rem_euclid(opts.len() as i64) as usize,
            None if dir < 0 => opts.len() - 1,
            None => 0,
        };
        self.focus = Some(opts[i]);
        self.focus_visible = true;
        self.dirty = true;
    }

    fn draw(&mut self, cfg: &Config) {
        // advance animations; keep repainting while anything is moving
        let targets = [
            cfg.launch_at_login as u8 as f64,
            cfg.auto_update as u8 as f64,
            cfg.copy_after_save as u8 as f64,
        ];
        let mut animating = self.recording.is_some();
        for (t, &want) in self.sw_t.iter_mut().zip(&targets) {
            if (*t - want).abs() > 0.001 {
                animating = true;
            }
            *t += (want - *t) * 0.28;
        }
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
        // snapshot state — buffer_mut holds &mut self.surf for the rest
        // of the fn, so only disjoint field borrows are allowed below
        let v = View {
            cfg,
            page: self.page,
            hover: self.hover,
            recording: self.recording,
            pulse: self.pulse,
            want_mod: self.want_mod,
            drop_open: self.drop_open,
            confirm: self.confirm,
            sw_t: self.sw_t,
            stats: self.stats,
            disk: self.disk,
            update_ver: self.update_ver.as_deref(),
            recents: &self.recents,
            thumbs: &self.thumbs,
            logo: self.logo.as_ref(),
            ocr_langs: &self.ocr_langs,
            installing: self.install.is_some(),
            install_msg: self.install_msg.as_deref(),
            clean_msg: self.clean_msg.as_deref(),
        };
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
            accent: cfg.accent_rgb(),
            accent_deep: accent_deep(&cfg.accent),
        };
        let mut hits = std::mem::take(&mut self.hits);
        hits.clear();

        // sidebar: darker panel + nav, separator hairline
        p.fill(&mut cv, (0.0, 0.0, SIDE_W, H), 0.0, SIDE_BG, 1.0);
        for py in 0..sh {
            cv.blend((SIDE_W * scale) as i32, py as i32, BORDER, 1.0);
        }
        if let Some(logo) = v.logo {
            let (ox, oy) = ((18.0 * scale) as i32, (22.0 * scale) as i32);
            for (px, py, c) in logo.enumerate_pixels() {
                let a = c[3] as f64 / 255.0;
                if a > 0.0 {
                    let rgb = ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32;
                    cv.blend(ox + px as i32, oy + py as i32, rgb, a);
                }
            }
        } else {
            p.icon(&mut cv, 18.0, 22.0, 64.0);
        }
        p.text(&mut cv, 18.0, 106.0, 17.0, "Quickshort", FG);
        p.text(
            &mut cv,
            18.0,
            124.0,
            11.0,
            concat!("v", env!("CARGO_PKG_VERSION")),
            FG_FAINT,
        );
        let mut ny = 152.0;
        for (i, (name, g)) in PAGES.iter().enumerate() {
            let r = (10.0, ny, SIDE_W - 20.0, 36.0);
            let sel = v.page == i;
            if sel {
                p.fill(&mut cv, r, 9.0, p.accent_deep, 1.0);
            } else if v.hover == Some(Hit::Nav(i)) {
                p.fill(&mut cv, r, 9.0, ROW_HOV, 1.0);
            }
            p.glyph(
                &mut cv,
                *g,
                r.0 + 9.0,
                r.1 + 10.0,
                16.0,
                if sel { 0xFFFFFF } else { FG_DIM },
            );
            p.text(
                &mut cv,
                r.0 + 34.0,
                r.1 + 24.0,
                13.0,
                name,
                if sel { 0xFFFFFF } else { FG },
            );
            hits.push((r, Hit::Nav(i)));
            ny += 40.0;
        }
        p.text(
            &mut cv,
            18.0,
            H - 18.0,
            10.5,
            "Preferences save automatically",
            FG_FAINT,
        );

        match v.page {
            0 => draw_capture(&p, &mut cv, &v, &mut hits),
            1 => draw_general(&p, &mut cv, &v, &mut hits),
            2 => draw_shortcuts(&p, &mut cv, &v, &mut hits),
            3 => draw_appearance(&p, &mut cv, &v, &mut hits),
            4 => draw_ocr(&p, &mut cv, &v, &mut hits),
            5 => draw_storage(&p, &mut cv, &v, &mut hits),
            _ => draw_about(&p, &mut cv, &v, &mut hits),
        }

        // open dropdown popup floats above the page — drawn last so its
        // hit rects win the topmost scan. Anchored to the button rect the
        // page registered in `hits`, so it can't drift from the control.
        if let Some(d) = v.drop_open {
            let (opts, cur) = drop_state(v.cfg, d);
            if let Some(&btn) = hits
                .iter()
                .find(|(_, h)| *h == Hit::Drop(d))
                .map(|(r, _)| r)
            {
                let ph = opts.len() as f64 * 30.0 + 8.0;
                // flip upward when the list would spill past the window edge
                let py = if btn.1 + btn.3 + 4.0 + ph > H - 8.0 {
                    (btn.1 - 4.0 - ph).max(8.0)
                } else {
                    btn.1 + btn.3 + 4.0
                };
                let pop = (btn.0, py, btn.2.max(160.0), ph);
                p.card(&mut cv, pop);
                for (i, o) in opts.iter().enumerate() {
                    let ir = (
                        pop.0 + 4.0,
                        pop.1 + 4.0 + i as f64 * 30.0,
                        pop.2 - 8.0,
                        30.0,
                    );
                    if v.hover == Some(Hit::DropOpt(i)) {
                        p.fill(&mut cv, ir, 6.0, ROW_HOV, 1.0);
                    }
                    if i == cur {
                        p.text(&mut cv, ir.0 + 10.0, ir.1 + 20.0, 12.0, "✓", p.accent_deep);
                        p.text(&mut cv, ir.0 + 26.0, ir.1 + 20.0, 12.0, o, FG);
                    } else {
                        p.text(&mut cv, ir.0 + 26.0, ir.1 + 20.0, 12.0, o, FG);
                    }
                    hits.push((ir, Hit::DropOpt(i)));
                }
            }
        }

        // keyboard focus ring — white halo + accent keeps it legible on
        // both white cards and the accent-filled nav pill
        if self.focus_visible {
            if let Some(f) = self.focus {
                if let Some(&(r, _)) = hits.iter().find(|(_, h)| *h == f) {
                    p.stroke(
                        &mut cv,
                        (r.0 - 3.0, r.1 - 3.0, r.2 + 6.0, r.3 + 6.0),
                        10.0,
                        0xFFFFFF,
                        1.0,
                        3.0,
                    );
                    p.stroke(
                        &mut cv,
                        (r.0 - 1.0, r.1 - 1.0, r.2 + 2.0, r.3 + 2.0),
                        9.0,
                        p.accent,
                        1.0,
                        1.5,
                    );
                }
            }
        }

        self.hits = hits;
        // debug: QS_SHOT=<png path> dumps the first rendered frame
        if let Ok(path) = std::env::var("QS_SHOT") {
            std::env::remove_var("QS_SHOT");
            let mut rgba = Vec::with_capacity(sw * sh * 4);
            for &px in buf.iter() {
                rgba.extend_from_slice(&[
                    ((px >> 16) & 0xFF) as u8,
                    ((px >> 8) & 0xFF) as u8,
                    (px & 0xFF) as u8,
                    255,
                ]);
            }
            let _ = image::save_buffer(&path, &rgba, sw as u32, sh as u32, image::ColorType::Rgba8);
            if std::env::var("QS_EXIT").is_ok() {
                std::process::exit(0);
            }
        }
        let _ = buf.present();
    }
}

/// Immutable snapshot of everything the page renderers need — `draw`
/// builds this before `buffer_mut` so page fns stay free functions and
/// never touch `&mut self` while the surface is borrowed.
struct View<'a> {
    cfg: &'a Config,
    page: usize,
    hover: Option<Hit>,
    recording: Option<Rec>,
    pulse: f64,
    want_mod: bool,
    drop_open: Option<Drop>,
    confirm: Option<Hit>,
    sw_t: [f64; 3],
    stats: (usize, u64),
    disk: Option<(u64, u64)>,
    update_ver: Option<&'a str>,
    recents: &'a [PathBuf],
    thumbs: &'a [Option<image::RgbaImage>],
    logo: Option<&'a image::RgbaImage>,
    ocr_langs: &'a [String],
    /// a pack install is running — the Install button locks + pulses
    installing: bool,
    install_msg: Option<&'a str>,
    /// outcome of the last manual clean-up
    clean_msg: Option<&'a str>,
}

// layout helpers ------------------------------------------------------------

fn card(x: f64, y: f64, rows: usize) -> LRect {
    (x, y, CW, ROW * rows.max(1) as f64)
}

/// Right-aligned control slot inside a card row.
fn slot(y: f64, w: f64) -> LRect {
    (CX + CW - 14.0 - w, y + (ROW - 30.0) / 2.0, w, 30.0)
}

/// dropdown option list + current index for `d`
fn drop_state(cfg: &Config, d: Drop) -> (Vec<&'static str>, usize) {
    match d {
        Drop::RecentN => (
            vec!["4 items", "8 items", "12 items"],
            match cfg.recent_max {
                4 => 0,
                12 => 2,
                _ => 1,
            },
        ),
        Drop::KeepDays => (
            vec!["Forever", "7 days", "30 days"],
            match cfg.keep_days {
                7 => 1,
                30 => 2,
                _ => 0,
            },
        ),
        Drop::PopupPos => (
            POPUP_POS.iter().map(|(_, l)| *l).collect(),
            POPUP_POS
                .iter()
                .position(|(k, _)| *k == cfg.popup_pos)
                .unwrap_or(0),
        ),
    }
}

fn section_title(p: &Painter, cv: &mut Canvas, y: f64, title: &str, sub: &str) {
    p.text(cv, CX, y, 13.5, title, FG);
    p.text(cv, CX, y + 19.0, 11.5, sub, FG_DIM);
}

/// Card row: icon tile + label (+ dim description) on the left.
fn row_label(p: &Painter, cv: &mut Canvas, r: LRect, g: Glyph, label: &str, desc: &str) {
    let tile = (r.0 + 12.0, r.1 + (r.3 - 36.0) / 2.0, 36.0, 36.0);
    p.fill(cv, tile, 9.0, TILE, 1.0);
    p.glyph(cv, g, tile.0 + 9.0, tile.1 + 9.0, 18.0, TILE_GLYPH);
    p.text(cv, r.0 + 60.0, r.1 + 21.0, 13.0, label, FG);
    if !desc.is_empty() {
        p.text(cv, r.0 + 60.0, r.1 + 39.0, 10.5, desc, FG_FAINT);
    }
}

fn dropdown_btn(p: &Painter, cv: &mut Canvas, r: LRect, label: &str, open: bool, hover: bool) {
    p.fill(
        cv,
        r,
        7.0,
        if open || hover { ROW_HOV } else { 0xFFFFFF },
        1.0,
    );
    p.stroke(cv, r, 7.0, BORDER_STRONG, 1.0, 0.75);
    p.text(cv, r.0 + 10.0, r.1 + 20.0, 12.0, label, FG);
    // chevron ⌄
    let (mx, my) = (r.0 + r.2 - 16.0, r.1 + r.3 / 2.0 - 2.0);
    p.seg(cv, mx, my, mx + 4.0, my + 5.0, 1.6, FG_DIM);
    p.seg(cv, mx + 8.0, my, mx + 4.0, my + 5.0, 1.6, FG_DIM);
}

fn ghost(p: &Painter, cv: &mut Canvas, r: LRect, label: &str, hover: bool, confirm: bool) -> LRect {
    if confirm {
        p.fill(cv, r, r.3 / 2.0, DANGER, 1.0);
        p.text(
            cv,
            r.0 + (r.2 - p.text_w("Confirm?", 11.5)) / 2.0,
            r.1 + r.3 / 2.0 + 4.0,
            11.5,
            "Confirm?",
            0xFFFFFF,
        );
    } else {
        p.ghost_button(cv, r, label, hover);
    }
    r
}

fn switch_row(
    p: &Painter,
    cv: &mut Canvas,
    v: &View,
    hits: &mut Vec<(LRect, Hit)>,
    r: LRect,
    g: Glyph,
    label: &str,
    desc: &str,
    sw: Sw,
) {
    if v.hover == Some(Hit::Sw(sw)) {
        p.fill(cv, r, 7.0, ROW_HOV, 1.0);
    }
    row_label(p, cv, r, g, label, desc);
    p.switch(
        cv,
        (r.0 + r.2 - 14.0 - 44.0, r.1 + 13.0, 44.0, 26.0),
        v.sw_t[sw.idx()],
    );
    hits.push((r, Hit::Sw(sw)));
}

// pages ---------------------------------------------------------------------

fn draw_capture(p: &Painter, cv: &mut Canvas, v: &View, hits: &mut Vec<(LRect, Hit)>) {
    p.text(cv, CX, 52.0, 24.0, "Capture", FG);
    p.text(
        cv,
        CX,
        76.0,
        12.5,
        "Configure how you capture and save screenshots.",
        FG_DIM,
    );
    section_title(
        p,
        cv,
        112.0,
        "Capture method",
        "Choose hotkey and capture behavior.",
    );
    let c = card(CX, 146.0, 2);
    p.card(cv, c);
    // row 1: region hotkey
    let r = (c.0, c.1, c.2, ROW);
    row_label(
        p,
        cv,
        r,
        Glyph::Keys,
        "Capture hotkey",
        "Select a region on screen",
    );
    let rb = slot(r.1, 76.0);
    if v.recording == Some(Rec::Global) {
        let a = 0.55 + 0.45 * v.pulse.sin().abs();
        p.stroke(
            cv,
            (c.0 + 8.0, r.1 + 11.0, c.2 - 16.0, 30.0),
            7.0,
            v.cfg.accent_rgb(),
            a,
            1.0,
        );
        let (msg, col) = if v.want_mod {
            ("Needs a modifier, e.g. ⌘ ⌥ ⌃", FG_DIM)
        } else {
            ("Press shortcut", p.accent_deep)
        };
        p.text(
            cv,
            rb.0 - p.text_w(msg, 11.5) - 12.0,
            r.1 + 29.0,
            11.5,
            msg,
            col,
        );
    } else {
        p.keycaps(
            cv,
            &v.cfg.hotkey,
            rb.0 - keycaps_w(p, &v.cfg.hotkey) - 10.0,
            r.1 + 12.0,
        );
    }
    p.ghost_button(cv, rb, "Change", v.hover == Some(Hit::Rec(Rec::Global)));
    hits.push((rb, Hit::Rec(Rec::Global)));

    // row 2: fullscreen hotkey — saves the whole screen, no selection
    let r = (c.0, c.1 + ROW, c.2, ROW);
    row_label(
        p,
        cv,
        r,
        Glyph::Keys,
        "Fullscreen hotkey",
        "Save the whole screen, no selection",
    );
    let rb = slot(r.1, 76.0);
    if v.recording == Some(Rec::GlobalFull) {
        let a = 0.55 + 0.45 * v.pulse.sin().abs();
        p.stroke(
            cv,
            (c.0 + 8.0, r.1 + 11.0, c.2 - 16.0, 30.0),
            7.0,
            v.cfg.accent_rgb(),
            a,
            1.0,
        );
        let (msg, col) = if v.want_mod {
            ("Needs a modifier, e.g. ⌘ ⌥ ⌃", FG_DIM)
        } else {
            ("Press shortcut", p.accent_deep)
        };
        p.text(
            cv,
            rb.0 - p.text_w(msg, 11.5) - 12.0,
            r.1 + 29.0,
            11.5,
            msg,
            col,
        );
    } else {
        p.keycaps(
            cv,
            &v.cfg.hotkey_full,
            rb.0 - keycaps_w(p, &v.cfg.hotkey_full) - 10.0,
            r.1 + 12.0,
        );
    }
    p.ghost_button(cv, rb, "Change", v.hover == Some(Hit::Rec(Rec::GlobalFull)));
    hits.push((rb, Hit::Rec(Rec::GlobalFull)));

    section_title(
        p,
        cv,
        268.0,
        "Save location",
        "Choose where captured images are saved.",
    );
    let c = card(CX, 302.0, 1);
    p.card(cv, c);
    row_label(
        p,
        cv,
        c,
        Glyph::Folder,
        "Save folder",
        &tail(&short_path(&v.cfg.save_dir().display().to_string()), 38),
    );
    let bb = slot(c.1, 76.0);
    p.ghost_button(cv, bb, "Change…", v.hover == Some(Hit::Browse));
    hits.push((bb, Hit::Browse));

    section_title(
        p,
        cv,
        368.0,
        "Options",
        "Additional settings for capture and workflow.",
    );
    let c = card(CX, 402.0, 1);
    p.card(cv, c);
    switch_row(
        p,
        cv,
        v,
        hits,
        c,
        Glyph::Bolt,
        "Launch at login",
        "Start Quickshort when you log in",
        Sw::Login,
    );

    // recent captures
    p.text(cv, CX, 482.0, 14.0, "Recent captures", FG);
    let va = (CX + CW - 84.0, 470.0, 84.0, 24.0);
    p.text(
        cv,
        va.0 + (va.2 - p.text_w("Open folder ›", 11.5)) / 2.0,
        483.0,
        11.5,
        "Open folder ›",
        if v.hover == Some(Hit::ViewAll) {
            FG
        } else {
            p.accent_deep
        },
    );
    hits.push((va, Hit::ViewAll));
    if v.recents.is_empty() {
        p.text(
            cv,
            CX,
            540.0,
            12.0,
            "No captures yet — your latest shots will show up here.",
            FG_DIM,
        );
    }
    for (i, path) in v.recents.iter().enumerate() {
        let x = CX + i as f64 * (THUMB_W + 14.0);
        let tr = (x, 500.0, THUMB_W, THUMB_H);
        if v.hover == Some(Hit::Recent(i)) {
            p.fill(
                cv,
                (tr.0 - 3.0, tr.1 - 3.0, tr.2 + 6.0, tr.3 + 6.0),
                8.0,
                ROW_HOV,
                1.0,
            );
        }
        p.thumb(cv, tr, v.thumbs.get(i).and_then(|t| t.as_ref()));
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        p.text(cv, x, tr.1 + tr.3 + 14.0, 10.5, &tail(&name, 20), FG_DIM);
        p.text(cv, x, tr.1 + tr.3 + 28.0, 10.0, &age_label(path), FG_FAINT);
        hits.push((
            (tr.0 - 3.0, tr.1 - 3.0, tr.2 + 6.0, tr.3 + 34.0),
            Hit::Recent(i),
        ));
    }
}

fn draw_general(p: &Painter, cv: &mut Canvas, v: &View, hits: &mut Vec<(LRect, Hit)>) {
    p.text(cv, CX, 52.0, 24.0, "General", FG);
    p.text(
        cv,
        CX,
        76.0,
        12.5,
        "App behavior and system integration.",
        FG_DIM,
    );
    section_title(p, cv, 112.0, "Updates", "Keep Quickshort current.");
    let c = card(CX, 146.0, 1);
    p.card(cv, c);
    switch_row(
        p,
        cv,
        v,
        hits,
        c,
        Glyph::Info,
        "Automatically check for updates",
        "Once a day, in the background",
        Sw::AutoUpd,
    );
    section_title(p, cv, 228.0, "Menu", "What shows in the menu bar icon.");
    let c = card(CX, 262.0, 1);
    p.card(cv, c);
    row_label(
        p,
        cv,
        c,
        Glyph::Folder,
        "Recent items",
        "Captures listed in the tray menu",
    );
    let mb = (c.0 + c.2 - 14.0 - 130.0, c.1 + 11.0, 130.0, 30.0);
    dropdown_btn(
        p,
        cv,
        mb,
        &format!("{} items", v.cfg.recent_max),
        v.drop_open == Some(Drop::RecentN),
        v.hover == Some(Hit::Drop(Drop::RecentN)),
    );
    hits.push((mb, Hit::Drop(Drop::RecentN)));

    section_title(p, cv, 344.0, "Reset", "Restore every setting to defaults.");
    let c = card(CX, 378.0, 1);
    p.card(cv, c);
    row_label(
        p,
        cv,
        c,
        Glyph::Gear,
        "Reset all settings",
        "Hotkey, shortcuts, accent, folders — all of it",
    );
    let rb = slot(c.1, 76.0);
    ghost(
        p,
        cv,
        rb,
        "Reset…",
        v.hover == Some(Hit::ResetAll),
        v.confirm == Some(Hit::ResetAll),
    );
    hits.push((rb, Hit::ResetAll));
}

fn draw_shortcuts(p: &Painter, cv: &mut Canvas, v: &View, hits: &mut Vec<(LRect, Hit)>) {
    p.text(cv, CX, 52.0, 24.0, "Shortcuts", FG);
    p.text(
        cv,
        CX,
        76.0,
        12.5,
        "Overlay action keys — held with ⌘ or ⌃ while annotating.",
        FG_DIM,
    );
    let c = card(CX, 104.0, KEY_ROWS.len());
    p.card(cv, c);
    for (i, (label, rec)) in KEY_ROWS.iter().enumerate() {
        let r = (c.0, c.1 + i as f64 * ROW, c.2, ROW);
        if i > 0 {
            p.line(cv, c.0 + 14.0, r.1, c.0 + c.2 - 14.0);
        }
        row_label(p, cv, r, Glyph::Keys, label, "");
        let rb = slot(r.1, 76.0);
        if v.recording == Some(*rec) {
            let a = 0.55 + 0.45 * v.pulse.sin().abs();
            p.stroke(
                cv,
                (c.0 + 8.0, r.1 + 11.0, c.2 - 16.0, 30.0),
                7.0,
                v.cfg.accent_rgb(),
                a,
                1.0,
            );
            let (msg, col) = if v.want_mod {
                ("Hold ⌘ or ⌃ plus a key", FG_DIM)
            } else {
                ("Press ⌘ + key", p.accent_deep)
            };
            p.text(
                cv,
                rb.0 - p.text_w(msg, 11.5) - 12.0,
                r.1 + 29.0,
                11.5,
                msg,
                col,
            );
        } else {
            let ks = format!("⌘+{}", key_str(v.cfg, *rec).to_uppercase());
            p.keycaps(cv, &ks, rb.0 - keycaps_w(p, &ks) - 10.0, r.1 + 12.0);
        }
        p.ghost_button(cv, rb, "Change", v.hover == Some(Hit::Rec(*rec)));
        hits.push((rb, Hit::Rec(*rec)));
    }
    p.text(
        cv,
        CX,
        c.1 + c.3 + 24.0,
        11.0,
        "The Undo key also redoes with ⇧. Fixed keys: Esc cancel, Enter commit.",
        FG_FAINT,
    );
    let rb = (CX + CW - 130.0, c.1 + c.3 + 44.0, 130.0, 30.0);
    ghost(
        p,
        cv,
        rb,
        "Reset to defaults",
        v.hover == Some(Hit::ResetKeys),
        v.confirm == Some(Hit::ResetKeys),
    );
    hits.push((rb, Hit::ResetKeys));
}

fn draw_appearance(p: &Painter, cv: &mut Canvas, v: &View, hits: &mut Vec<(LRect, Hit)>) {
    p.text(cv, CX, 52.0, 24.0, "Appearance", FG);
    p.text(cv, CX, 76.0, 12.5, "Interface and behavior.", FG_DIM);
    section_title(p, cv, 112.0, "Accent", "Selection and highlight color.");
    let c = card(CX, 146.0, 1);
    p.card(cv, c);
    row_label(
        p,
        cv,
        c,
        Glyph::Drop,
        "Accent color",
        "Used for selections and highlights",
    );
    for (i, (name, rgb)) in ACCENTS.iter().enumerate() {
        let r = (
            c.0 + c.2 - 14.0 - (ACCENTS.len() - i) as f64 * 34.0,
            c.1 + 15.0,
            26.0,
            22.0,
        );
        p.swatch(
            cv,
            r,
            *rgb,
            v.cfg.accent == *name,
            v.hover == Some(Hit::Swatch(i)),
        );
        hits.push((r, Hit::Swatch(i)));
    }
    section_title(p, cv, 228.0, "Clipboard", "What happens after a save.");
    let c = card(CX, 262.0, 1);
    p.card(cv, c);
    switch_row(
        p,
        cv,
        v,
        hits,
        c,
        Glyph::Drop,
        "Copy captures after saving",
        "Saved screenshots also go to the clipboard",
        Sw::CopySave,
    );

    section_title(p, cv, 344.0, "Popups", "Where floating cards appear.");
    let c = card(CX, 378.0, 1);
    p.card(cv, c);
    row_label(
        p,
        cv,
        c,
        Glyph::Drop,
        "Popup position",
        "Toasts, capture cards and the REC pill",
    );
    let pb = (c.0 + c.2 - 14.0 - 140.0, c.1 + 11.0, 140.0, 30.0);
    let label = POPUP_POS
        .iter()
        .find(|(k, _)| *k == v.cfg.popup_pos)
        .map(|(_, l)| *l)
        .unwrap_or(POPUP_POS[0].1);
    dropdown_btn(
        p,
        cv,
        pb,
        label,
        v.drop_open == Some(Drop::PopupPos),
        v.hover == Some(Hit::Drop(Drop::PopupPos)),
    );
    hits.push((pb, Hit::Drop(Drop::PopupPos)));
}

/// Square checkbox + tick — a checklist control where a switch would
/// imply exclusivity.
fn check_box(p: &Painter, cv: &mut Canvas, x: f64, y: f64, on: bool, hover: bool) {
    if on {
        p.fill(cv, (x, y, 20.0, 20.0), 5.0, p.accent_deep, 1.0);
        p.seg(cv, x + 5.5, y + 10.5, x + 9.0, y + 14.0, 2.0, 0xFFFFFF);
        p.seg(cv, x + 9.0, y + 14.0, x + 15.0, y + 6.5, 2.0, 0xFFFFFF);
    } else {
        p.fill(cv, (x, y, 20.0, 20.0), 5.0, 0xFFFFFF, 1.0);
        p.stroke(
            cv,
            (x, y, 20.0, 20.0),
            5.0,
            if hover { BORDER_STRONG } else { BORDER },
            1.0,
            1.5,
        );
    }
}

/// apt install of missing packs — pkexec raises the polkit auth dialog
/// so the app never needs root itself. Worker thread reports via the
/// shared InstallState that about_to_wait polls.
#[cfg(target_os = "linux")]
fn install_packs(pkgs: &[String]) -> InstallState {
    let status = std::process::Command::new("pkexec")
        .arg("apt-get")
        .arg("install")
        .arg("-y")
        .args(pkgs)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => InstallState::Ok(format!("Installed {} pack(s)", pkgs.len())),
        Ok(s) => InstallState::Failed(format!("apt exited {s} — nothing installed")),
        Err(e) => InstallState::Failed(format!("pkexec failed: {e}")),
    }
}

#[cfg(not(target_os = "linux"))]
fn install_packs(_: &[String]) -> InstallState {
    // unreachable — the button only renders where packs can be installed
    InstallState::Failed("pack install is Linux-only".into())
}

fn draw_ocr(p: &Painter, cv: &mut Canvas, v: &View, hits: &mut Vec<(LRect, Hit)>) {
    p.text(cv, CX, 52.0, 24.0, "OCR", FG);
    p.text(
        cv,
        CX,
        76.0,
        12.5,
        "Languages the text action (T) recognizes.",
        FG_DIM,
    );

    let auto = v.cfg.ocr_lang.trim() == "auto";
    section_title(
        p,
        cv,
        112.0,
        "Mode",
        if auto {
            "Every installed script is scanned — choose a language below to limit it."
        } else {
            "Only checked languages run — uncheck them all to return to auto."
        },
    );
    let c = card(CX, 146.0, 1);
    p.card(cv, c);
    let r = (c.0, c.1, c.2, ROW);
    row_label(
        p,
        cv,
        r,
        Glyph::Text,
        "Auto-detect",
        "Let the engine identify the script — recommended",
    );
    check_box(
        p,
        cv,
        r.0 + r.2 - 34.0,
        r.1 + 16.0,
        auto,
        v.hover == Some(Hit::OcrAuto),
    );
    hits.push((r, Hit::OcrAuto));

    // Media.Ocr is single-language; tesseract and Vision take lists
    let sel_note = if auto {
        "Auto-detect is on — ticking a language switches to manual."
    } else if cfg!(target_os = "windows") {
        "The first checked language wins — Media.Ocr takes one."
    } else {
        "All checked languages run together."
    };
    section_title(p, cv, 214.0, "Languages", sel_note);
    // two-column checklist, 40px rows
    const LROW: f64 = 40.0;
    let rows = qs_ocr::LANGS.len().div_ceil(2);
    let gy = 248.0;
    let gh = rows as f64 * LROW + 12.0;
    p.card(cv, (CX, gy, CW, gh));
    let col_w = (CW - 16.0) / 2.0;
    let sel = qs_ocr::lang_codes(&v.cfg.ocr_lang);
    for (i, e) in qs_ocr::LANGS.iter().enumerate() {
        let col = i / rows;
        let row = i % rows;
        let x = CX + 8.0 + col as f64 * col_w;
        let y = gy + 6.0 + row as f64 * LROW;
        let hr = (x, y, col_w, LROW);
        let on = sel.iter().any(|s| s == e.0);
        // a language the engine can't use isn't a control — except an
        // already-checked one, which must stay uncheckable
        let missing = !v.ocr_langs.iter().any(|l| l == e.0);
        let live = !missing || on;
        let hover = live && v.hover == Some(Hit::Lang(i));
        if hover {
            p.fill(cv, hr, 6.0, ROW_HOV, 1.0);
        }
        if missing && !on {
            p.fill(cv, (x + 6.0, y + 10.0, 20.0, 20.0), 5.0, TILE, 0.8);
            p.stroke(cv, (x + 6.0, y + 10.0, 20.0, 20.0), 5.0, BORDER, 0.8, 1.0);
        } else {
            check_box(p, cv, x + 6.0, y + 10.0, on, hover);
        }
        p.text(
            cv,
            x + 34.0,
            y + 25.0,
            13.0,
            e.1,
            if missing { FG_FAINT } else { FG },
        );
        if missing {
            let note = "not installed";
            p.text(
                cv,
                x + col_w - 16.0 - p.text_w(note, 10.5),
                y + 25.0,
                10.5,
                note,
                FG_FAINT,
            );
        } else {
            // code sits beside the name so the two read as one option
            p.text(
                cv,
                x + 34.0 + p.text_w(e.1, 13.0) + 8.0,
                y + 25.0,
                10.5,
                e.0,
                FG_FAINT,
            );
        }
        if live {
            hits.push((hr, Hit::Lang(i)));
        }
    }

    // footer: install action / progress / outcome / per-OS hint
    let fy = gy + gh + 22.0;
    #[cfg(target_os = "linux")]
    {
        if v.installing {
            // pulses while the worker runs — the polkit auth dialog is
            // the user's cue that the click did something
            let a = 0.35 + 0.65 * v.pulse.sin().abs();
            p.text(
                cv,
                CX + 8.0,
                fy + 4.0,
                11.5,
                "Installing language packs… approve the system prompt",
                mix(FG_FAINT, FG_DIM, a),
            );
        } else {
            let missing: Vec<&str> = qs_ocr::LANGS
                .iter()
                .map(|e| e.0)
                .filter(|c| !v.ocr_langs.iter().any(|l| l == *c))
                .collect();
            if !missing.is_empty() {
                let lb = (CX, fy - 6.0, 190.0, 30.0);
                p.ghost_button(
                    cv,
                    lb,
                    &format!("Install {} missing packs", missing.len()),
                    v.hover == Some(Hit::OcrInstall),
                );
                hits.push((lb, Hit::OcrInstall));
                p.text(
                    cv,
                    lb.0 + lb.2 + 14.0,
                    fy + 14.0,
                    11.0,
                    "or: sudo apt install tesseract-ocr-<code>",
                    FG_FAINT,
                );
            }
            // outcome sits under the button row so the two never overlap
            let my = if missing.is_empty() {
                fy + 4.0
            } else {
                fy + 42.0
            };
            if let Some(m) = v.install_msg {
                p.text(cv, CX + 8.0, my, 11.5, m, FG_DIM);
            } else if missing.is_empty() {
                p.text(
                    cv,
                    CX + 8.0,
                    my,
                    11.5,
                    "All listed languages are installed.",
                    FG_FAINT,
                );
            }
        }
    }
    #[cfg(target_os = "macos")]
    p.text(
        cv,
        CX + 8.0,
        fy + 4.0,
        11.5,
        "Recognition runs through macOS Vision — dimmed entries are unsupported.",
        FG_FAINT,
    );
    #[cfg(target_os = "windows")]
    p.text(
        cv,
        CX + 8.0,
        fy + 4.0,
        11.5,
        "Missing packs: Windows Settings > Time & Language > Language & region.",
        FG_FAINT,
    );
}

fn draw_storage(p: &Painter, cv: &mut Canvas, v: &View, hits: &mut Vec<(LRect, Hit)>) {
    p.text(cv, CX, 52.0, 24.0, "Storage", FG);
    p.text(cv, CX, 76.0, 12.5, "Manage captured files.", FG_DIM);
    section_title(p, cv, 112.0, "Location", "Where screenshots live.");
    let c = card(CX, 146.0, 2);
    p.card(cv, c);
    row_label(
        p,
        cv,
        (c.0, c.1, c.2, ROW),
        Glyph::Folder,
        &short_path(&v.cfg.save_dir().display().to_string()),
        &format!("{} captures · {}", v.stats.0, human_bytes(v.stats.1)),
    );
    let bb = slot(c.1, 110.0);
    p.ghost_button(cv, bb, "Open in Finder", v.hover == Some(Hit::OpenDir));
    hits.push((bb, Hit::OpenDir));

    // second row: volume headroom — a segmented disk chart made a few
    // MB of captures indistinguishable from rounding noise
    p.line(cv, c.0 + 14.0, c.1 + ROW, c.0 + c.2 - 14.0);
    let r = (c.0, c.1 + ROW, c.2, ROW);
    row_label(
        p,
        cv,
        r,
        Glyph::Folder,
        "Free space",
        "On the disk holding your captures",
    );
    if let Some((total, free)) = v.disk {
        let txt = format!("{} free of {}", human_bytes(free), human_bytes(total));
        p.text(
            cv,
            r.0 + r.2 - 14.0 - p.text_w(&txt, 10.5),
            r.1 + 32.0,
            10.5,
            &txt,
            FG_DIM,
        );
    }

    section_title(
        p,
        cv,
        280.0,
        "Retention",
        "Automatically delete old captures.",
    );
    let c = card(CX, 314.0, 2);
    p.card(cv, c);
    let r = (c.0, c.1, c.2, ROW);
    row_label(
        p,
        cv,
        r,
        Glyph::Gear,
        "Keep captures",
        if v.cfg.keep_days == 0 {
            "Files stay until you remove them"
        } else {
            "Older files are removed automatically"
        },
    );
    let kb = (r.0 + r.2 - 14.0 - 150.0, r.1 + 11.0, 150.0, 30.0);
    let label = match v.cfg.keep_days {
        7 => "7 days",
        30 => "30 days",
        _ => "Forever",
    };
    dropdown_btn(
        p,
        cv,
        kb,
        label,
        v.drop_open == Some(Drop::KeepDays),
        v.hover == Some(Hit::Drop(Drop::KeepDays)),
    );
    hits.push((kb, Hit::Drop(Drop::KeepDays)));
    p.line(cv, c.0 + 14.0, c.1 + ROW, c.0 + c.2 - 14.0);
    let r = (c.0, c.1 + ROW, c.2, ROW);
    row_label(
        p,
        cv,
        r,
        Glyph::Folder,
        "Clean up now",
        if v.cfg.keep_days == 0 {
            "Choose a retention period first"
        } else {
            "Apply the retention policy immediately"
        },
    );
    let rb = slot(r.1, 110.0);
    if v.cfg.keep_days == 0 {
        // no policy, no action — a live button that deletes nothing is
        // worse than a visibly disabled one
        p.ghost_disabled(cv, rb, "Clean now");
    } else {
        ghost(
            p,
            cv,
            rb,
            "Clean now",
            v.hover == Some(Hit::Clean),
            v.confirm == Some(Hit::Clean),
        );
        hits.push((rb, Hit::Clean));
    }
    if let Some(m) = v.clean_msg {
        p.text(cv, CX + 8.0, c.1 + c.3 + 26.0, 11.5, m, FG_DIM);
    }
}

fn draw_about(p: &Painter, cv: &mut Canvas, v: &View, hits: &mut Vec<(LRect, Hit)>) {
    let mid = CX + CW / 2.0;
    p.text(
        cv,
        mid - p.text_w("Quickshort", 22.0) / 2.0,
        140.0,
        22.0,
        "Quickshort",
        FG,
    );
    let ver = concat!("v", env!("CARGO_PKG_VERSION"));
    p.text(
        cv,
        mid - p.text_w(ver, 12.5) / 2.0,
        162.0,
        12.5,
        ver,
        FG_DIM,
    );
    p.text(
        cv,
        mid - p.text_w("Fast native screenshots — annotate, OCR, share.", 11.5) / 2.0,
        186.0,
        11.5,
        "Fast native screenshots — annotate, OCR, share.",
        FG_FAINT,
    );

    let (label, w) = match v.update_ver {
        Some(vr) => (format!("Update to v{vr} — Restart"), 240.0),
        None => ("Check for Updates…".to_string(), 180.0),
    };
    let ub = (mid - w / 2.0, 220.0, w, 32.0);
    p.ghost_button(cv, ub, &label, v.hover == Some(Hit::Update));
    hits.push((ub, Hit::Update));

    // links
    let lw: f64 = LINKS
        .iter()
        .map(|(t, _)| p.text_w(t, 12.0) + 28.0)
        .sum::<f64>()
        - 28.0;
    let mut lx = mid - lw / 2.0;
    for (i, (t, _)) in LINKS.iter().enumerate() {
        let tw = p.text_w(t, 12.0);
        p.text(
            cv,
            lx,
            292.0,
            12.0,
            t,
            if v.hover == Some(Hit::Link(i)) {
                FG
            } else {
                p.accent_deep
            },
        );
        if v.hover == Some(Hit::Link(i)) {
            p.seg(cv, lx, 298.0, lx + tw, 298.0, 1.0, FG);
        }
        hits.push(((lx, 282.0, tw, 20.0), Hit::Link(i)));
        lx += tw + 28.0;
    }

    p.text(
        cv,
        mid - p.text_w("Free and open source — MIT", 10.5) / 2.0,
        570.0,
        10.5,
        "Free and open source — MIT",
        FG_FAINT,
    );
}

// config plumbing -----------------------------------------------------------

fn key_str(cfg: &Config, r: Rec) -> &str {
    match r {
        Rec::Save => &cfg.key_save,
        Rec::Copy => &cfg.key_copy,
        Rec::Ocr => &cfg.key_ocr,
        Rec::Share => &cfg.key_share,
        Rec::Scroll => &cfg.key_scroll,
        Rec::Record => &cfg.key_record,
        Rec::Undo => &cfg.key_undo,
        Rec::Global | Rec::GlobalFull => "",
    }
}

/// Assign `ch` to a key slot; if another slot already uses it, swap so the
/// binding stays unique — silent duplicates would make one action dead.
fn set_key(cfg: &mut Config, slot: Rec, ch: char) {
    let old = key_str(cfg, slot).chars().next().unwrap_or(' ');
    for other in [
        Rec::Save,
        Rec::Copy,
        Rec::Ocr,
        Rec::Share,
        Rec::Scroll,
        Rec::Record,
        Rec::Undo,
    ] {
        if other != slot && key_str(cfg, other).chars().next() == Some(ch) {
            *key_mut(cfg, other) = old.to_string();
        }
    }
    *key_mut(cfg, slot) = ch.to_string();
}

fn key_mut(cfg: &mut Config, r: Rec) -> &mut String {
    match r {
        Rec::Save => &mut cfg.key_save,
        Rec::Copy => &mut cfg.key_copy,
        Rec::Ocr => &mut cfg.key_ocr,
        Rec::Share => &mut cfg.key_share,
        Rec::Scroll => &mut cfg.key_scroll,
        Rec::Record => &mut cfg.key_record,
        Rec::Undo => &mut cfg.key_undo,
        Rec::Global | Rec::GlobalFull => unreachable!(),
    }
}

fn drop_apply(cfg: &mut Config, d: Drop, i: usize) {
    match d {
        Drop::RecentN => cfg.recent_max = [4u8, 8, 12][i.min(2)],
        Drop::KeepDays => {
            cfg.keep_days = [0u32, 7, 30][i.min(2)];
            // apply the new policy immediately, not just on next launch
            actions::cleanup_captures(&cfg.save_dir(), cfg.keep_days);
        }
        Drop::PopupPos => {
            cfg.popup_pos = POPUP_POS
                .get(i)
                .map(|(k, _)| *k)
                .unwrap_or(POPUP_POS[0].0)
                .into();
        }
    }
}

/// Thumbnails sized in *physical* px for the window's current scale.
fn load_thumbs(recents: &[PathBuf], scale: f64) -> Vec<Option<image::RgbaImage>> {
    let (tw, th) = ((THUMB_W * scale) as u32, (THUMB_H * scale) as u32);
    recents
        .iter()
        .map(|p| {
            image::open(p).ok().map(|i| {
                i.resize_to_fill(tw, th, image::imageops::FilterType::Triangle)
                    .to_rgba8()
            })
        })
        .collect()
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

/// Pin the window to the light Aqua appearance — the settings UI is
/// light-themed, so a dark titlebar looks out of place when macOS is in
/// dark mode.
#[cfg(target_os = "macos")]
fn force_light_appearance(win: &Window) {
    use objc2_app_kit::{NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSView};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = win.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return;
    };
    let ns_view: &NSView = unsafe { &*(h.ns_view.as_ptr() as *const NSView) };
    let Some(ns_win) = ns_view.window() else {
        return;
    };
    unsafe {
        ns_win.setAppearance(NSAppearance::appearanceNamed(NSAppearanceNameAqua).as_deref());
    }
}

#[cfg(not(target_os = "macos"))]
fn force_light_appearance(_win: &Window) {}

/// right-trimmed tail for display
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

/// "1.4 MB" from a byte count.
fn human_bytes(b: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let (mut v, mut u) = (b as f64, 0);
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
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
/// "⌘+s" style strings pass straight through.
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
                    other => other.to_uppercase(),
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                t.to_uppercase()
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
                    SettingsEvent::HotkeyRecord
                    | SettingsEvent::CheckUpdates
                    | SettingsEvent::InstallUpdate => {} // standalone: no host
                    SettingsEvent::Closed => el.exit(),
                }
            }
        }
        fn about_to_wait(&mut self, el: &ActiveEventLoop) {
            if let Some(s) = &mut self.s {
                s.about_to_wait();
                // a pack install reports via shared state — wake the loop
                // to poll it instead of waiting for a window event
                if s.poll_needed() {
                    el.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
                        std::time::Instant::now() + std::time::Duration::from_millis(250),
                    ));
                }
            }
        }
    }
    let el = winit::event_loop::EventLoop::new()?;
    let mut cfg = Config::load();
    // QS_ACCENT=name recolors the snapshot without touching saved prefs
    if let Ok(a) = std::env::var("QS_ACCENT") {
        cfg.accent = a;
    }
    let mut run = Run { cfg, s: None };
    el.run_app(&mut run)?;
    Ok(())
}

/// SDF + fontdue drawing helpers over the softbuffer surface.
struct Painter<'a> {
    font: &'a Font,
    scale: f64,
    /// current accent preset — mirrors the mockup's `--accent` variable
    accent: u32,
    /// accent darkened for white text/glyphs (nav pill, ticks, switches)
    accent_deep: u32,
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
                let d = if rad <= 0.0 {
                    // square corners: inside = negative distance to box
                    (lx - x - w / 2.0).abs().max(w / 2.0) + (ly - y - h / 2.0).abs().max(h / 2.0)
                        - (w / 2.0 + h / 2.0)
                } else {
                    rrect_sdf(lx, ly, x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0, rad)
                };
                let c = if rad <= 0.0 { a } else { fill_cov(d, s) * a };
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

    /// Stroke a segment (logical coords) with SDF coverage.
    fn seg(&self, cv: &mut Canvas, x0: f64, y0: f64, x1: f64, y1: f64, w: f64, rgb: u32) {
        let s = self.scale;
        let (bx0, by0) = (x0.min(x1) - w - 1.0, y0.min(y1) - w - 1.0);
        let (bx1, by1) = (x0.max(x1) + w + 1.0, y0.max(y1) + w + 1.0);
        for py in (by0 * s) as i32..(by1 * s) as i32 {
            for px in (bx0 * s) as i32..(bx1 * s) as i32 {
                let lx = px as f64 / s + 0.5 / s;
                let ly = py as f64 / s + 0.5 / s;
                let c = stroke_cov(seg_sdf(lx, ly, x0, y0, x1, y1), w, s);
                if c > 0.0 {
                    cv.blend(px, py, rgb, c);
                }
            }
        }
    }

    /// Grouped inset card: flat white panel + hairline border.
    fn card(&self, cv: &mut Canvas, r: LRect) {
        self.fill(cv, r, 10.0, CARD_TOP, 1.0);
        self.stroke(cv, r, 10.0, BORDER, 1.0, 0.75);
    }

    /// Text button — hairline border, fills accent on hover.
    fn ghost_button(&self, cv: &mut Canvas, r: LRect, label: &str, hover: bool) {
        if hover {
            self.fill(cv, r, r.3 / 2.0, ROW_HOV, 1.0);
        } else {
            self.fill(cv, r, r.3 / 2.0, 0xFFFFFF, 1.0);
        }
        self.stroke(cv, r, r.3 / 2.0, BORDER_STRONG, 1.0, 0.75);
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

    /// Unavailable action — dashed-looking hairline, no hover response.
    fn ghost_disabled(&self, cv: &mut Canvas, r: LRect, label: &str) {
        self.fill(cv, r, r.3 / 2.0, 0xFAFBFC, 1.0);
        self.stroke(cv, r, r.3 / 2.0, BORDER, 1.0, 0.6);
        let tw = self.text_w(label, 11.5);
        self.text(
            cv,
            r.0 + (r.2 - tw) / 2.0,
            r.1 + r.3 / 2.0 + 4.0,
            11.5,
            label,
            FG_FAINT,
        );
    }

    /// Keyboard-style keycaps, one per token.
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
        let track = mix(SWITCH_OFF, self.accent_deep, t);
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

    /// Accent preset circle; selected gets a hairline ring.
    fn swatch(&self, cv: &mut Canvas, r: LRect, rgb: u32, selected: bool, hover: bool) {
        let (cx, cy) = (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0);
        let rad = r.3 / 2.0;
        let s = self.scale;
        let (x0, y0) = (((cx - rad - 2.0) * s) as i32, ((cy - rad - 2.0) * s) as i32);
        let (x1, y1) = (((cx + rad + 2.0) * s) as i32, ((cy + rad + 2.0) * s) as i32);
        for py in y0..y1 {
            for px in x0..x1 {
                let lx = px as f64 / s + 0.5 / s;
                let ly = py as f64 / s + 0.5 / s;
                let d = ellipse_sdf(lx, ly, cx, cy, rad, rad);
                let c = fill_cov(d, s);
                if c > 0.0 {
                    cv.blend(px, py, rgb, c);
                }
            }
        }
        if selected || hover {
            self.stroke(
                cv,
                (r.0 - 3.0, r.1 - 3.0, r.2 + 6.0, r.3 + 6.0),
                r.3,
                if selected { rgb } else { FG_DIM },
                if selected { 1.0 } else { 0.5 },
                1.2,
            );
        }
    }

    /// Recent-capture thumbnail clipped to a rounded rect + hairline ring.
    fn thumb(&self, cv: &mut Canvas, r: LRect, img: Option<&image::RgbaImage>) {
        let s = self.scale;
        match img {
            Some(img) => {
                let (x0, y0) = ((r.0 * s) as i32, (r.1 * s) as i32);
                for py in 0..img.height() as i32 {
                    for px in 0..img.width() as i32 {
                        let lx = (x0 + px) as f64 / s + 0.5 / s;
                        let ly = (y0 + py) as f64 / s + 0.5 / s;
                        let d = rrect_sdf(
                            lx,
                            ly,
                            r.0 + r.2 / 2.0,
                            r.1 + r.3 / 2.0,
                            r.2 / 2.0,
                            r.3 / 2.0,
                            8.0,
                        );
                        let cov = fill_cov(d, s);
                        if cov <= 0.0 {
                            continue;
                        }
                        let c = img.get_pixel(px as u32, py as u32);
                        let a = cov * c[3] as f64 / 255.0;
                        let rgb = ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32;
                        cv.blend(x0 + px, y0 + py, rgb, a);
                    }
                }
            }
            None => self.fill(cv, r, 8.0, TILE, 1.0),
        }
        self.stroke(cv, r, 8.0, BORDER, 1.0, 0.75);
    }

    /// Nav / row glyphs — 18×18 logical, SDF strokes.
    fn glyph(&self, cv: &mut Canvas, g: Glyph, x: f64, y: f64, s: f64, rgb: u32) {
        let w = s * 0.11;
        match g {
            Glyph::Crop => {
                let ins = s * 0.18;
                let len = s * 0.26;
                let (x0, y0, x1, y1) = (x + ins, y + ins, x + s - ins, y + s - ins);
                for (ax, ay, bx, by) in [
                    (x0, y0 + len, x0, y0),
                    (x0, y0, x0 + len, y0),
                    (x1 - len, y0, x1, y0),
                    (x1, y0, x1, y0 + len),
                    (x0, y1 - len, x0, y1),
                    (x0, y1, x0 + len, y1),
                    (x1 - len, y1, x1, y1),
                    (x1, y1 - len, x1, y1),
                ] {
                    self.seg(cv, ax, ay, bx, by, w, rgb);
                }
            }
            Glyph::Gear => {
                let (cx, cy) = (x + s / 2.0, y + s / 2.0);
                for i in 0..8 {
                    let a = i as f64 * std::f64::consts::FRAC_PI_4;
                    let (dx, dy) = (a.cos(), a.sin());
                    self.seg(
                        cv,
                        cx + dx * s * 0.36,
                        cy + dy * s * 0.36,
                        cx + dx * s * 0.48,
                        cy + dy * s * 0.48,
                        w * 1.3,
                        rgb,
                    );
                }
                self.circle(cv, cx, cy, s * 0.28, w, rgb);
            }
            Glyph::Keys => {
                self.stroke(
                    cv,
                    (x + s * 0.08, y + s * 0.24, s * 0.84, s * 0.52),
                    3.0,
                    rgb,
                    1.0,
                    w,
                );
                for r in 0..2 {
                    for c in 0..3 {
                        self.fill(
                            cv,
                            (
                                x + s * (0.18 + c as f64 * 0.24),
                                y + s * (0.32 + r as f64 * 0.2),
                                s * 0.12,
                                s * 0.12,
                            ),
                            1.0,
                            rgb,
                            1.0,
                        );
                    }
                }
            }
            Glyph::Drop => {
                // half-filled circle: reads as a paint/appearance swatch
                let (cx, cy) = (x + s / 2.0, y + s / 2.0);
                self.circle(cv, cx, cy, s * 0.4, w, rgb);
                let sd = self.scale;
                let (x0, y0) = (((cx - s * 0.4) * sd) as i32, (cy * sd) as i32);
                let (x1, y1) = (((cx + s * 0.4) * sd) as i32, ((cy + s * 0.4) * sd) as i32);
                for py in y0..y1 {
                    for px in x0..x1 {
                        let lx = px as f64 / sd + 0.5 / sd;
                        let ly = py as f64 / sd + 0.5 / sd;
                        if ly < cy {
                            continue;
                        }
                        let c = fill_cov(ellipse_sdf(lx, ly, cx, cy, s * 0.4, s * 0.4), sd);
                        if c > 0.0 {
                            cv.blend(px, py, rgb, c);
                        }
                    }
                }
            }
            Glyph::Folder => {
                let body = (x + s * 0.06, y + s * 0.3, s * 0.88, s * 0.56);
                self.stroke(cv, body, 3.0, rgb, 1.0, w);
                self.fill(
                    cv,
                    (x + s * 0.06, y + s * 0.2, s * 0.4, s * 0.14),
                    2.0,
                    rgb,
                    1.0,
                );
            }
            Glyph::Info => {
                let (cx, cy) = (x + s / 2.0, y + s / 2.0);
                self.circle(cv, cx, cy, s * 0.42, w, rgb);
                self.fill(
                    cv,
                    (cx - s * 0.07, cy - s * 0.3, s * 0.14, s * 0.14),
                    s * 0.07,
                    rgb,
                    1.0,
                );
                self.seg(cv, cx, cy - s * 0.06, cx, cy + s * 0.24, w * 1.4, rgb);
            }
            Glyph::Text => {
                // three staggered text lines — reads as a document
                for (i, frac) in [0.62_f64, 0.84, 0.5].iter().enumerate() {
                    let ly = y + s * (0.28 + i as f64 * 0.22);
                    self.seg(
                        cv,
                        x + s * 0.16,
                        ly,
                        x + s * (0.16 + frac),
                        ly,
                        w * 1.4,
                        rgb,
                    );
                }
            }
            Glyph::Bolt => {
                self.seg(
                    cv,
                    x + s * 0.58,
                    y + s * 0.06,
                    x + s * 0.3,
                    y + s * 0.55,
                    w * 1.4,
                    rgb,
                );
                self.seg(
                    cv,
                    x + s * 0.3,
                    y + s * 0.55,
                    x + s * 0.55,
                    y + s * 0.55,
                    w * 1.4,
                    rgb,
                );
                self.seg(
                    cv,
                    x + s * 0.55,
                    y + s * 0.55,
                    x + s * 0.4,
                    y + s * 0.94,
                    w * 1.4,
                    rgb,
                );
            }
        }
    }

    /// Circle outline via ellipse SDF stroke.
    fn circle(&self, cv: &mut Canvas, cx: f64, cy: f64, r: f64, w: f64, rgb: u32) {
        let s = self.scale;
        let (x0, y0) = (((cx - r - w) * s) as i32, ((cy - r - w) * s) as i32);
        let (x1, y1) = (((cx + r + w) * s) as i32, ((cy + r + w) * s) as i32);
        for py in y0..y1 {
            for px in x0..x1 {
                let lx = px as f64 / s + 0.5 / s;
                let ly = py as f64 / s + 0.5 / s;
                let c = stroke_cov(ellipse_sdf(lx, ly, cx, cy, r, r), w, s);
                if c > 0.0 {
                    cv.blend(px, py, rgb, c);
                }
            }
        }
    }

    /// App glyph — the tray's crop-corners mark in an accent squircle.
    fn icon(&self, cv: &mut Canvas, x: f64, y: f64, size: f64) {
        self.fill(cv, (x, y, size, size), 9.0, self.accent_deep, 1.0);
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
            cv.blend(px, (y * s) as i32, BORDER, 1.0);
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
