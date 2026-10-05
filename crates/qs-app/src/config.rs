//! `~/.config/quickshort/config.toml` — user-tweakable settings.
//! Missing file / unknown keys fall back to defaults; a corrupt file never
//! kills the app.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Global hotkey in `global-hotkey` syntax, e.g. "CmdOrCtrl+Shift+S".
    pub hotkey: String,
    /// Screenshot folder override; `None` = `~/Pictures/quickshort`.
    pub save_dir: Option<PathBuf>,
    pub launch_at_login: bool,
    /// Once-a-day release check from GitHub.
    pub auto_update: bool,
    /// How many recent captures the tray menu lists.
    pub recent_max: u8,
    /// Selection/highlight accent preset name (see `accent_rgb`).
    pub accent: String,
    /// Also put the PNG on the clipboard after a Save.
    pub copy_after_save: bool,
    /// Auto-delete captures older than this many days; 0 = keep forever.
    pub keep_days: u32,
    /// Overlay ⌘+key triggers — single chars (see `overlay_prefs`).
    pub key_save: String,
    pub key_copy: String,
    pub key_ocr: String,
    pub key_share: String,
    /// ⌘+key = undo, ⌘⇧+key = redo — one field drives both.
    pub key_undo: String,
    /// tesseract language list, e.g. "eng", "eng+jpn", "eng+vie".
    pub ocr_lang: String,
    /// Unix ts of the last update check (auto-checks once a day).
    #[serde(default)]
    pub last_update_check: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "CmdOrCtrl+Shift+S".into(),
            save_dir: None,
            launch_at_login: false,
            auto_update: true,
            recent_max: 8,
            accent: "blue".into(),
            copy_after_save: false,
            keep_days: 0,
            key_save: "s".into(),
            key_copy: "c".into(),
            key_ocr: "t".into(),
            key_share: "l".into(),
            key_undo: "z".into(),
            ocr_lang: "auto".into(),
            last_update_check: 0,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("quickshort")
            .join("config.toml")
    }

    pub fn load() -> Self {
        let Ok(s) = std::fs::read_to_string(Self::path()) else {
            return Self::default();
        };
        toml::from_str(&s).unwrap_or_else(|e| {
            eprintln!("config parse failed ({e}), using defaults");
            Self::default()
        })
    }

    pub fn save(&self) {
        let p = Self::path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        match toml::to_string_pretty(self) {
            Ok(s) => {
                if let Err(e) = std::fs::write(&p, s) {
                    eprintln!("config write {}: {e}", p.display());
                }
            }
            Err(e) => eprintln!("config serialize: {e}"),
        }
    }

    /// Effective screenshot folder (default `~/Pictures/quickshort`).
    pub fn save_dir(&self) -> PathBuf {
        self.save_dir.clone().unwrap_or_else(|| {
            dirs::picture_dir()
                .or_else(dirs::home_dir)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("quickshort")
        })
    }

    /// Accent preset name -> 0xRRGGBB. Unknown names fall back to blue.
    pub fn accent_rgb(&self) -> u32 {
        accent_rgb(&self.accent)
    }

    /// Overlay prefs mapped from config (accent + ⌘+key triggers).
    pub fn overlay_prefs(&self) -> qs_overlay::Prefs {
        let key = |s: &str, d: char| s.chars().next().unwrap_or(d).to_ascii_lowercase();
        qs_overlay::Prefs {
            accent: self.accent_rgb(),
            keys: qs_overlay::Keys {
                save: key(&self.key_save, 's'),
                copy: key(&self.key_copy, 'c'),
                ocr: key(&self.key_ocr, 't'),
                share: key(&self.key_share, 'l'),
                undo: key(&self.key_undo, 'z'),
            },
        }
    }
}

/// Accent presets offered by the Appearance page — (name, rgb).
pub const ACCENTS: &[(&str, u32)] = &[
    ("blue", 0x0A84FF),
    ("purple", 0xBF5AF2),
    ("pink", 0xFF375F),
    ("orange", 0xFF9F0A),
    ("green", 0x30D158),
    ("gray", 0x98989D),
];

pub fn accent_rgb(name: &str) -> u32 {
    ACCENTS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, rgb)| *rgb)
        .unwrap_or(ACCENTS[0].1)
}
