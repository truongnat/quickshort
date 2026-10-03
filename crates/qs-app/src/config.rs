//! `~/.config/quickshort/config.toml` — user-tweakable settings.
//! Missing file / unknown keys fall back to defaults; a corrupt file never
//! kills the app.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Global hotkey in `global-hotkey` syntax, e.g. "CmdOrCtrl+Shift+S".
    pub hotkey: String,
    /// Screenshot folder override; `None` = `~/Pictures/quickshort`.
    pub save_dir: Option<PathBuf>,
    pub launch_at_login: bool,
    /// Target language for ⌘T translate (BCP-47: "vi", "en", "ja", ...).
    pub translate_target: String,
    /// Source language; "auto" lets the backend detect it.
    pub translate_source: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "CmdOrCtrl+Shift+S".into(),
            save_dir: None,
            launch_at_login: false,
            translate_target: "vi".into(),
            translate_source: "auto".into(),
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
}
