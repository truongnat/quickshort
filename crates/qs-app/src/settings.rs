//! Preferences window — same stack as the overlay: one winit window,
//! softbuffer pixels, SDF-drawn widgets, fontdue text (system font looked
//! up at runtime, no bundled assets).

use anyhow::{Context, Result};
use fontdue::{Font, FontSettings};
use qs_overlay::sdf::{fill_cov, rrect_sdf, Canvas};
use softbuffer::{Context as SbContext, Surface};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::actions;
use crate::config::Config;
use crate::tray;

const W: f64 = 500.0;
const H: f64 = 340.0;
const BG: u32 = 0x1C1C1E;
const FIELD: u32 = 0x2C2C2E;
const FIELD_HOV: u32 = 0x3A3A3E;
const ACCENT: u32 = 0x0A84FF;
const FG: u32 = 0xF2F2F7;
const FG_DIM: u32 = 0x98989D;

/// What the host needs to know after a settings event.
pub enum SettingsEvent {
    /// Config changed + saved — re-register the hotkey, rebuild the menu.
    Changed,
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

pub struct Settings {
    win: Rc<Window>,
    surf: Surface<Rc<Window>, Rc<Window>>,
    font: Font,
    recording: bool,
    hover: Option<Hit>,
    mods: ModifiersState,
    recents: Vec<PathBuf>,
    last_pos: (f64, f64),
    dirty: bool,
    closed: bool,
}

impl Settings {
    pub fn open(el: &ActiveEventLoop, cfg: &Config) -> Result<Self> {
        let attrs = Window::default_attributes()
            .with_title("quickshort Settings")
            .with_inner_size(winit::dpi::LogicalSize::new(W, H))
            .with_resizable(false)
            .with_window_level(winit::window::WindowLevel::AlwaysOnTop);
        let win = Rc::new(el.create_window(attrs).context("settings window")?);
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
            recents: actions::recent_captures(&cfg.save_dir(), 6),
            last_pos: (0.0, 0.0),
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
            } => {
                let p = self.cursor_logical();
                match self.hit(p) {
                    Some(Hit::Record) => {
                        self.recording = true;
                        self.dirty = true;
                    }
                    Some(Hit::Browse) => {
                        if let Some(d) = rfd::FileDialog::new()
                            .set_directory(cfg.save_dir())
                            .pick_folder()
                        {
                            cfg.save_dir = Some(d);
                            self.recents = actions::recent_captures(&cfg.save_dir(), 6);
                            return Some(SettingsEvent::Changed);
                        }
                    }
                    Some(Hit::LoginToggle) => {
                        let want = !cfg.launch_at_login;
                        match tray::set_launch_at_login(want) {
                            Ok(()) => {
                                cfg.launch_at_login = want;
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
                    None => {}
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
                    if self.recording {
                        self.recording = false;
                        self.dirty = true;
                    } else {
                        self.closed = true;
                    }
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
                }
            }
            _ => {}
        }
        self.closed.then_some(SettingsEvent::Closed)
    }

    fn cursor_logical(&self) -> (f64, f64) {
        self.last_pos
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

    /// Hit-test logical point against the fixed layout.
    fn hit(&self, p: (f64, f64)) -> Option<Hit> {
        let (x, y) = p;
        let inside =
            |r: (f64, f64, f64, f64)| x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3;
        if inside(self.record_rect()) {
            return Some(Hit::Record);
        }
        if inside((398.0, 96.0, 82.0, 30.0)) {
            return Some(Hit::Browse);
        }
        if inside((20.0, 140.0, 200.0, 24.0)) {
            return Some(Hit::LoginToggle);
        }
        for (i, _) in self.recents.iter().enumerate() {
            if inside((20.0, 216.0 + i as f64 * 22.0, W - 40.0, 22.0)) {
                return Some(Hit::Recent(i));
            }
        }
        None
    }

    fn record_rect(&self) -> (f64, f64, f64, f64) {
        (352.0, 58.0, 128.0, 30.0)
    }

    fn draw(&mut self, cfg: &Config) {
        self.dirty = false;
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
        // hoist all the state the draw code needs — `buffer_mut` holds a
        // mutable borrow of `self.surf` for the rest of the function
        let rr = self.record_rect();
        let hover = self.hover;
        let recording = self.recording;
        let recents = self.recents.clone();
        let Ok(mut buf) = self.surf.buffer_mut() else {
            return;
        };
        buf.fill(BG);
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

        p.text(&mut cv, 20.0, 36.0, 18.0, "quickshort Settings", FG);

        // Global hotkey row
        p.text(&mut cv, 20.0, 80.0, 13.0, "Global hotkey", FG_DIM);
        let chip = (170.0, 58.0, 170.0, 30.0);
        p.field(&mut cv, chip);
        let label = if recording {
            "Press shortcut…".to_string()
        } else {
            pretty_hotkey(&cfg.hotkey)
        };
        p.text(
            &mut cv,
            chip.0 + 10.0,
            chip.1 + 20.0,
            13.0,
            &label,
            if recording { ACCENT } else { FG },
        );
        p.button(&mut cv, rr, "Record", hover == Some(Hit::Record));

        // Save folder row
        p.text(&mut cv, 20.0, 122.0, 13.0, "Save folder", FG_DIM);
        let f = (170.0, 96.0, 216.0, 30.0);
        p.field(&mut cv, f);
        p.text(
            &mut cv,
            f.0 + 10.0,
            f.1 + 20.0,
            12.0,
            &tail(&cfg.save_dir().display().to_string(), 30),
            FG,
        );
        p.button(
            &mut cv,
            (398.0, 96.0, 82.0, 30.0),
            "Browse…",
            hover == Some(Hit::Browse),
        );

        // Launch at login
        let cb = (20.0, 140.0, 18.0, 18.0);
        p.checkbox(
            &mut cv,
            cb,
            cfg.launch_at_login,
            hover == Some(Hit::LoginToggle),
        );
        p.text(&mut cv, 46.0, 154.0, 13.0, "Launch at login", FG);

        // Divider + recents
        p.line(&mut cv, 20.0, 178.0, W - 20.0);
        p.text(&mut cv, 20.0, 202.0, 13.0, "Recent captures", FG_DIM);
        for (i, r) in recents.iter().enumerate() {
            let y = 216.0 + i as f64 * 22.0;
            if hover == Some(Hit::Recent(i)) {
                p.fill(&mut cv, (20.0, y, W - 40.0, 22.0), 4.0, FIELD_HOV, 1.0);
            }
            let name = r
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            p.text(
                &mut cv,
                28.0,
                y + 15.0,
                12.0,
                &name,
                if hover == Some(Hit::Recent(i)) {
                    FG
                } else {
                    FG_DIM
                },
            );
        }
        if recents.is_empty() {
            p.text(&mut cv, 28.0, 231.0, 12.0, "No captures yet", FG_DIM);
        }

        let _ = buf.present();
    }
}

/// right-trimmed path tail for display
fn tail(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!(
            "…{}",
            s.chars()
                .rev()
                .take(n)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        )
    }
}

/// "CmdOrCtrl+Shift+S" -> "⌃⇧⌘S"-ish display: show modifier glyphs on
/// macOS, "+"-joined names elsewhere.
fn pretty_hotkey(s: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        let mut out = String::new();
        for t in s.split('+') {
            out.push_str(match t.trim() {
                "CmdOrCtrl" | "Cmd" | "Super" | "Command" => "⌘",
                "Ctrl" | "Control" => "⌃",
                "Alt" | "Option" => "⌥",
                "Shift" => "⇧",
                other => other,
            });
        }
        out
    }
    #[cfg(not(target_os = "macos"))]
    {
        s.to_string()
    }
}

/// winit `Key` -> global-hotkey key token.
fn key_token(key: &Key) -> Option<String> {
    match key {
        Key::Character(c) => {
            let u = c.to_uppercase();
            // letters, digits and symbol chars parse as themselves
            if u.chars().count() == 1 {
                Some(u)
            } else {
                None
            }
        }
        Key::Named(n) => Some(
            match n {
                NamedKey::Space => "Space",
                NamedKey::Enter => "Enter",
                NamedKey::Tab => "Tab",
                NamedKey::Backspace => "Backspace",
                NamedKey::Delete => "Delete",
                NamedKey::Insert => "Insert",
                NamedKey::Home => "Home",
                NamedKey::End => "End",
                NamedKey::PageUp => "PageUp",
                NamedKey::PageDown => "PageDown",
                NamedKey::ArrowUp => "Up",
                NamedKey::ArrowDown => "Down",
                NamedKey::ArrowLeft => "Left",
                NamedKey::ArrowRight => "Right",
                NamedKey::PrintScreen => "PrintScreen",
                other => {
                    let d = format!("{other:?}");
                    if d.starts_with('F') && d[1..].chars().all(|c| c.is_ascii_digit()) {
                        return Some(d);
                    }
                    return None;
                }
            }
            .to_string(),
        ),
        _ => None,
    }
}

/// Dev harness: `quickshort --settings` opens just the preferences window
/// in a one-shot event loop.
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
        fn window_event(
            &mut self,
            el: &ActiveEventLoop,
            id: winit::window::WindowId,
            ev: WindowEvent,
        ) {
            let Some(s) = &mut self.s else { return };
            if s.win_id() != id {
                return;
            }
            if let Some(e) = s.window_event(&mut self.cfg, ev) {
                match e {
                    SettingsEvent::Changed => self.cfg.save(),
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

/// First usable system font (fontdue reads TTF/OTF).
fn system_font() -> Result<Font> {
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
        #[cfg(target_os = "linux")]
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
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

/// SDF + fontdue drawing helpers over the softbuffer surface.
struct Painter<'a> {
    font: &'a Font,
    scale: f64,
}

impl Painter<'_> {
    /// Logical coords in, physical coverage out. `r` = (x, y, w, h).
    fn fill(&self, cv: &mut Canvas, r: (f64, f64, f64, f64), rad: f64, rgb: u32, a: f64) {
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

    fn button(&self, cv: &mut Canvas, r: (f64, f64, f64, f64), label: &str, hover: bool) {
        self.fill(cv, r, 7.0, if hover { ACCENT } else { FIELD }, 1.0);
        let tw = self.text_w(label, 12.0);
        self.text(cv, r.0 + (r.2 - tw) / 2.0, r.1 + 20.0, 12.0, label, FG);
    }

    fn field(&self, cv: &mut Canvas, r: (f64, f64, f64, f64)) {
        self.fill(cv, r, 7.0, FIELD, 1.0);
    }

    fn checkbox(&self, cv: &mut Canvas, r: (f64, f64, f64, f64), on: bool, hover: bool) {
        let bg = if on {
            ACCENT
        } else if hover {
            FIELD_HOV
        } else {
            FIELD
        };
        self.fill(cv, r, 5.0, bg, 1.0);
        if on {
            // checkmark: two strokes
            let (cx, cy) = (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0);
            let s = self.scale;
            for py in (r.1 * s) as i32..((r.1 + r.3) * s) as i32 {
                for px in (r.0 * s) as i32..((r.0 + r.2) * s) as i32 {
                    let lx = px as f64 / s + 0.5 / s;
                    let ly = py as f64 / s + 0.5 / s;
                    let d =
                        qs_overlay::sdf::seg_sdf(lx, ly, cx - 4.5, cy + 0.2, cx - 1.0, cy + 3.6)
                            .min(qs_overlay::sdf::seg_sdf(
                                lx,
                                ly,
                                cx - 1.0,
                                cy + 3.6,
                                cx + 4.8,
                                cy - 3.8,
                            ));
                    let c = (1.0 - d).clamp(0.0, 1.0);
                    if c > 0.0 {
                        cv.blend(px, py, 0xFFFFFF, c);
                    }
                }
            }
        }
    }

    fn line(&self, cv: &mut Canvas, x0: f64, y: f64, x1: f64) {
        let s = self.scale;
        for px in (x0 * s) as i32..(x1 * s) as i32 {
            cv.blend(px, (y * s) as i32, 0xFFFFFF, 0.12);
        }
    }

    /// Draw `s` with baseline at logical (x, y); returns advance width in
    /// logical px.
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
