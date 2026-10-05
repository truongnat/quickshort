//! Resident tray app: one persistent winit event loop; the overlay is
//! spawned on demand (tray menu click or global hotkey) and torn down when
//! the selection phase ends.

use anyhow::Result;
use global_hotkey::{hotkey::HotKey, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use muda::{MenuEvent, MenuId};
use qs_overlay::{Overlay, OverlayEvent};
use std::path::PathBuf;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::WindowId;

use crate::actions::{self, Outcome, SaveTarget};
use crate::config::Config;
use crate::settings::{Settings, SettingsEvent};
use crate::toast::Toast;
use crate::tray;
use crate::update::{self, Release};

/// Everything that wakes the loop besides window events.
enum HostEvent {
    /// Tray "Capture Region" or the global hotkey.
    Trigger,
    Menu(MenuId),
    /// Action worker finished — refresh the menu, update the toast.
    Done(Result<Outcome, String>),
    /// Release check finished — `manual` gates the "up to date" toast.
    UpdateCheck {
        res: Result<Option<Release>, String>,
        manual: bool,
    },
    /// Update install finished — relaunch on success.
    UpdateInstall(Result<PathBuf, String>),
}

/// Run the resident tray app. Blocks until Quit.
pub fn run(cfg: Config) -> Result<()> {
    let el = EventLoop::<HostEvent>::with_user_event().build()?;
    el.set_control_flow(ControlFlow::Wait);
    let proxy = el.create_proxy();

    // Menu clicks arrive on an mpsc channel — pump them into the event loop.
    let px = proxy.clone();
    std::thread::spawn(move || {
        while let Ok(e) = MenuEvent::receiver().recv() {
            if px.send_event(HostEvent::Menu(e.id().clone())).is_err() {
                return;
            }
        }
    });
    // Hotkey events: forward straight through the proxy.
    let px = proxy.clone();
    GlobalHotKeyEvent::set_event_handler(Some(Box::new(move |e: GlobalHotKeyEvent| {
        if e.state == HotKeyState::Pressed {
            let _ = px.send_event(HostEvent::Trigger);
        }
    })));

    let mut host = Host::new(cfg, proxy);
    el.run_app(&mut host)?;
    Ok(())
}

struct Host {
    cfg: Config,
    proxy: EventLoopProxy<HostEvent>,
    overlay: Option<Overlay>,
    /// Frames captured for the active overlay session — kept for stitching.
    frames: Option<Vec<qs_core::CapturedFrame>>,
    tray: Option<tray_icon::TrayIcon>,
    hotkeys: Option<GlobalHotKeyManager>,
    hotkey: Option<HotKey>,
    /// hotkey unregistered while the settings recorder is armed
    hotkey_suspended: bool,
    settings: Option<Settings>,
    toast: Option<Toast>,
    /// Newer release found, waiting for the user to click Update.
    pending_update: Option<Release>,
    /// Lazy — the listener + ssh tunnel only spawn on the first Share.
    share: Option<std::sync::Arc<crate::share::ShareManager>>,
}

impl Host {
    fn new(cfg: Config, proxy: EventLoopProxy<HostEvent>) -> Self {
        Self {
            cfg,
            proxy,
            overlay: None,
            frames: None,
            tray: None,
            hotkeys: None,
            hotkey: None,
            hotkey_suspended: false,
            settings: None,
            toast: None,
            pending_update: None,
            share: None,
        }
    }

    fn rebuild_menu(&self) {
        if let Some(t) = &self.tray {
            let recents =
                actions::recent_captures(&self.cfg.save_dir(), self.cfg.recent_max as usize);
            t.set_menu(Some(Box::new(tray::build_menu(
                &self.cfg,
                &recents,
                self.pending_update.as_ref().map(|r| r.version.as_str()),
            ))));
        }
    }

    /// Hit GitHub Releases on a worker; `manual` shows a busy toast now and
    /// an "up to date" toast later.
    fn check_updates(&mut self, el: &ActiveEventLoop, manual: bool) {
        self.cfg.last_update_check = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.cfg.save();
        if manual && self.toast.is_none() {
            if let Ok(t) = Toast::open(el, "Checking for updates…") {
                self.toast = Some(t);
            }
        }
        let px = self.proxy.clone();
        std::thread::spawn(move || {
            let _ = px.send_event(HostEvent::UpdateCheck {
                res: update::check().map_err(|e| format!("{e:#}")),
                manual,
            });
        });
    }

    /// Download + swap the pending release on a worker.
    #[cfg(target_os = "macos")]
    fn install_update(&mut self, el: &ActiveEventLoop, rel: Release) {
        if self.toast.is_none() {
            let msg = format!("Updating to v{}…", rel.version);
            if let Ok(t) = Toast::open(el, &msg) {
                self.toast = Some(t);
            }
        }
        let px = self.proxy.clone();
        std::thread::spawn(move || {
            let _ = px.send_event(HostEvent::UpdateInstall(
                update::install(&rel).map_err(|e| format!("{e:#}")),
            ));
        });
    }

    /// In-place update is macOS-only for now.
    #[cfg(not(target_os = "macos"))]
    fn install_update(&mut self, _el: &ActiveEventLoop, _rel: Release) {}

    /// Capture every monitor and spawn the overlay session.
    fn trigger(&mut self, el: &ActiveEventLoop) {
        eprintln!("trigger: overlay={}", self.overlay.is_some());
        if self.overlay.is_some() {
            return; // a selection is already in progress
        }
        let frames = match qs_capture::backend().and_then(|b| b.capture_all()) {
            Ok(f) if !f.is_empty() => f,
            Ok(_) => return eprintln!("no monitors captured"),
            Err(e) => {
                eprintln!("capture failed: {e:#}");
                if self.toast.is_none() {
                    if let Ok(t) = Toast::open(el, &format!("Capture failed: {e:#}")) {
                        self.toast = Some(t);
                    }
                }
                // LSUIElement apps get no TCC prompt — open the pane so the
                // user can grant Screen Recording manually
                if cfg!(target_os = "macos") {
                    let _ = std::process::Command::new("open")
                        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
                        .spawn();
                }
                return;
            }
        };
        let bounds = qs_core::virtual_bounds(&frames);
        let prefs = self.cfg.overlay_prefs();
        match Overlay::spawn(el, &frames, bounds, &prefs) {
            Ok(ov) => {
                self.frames = Some(frames);
                self.overlay = Some(ov);
            }
            Err(e) => eprintln!("overlay failed: {e:#}"),
        }
    }

    fn on_menu(&mut self, el: &ActiveEventLoop, id: MenuId) {
        let id = id.0.as_str();
        match id {
            tray::ID_CAPTURE => self.trigger(el),
            tray::ID_OPEN_DIR => actions::open_path(&self.cfg.save_dir()),
            tray::ID_QUIT => el.exit(),
            tray::ID_DONATE => actions::open_path(std::path::Path::new(
                "https://truongnat.github.io/quickshort/#donate",
            )),
            tray::ID_UPDATE => {
                if let Some(rel) = self.pending_update.take() {
                    self.install_update(el, rel);
                } else {
                    self.check_updates(el, true);
                }
            }
            tray::ID_SETTINGS => {
                if self.settings.is_none() {
                    match Settings::open(el, &self.cfg) {
                        Ok(mut s) => {
                            s.set_update(self.pending_update.as_ref().map(|r| r.version.clone()));
                            self.settings = Some(s);
                        }
                        Err(e) => eprintln!("settings: {e:#}"),
                    }
                } else {
                    // focus the existing window
                    if let Some(s) = &self.settings {
                        s.win().request_user_attention(None);
                        s.focus();
                    }
                }
            }
            tray::ID_LOGIN => {
                self.cfg.launch_at_login = !self.cfg.launch_at_login;
                if let Err(e) = tray::set_launch_at_login(self.cfg.launch_at_login) {
                    eprintln!("launch-at-login: {e:#}");
                    self.cfg.launch_at_login = !self.cfg.launch_at_login; // revert
                }
                self.cfg.save();
                self.rebuild_menu();
            }
            _ => {
                if let Some(p) = id.strip_prefix(tray::ID_RECENT_PREFIX) {
                    actions::open_path(&PathBuf::from(p));
                }
            }
        }
    }

    /// While the settings recorder is armed, drop the global grab so the
    /// chord reaches the settings window instead of triggering capture.
    fn suspend_hotkey(&mut self, suspended: bool) {
        if suspended == self.hotkey_suspended {
            return;
        }
        if let (Some(m), Some(hk)) = (&self.hotkeys, self.hotkey) {
            let r = if suspended {
                m.unregister(hk)
            } else {
                m.register(hk)
            };
            if let Err(e) = r {
                eprintln!("hotkey suspend: {e:#}");
                return;
            }
        }
        self.hotkey_suspended = suspended;
    }

    /// Persist config, (re)register the hotkey, refresh the tray menu —
    /// called after the settings window mutates `cfg`.
    fn apply_cfg(&mut self, el: &ActiveEventLoop) {
        self.cfg.save();
        if let (Some(m), Ok(hk)) = (&self.hotkeys, self.cfg.hotkey.parse::<HotKey>()) {
            if Some(hk) != self.hotkey {
                if let Some(old) = self.hotkey {
                    let _ = m.unregister(old);
                }
                match m.register(hk) {
                    Ok(()) => {
                        self.hotkey = Some(hk);
                        self.toast_done(el, format!("Hotkey set to {}", self.cfg.hotkey), true);
                    }
                    Err(e) => {
                        eprintln!("hotkey register failed: {e:#}");
                        // restore the old grab — a dead hotkey is worse than
                        // a stale one, and self.hotkey must stay truthful
                        if let Some(old) = self.hotkey {
                            let _ = m.register(old);
                        }
                        self.toast_done(
                            el,
                            format!(
                                "Hotkey {} is already in use — kept the old one",
                                self.cfg.hotkey
                            ),
                            false,
                        );
                    }
                }
            }
        }
        self.rebuild_menu();
    }

    /// Mirror a finished action into the toast (opened for OCR; lazily
    /// created for failures so errors are visible even for fast actions).
    fn toast_done(&mut self, el: &ActiveEventLoop, msg: String, ok: bool) {
        match &mut self.toast {
            Some(t) => t.done(msg, ok),
            None => match Toast::open(el, &msg) {
                Ok(mut t) => {
                    t.done(msg, ok);
                    self.toast = Some(t);
                }
                Err(e) => eprintln!("toast: {e:#}"),
            },
        }
    }
}

impl ApplicationHandler<HostEvent> for Host {
    fn resumed(&mut self, _el: &ActiveEventLoop) {
        // Tray-only app — hide the Dock icon (macOS only; elsewhere no-op).
        #[cfg(target_os = "macos")]
        {
            use objc2::MainThreadMarker;
            use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
            if let Some(mtm) = MainThreadMarker::new() {
                NSApplication::sharedApplication(mtm)
                    .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            }
        }

        #[cfg(target_os = "linux")]
        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            // GNOME Wayland: tray needs the AppIndicator extension, and
            // global-hotkey's X11 grabs never reach us. The supported path
            // is a GNOME custom shortcut running `quickshort --capture`.
            eprintln!(
                "note: on Wayland, tray/hotkey may not work — bind \
                 `quickshort --capture` in Settings > Keyboard instead"
            );
        }

        let recents = actions::recent_captures(&self.cfg.save_dir(), self.cfg.recent_max as usize);
        let builder = tray_icon::TrayIcon::builder()
            .with_tooltip("Quickshort — region screenshot")
            .with_menu(Box::new(tray::build_menu(&self.cfg, &recents, None)));
        // template icon = auto dark/light menu-bar tinting (macOS only)
        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_templated(tray::tray_icon(44));
        #[cfg(not(target_os = "macos"))]
        let builder = builder.with_icon(tray::tray_icon(44));
        match builder.build() {
            Ok(t) => self.tray = Some(t),
            Err(e) => eprintln!("tray icon failed: {e:#}"),
        }

        match self.cfg.hotkey.parse::<HotKey>() {
            Ok(hk) => match GlobalHotKeyManager::new().and_then(|m| {
                m.register(hk)?;
                Ok(m)
            }) {
                Ok(m) => {
                    self.hotkey = Some(hk);
                    self.hotkeys = Some(m);
                }
                Err(e) => eprintln!("hotkey register failed: {e:#}"),
            },
            Err(e) => eprintln!("bad hotkey {:?}: {e}", self.cfg.hotkey),
        }

        // honor the retention policy, then the once-a-day update check
        let gone = actions::cleanup_captures(&self.cfg.save_dir(), self.cfg.keep_days);
        if gone > 0 {
            eprintln!("retention sweep removed {gone} old captures");
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if self.cfg.auto_update && now.saturating_sub(self.cfg.last_update_check) > 24 * 3600 {
            self.check_updates(_el, false);
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, ev: HostEvent) {
        match ev {
            HostEvent::Trigger => self.trigger(el),
            HostEvent::Menu(id) => self.on_menu(el, id),
            HostEvent::UpdateCheck { res, manual } => match res {
                Ok(Some(rel)) => {
                    let v = rel.version.clone();
                    self.pending_update = Some(rel);
                    if let Some(s) = &mut self.settings {
                        s.set_update(Some(v.clone()));
                    }
                    self.rebuild_menu();
                    self.toast_done(el, format!("v{v} available — Update from the menu"), true);
                }
                Ok(None) => {
                    if let Some(s) = &mut self.settings {
                        s.set_update(None);
                    }
                    if manual {
                        self.toast_done(el, "You're up to date".to_string(), true);
                    }
                }
                Err(e) => {
                    if manual {
                        self.toast_done(el, format!("Update check failed: {e}"), false);
                    } else {
                        eprintln!("update check: {e}");
                    }
                }
            },
            HostEvent::UpdateInstall(res) => match res {
                Ok(app) => {
                    #[cfg(target_os = "macos")]
                    let _ = update::relaunch(&app);
                    #[cfg(not(target_os = "macos"))]
                    let _ = app;
                    el.exit();
                }
                Err(e) => self.toast_done(el, format!("Update failed: {e}"), false),
            },
            HostEvent::Done(res) => {
                eprintln!("done: {res:?}");
                match res {
                    Ok(Outcome::Saved(p)) => {
                        self.rebuild_menu();
                        let name = p
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        self.toast_done(el, format!("Saved {name}"), true);
                    }
                    Ok(Outcome::Copied) => {
                        self.toast_done(el, "Copied to clipboard".to_string(), true)
                    }
                    Ok(Outcome::Shared(url)) => {
                        self.toast_done(el, format!("Link copied — {url}"), true)
                    }
                    Ok(Outcome::TextCopied(n)) => {
                        self.toast_done(el, format!("Copied {n} chars"), true)
                    }
                    Ok(Outcome::NoText) => self.toast_done(el, "No text found".to_string(), false),
                    Err(e) => self.toast_done(el, format!("Failed: {e}"), false),
                }
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if let Some(t) = &mut self.toast {
            if t.win_id() == id {
                if t.window_event(event) {
                    self.toast = None;
                }
                return;
            }
        }
        let is_settings = self.settings.as_ref().is_some_and(|s| s.win_id() == id);
        if is_settings {
            let s = self.settings.as_mut().unwrap();
            let ev = s.window_event(&mut self.cfg, event);
            let recording = s.is_recording();
            match ev {
                Some(SettingsEvent::Changed) => {
                    // resume the grab only when the chord actually ended
                    // recording — a Browse/login change while still armed
                    // must keep it suspended or the next chord fires capture
                    self.suspend_hotkey(recording);
                    self.apply_cfg(el);
                }
                Some(SettingsEvent::HotkeyRecord) => self.suspend_hotkey(recording),
                Some(SettingsEvent::CheckUpdates) => self.check_updates(el, true),
                Some(SettingsEvent::InstallUpdate) => {
                    if let Some(rel) = self.pending_update.take() {
                        self.install_update(el, rel);
                    }
                }
                Some(SettingsEvent::Closed) => {
                    self.suspend_hotkey(false);
                    self.settings = None;
                }
                None => {}
            }
            return;
        }
        let Some(ov) = &mut self.overlay else { return };
        let Some(out) = ov.window_event(id, event) else {
            return;
        };
        self.overlay = None; // dropping destroys the overlay windows
        let Some(frames) = self.frames.take() else {
            return;
        };
        if let OverlayEvent::Done(sel, action, annots) = out {
            // slow actions get a live toast while the worker runs
            if self.toast.is_none() {
                let wait_msg = match action {
                    qs_overlay::Action::Ocr => Some("Extracting text…"),
                    qs_overlay::Action::Share => Some("Publishing link…"),
                    _ => None,
                };
                if let Some(m) = wait_msg {
                    match Toast::open(el, m) {
                        Ok(t) => self.toast = Some(t),
                        Err(e) => eprintln!("toast: {e:#}"),
                    }
                }
            }
            // stitch + encode off the UI thread; Done updates menu/toast later
            let px = self.proxy.clone();
            let target = SaveTarget::Dir(self.cfg.save_dir());
            let share = (action == qs_overlay::Action::Share).then(|| {
                self.share
                    .get_or_insert_with(crate::share::ShareManager::new)
                    .clone()
            });
            let copy = self.cfg.copy_after_save;
            let lang = self.cfg.ocr_lang.clone();
            std::thread::spawn(move || {
                let res = actions::dispatch(
                    frames, sel, action, &annots, &target, false, share, copy, &lang,
                )
                        .map_err(|e| e.to_string());
                let _ = px.send_event(HostEvent::Done(res));
            });
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if let Some(ov) = &mut self.overlay {
            ov.about_to_wait();
        }
        if let Some(s) = &self.settings {
            s.about_to_wait();
        }
        if let Some(t) = &mut self.toast {
            match t.pump() {
                Some(wake) => el.set_control_flow(ControlFlow::WaitUntil(wake)),
                None => {
                    self.toast = None;
                    el.set_control_flow(ControlFlow::Wait);
                }
            }
        }
    }
}
