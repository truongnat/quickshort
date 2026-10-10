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

use crate::actions::{self, Dispatch, Outcome, SaveTarget};
use crate::chrome::{self, Chrome, ChromeCmd, ChromeEv, Land, ShotKind};
use crate::config::Config;
use crate::pin::Pin;
use crate::quick_access::{QaAction, QuickAccess};
use crate::settings::{Settings, SettingsEvent};
use crate::toast::Toast;
use crate::tray;
use crate::update::{self, Release};

/// Everything that wakes the loop besides window events.
pub(crate) enum HostEvent {
    /// Tray "Capture Region" or the global hotkey.
    Trigger,
    Menu(MenuId),
    /// Action worker finished — refresh the menu, open the result card.
    Done(Result<Dispatch, String>),
    /// A share upload kicked off from the quick-access card finished.
    QuickShare(Result<crate::share::SharedLink, String>),
    /// Scroll-capture worker finished — the stitched tall image or error,
    /// plus the selection for the fly-in origin.
    ScrollDone(Result<image::RgbaImage, String>, qs_core::Rect),
    /// Recording worker finished — (first frame, mp4 path) or error, plus
    /// the selection.
    RecDone(Result<(image::RgbaImage, std::path::PathBuf), String>, qs_core::Rect),
    /// Release check finished — `manual` gates the "up to date" toast.
    UpdateCheck {
        res: Result<Option<Release>, String>,
        manual: bool,
    },
    /// Update install finished — relaunch on success.
    UpdateInstall(Result<PathBuf, String>),
    /// Input raised by a chrome-layer view (stack hover/clicks/keys).
    Chrome(WindowId, ChromeEv),
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
    /// Linux: gtk pins the tray to its own thread — the handle is a channel
    #[cfg(target_os = "linux")]
    tray: Option<tray::TrayHandle>,
    #[cfg(not(target_os = "linux"))]
    tray: Option<tray_icon::TrayIcon>,
    hotkeys: Option<GlobalHotKeyManager>,
    hotkey: Option<HotKey>,
    /// hotkey unregistered while the settings recorder is armed
    hotkey_suspended: bool,
    /// Wayland compositor/portal binding — gsettings custom keybinding,
    /// KDE GlobalShortcuts session, sway/hyprland IPC. Present only when
    /// hotkey_linux::register found a desktop mechanism.
    #[cfg(target_os = "linux")]
    hotkey_guard: Option<crate::hotkey_linux::Guard>,
    settings: Option<Settings>,
    toast: Option<Toast>,
    /// Post-capture thumbnail card — fallback where the chrome layer
    /// isn't implemented yet (Windows/Linux until their backends land).
    card: Option<QuickAccess>,
    /// Album stack surfaces — one transparent window per monitor.
    chrome: Chrome,
    /// (window, thumb index) the chrome context menu is open for.
    chrome_ctx: Option<(WindowId, usize)>,
    /// Floating always-on-top reference images.
    pins: Vec<Pin>,
    /// Recording stop card + the flag its worker watches. The window is
    /// dropped on Stop; the flag lives on until the encoder finishes.
    rec: Option<(
        crate::recstop::RecStop,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
    )>,
    /// Newer release found, waiting for the user to click Update.
    pending_update: Option<Release>,
    /// Lazy — the listener + ssh tunnel only spawn on the first Share.
    share: Option<std::sync::Arc<crate::share::ShareManager>>,
}

impl Host {
    fn new(cfg: Config, proxy: EventLoopProxy<HostEvent>) -> Self {
        let px = proxy.clone();
        let chrome = Chrome::new(
            cfg.accent_rgb(),
            &cfg.popup_pos,
            Box::new(move |wid| {
                let px = px.clone();
                std::rc::Rc::new(move |ev| {
                    let _ = px.send_event(HostEvent::Chrome(wid, ev));
                })
            }),
        );
        Self {
            cfg,
            proxy,
            overlay: None,
            frames: None,
            tray: None,
            hotkeys: None,
            hotkey: None,
            hotkey_suspended: false,
            #[cfg(target_os = "linux")]
            hotkey_guard: None,
            settings: None,
            toast: None,
            card: None,
            chrome,
            chrome_ctx: None,
            pins: Vec::new(),
            rec: None,
            pending_update: None,
            share: None,
        }
    }

    fn rebuild_menu(&self) {
        let recents = actions::recent_captures(&self.cfg.save_dir(), self.cfg.recent_max as usize);
        let update = self.pending_update.as_ref().map(|r| r.version.clone());
        #[cfg(target_os = "linux")]
        if let Some(t) = &self.tray {
            t.set_menu(self.cfg.clone(), recents, update);
        }
        #[cfg(not(target_os = "linux"))]
        if let Some(t) = &self.tray {
            t.set_menu(Some(Box::new(tray::build_menu(
                &self.cfg,
                &recents,
                update.as_deref(),
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
            if let Ok(t) = Toast::open(
                el,
                "Checking for updates…",
                self.cfg.accent_rgb(),
                &self.cfg.popup_pos,
            ) {
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
    fn install_update(&mut self, el: &ActiveEventLoop, rel: Release) {
        if self.toast.is_none() {
            let msg = format!("Updating to v{}…", rel.version);
            if let Ok(t) = Toast::open(el, &msg, self.cfg.accent_rgb(), &self.cfg.popup_pos) {
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

    /// Capture every monitor and spawn the overlay session.
    fn trigger(&mut self, el: &ActiveEventLoop) {
        eprintln!("trigger: overlay={}", self.overlay.is_some());
        if self.overlay.is_some() {
            return; // a selection is already in progress
        }
        self.card = None; // a stale card owns the previous capture's image
        let frames = match qs_capture::backend().and_then(|b| b.capture_all()) {
            Ok(f) if !f.is_empty() => f,
            Ok(_) => return eprintln!("no monitors captured"),
            Err(e) => {
                eprintln!("capture failed: {e:#}");
                self.toast_done(el, format!("Capture failed: {e:#}"), false);
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
        if id.starts_with("qs-chrome.") {
            self.on_chrome_menu(el, id);
            return;
        }
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
        // compositor/portal binding needs the same treatment — otherwise
        // the grabbed chord never reaches the settings recorder
        #[cfg(target_os = "linux")]
        if let Some(g) = &mut self.hotkey_guard {
            g.suspend(suspended);
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
        // Wayland: re-issue the compositor/portal binding for the new chord
        // (re-BindShortcuts for KDE, gsettings/swaymsg/hyprctl overwrite)
        #[cfg(target_os = "linux")]
        if let Some(g) = &mut self.hotkey_guard {
            if let Err(e) = g.rebind(&self.cfg.hotkey) {
                eprintln!("hotkey rebind failed: {e:#}");
            }
            // a Changed event while the recorder is still armed must not
            // leave the binding live
            g.suspend(self.hotkey_suspended);
        }
        self.rebuild_menu();
    }

    /// Fly the shot into the corner album; where the chrome layer can't
    /// take it (unsupported platform or a window failure) the quick-access
    /// card keeps working as the fallback completion surface.
    fn land_shot(
        &mut self,
        el: &ActiveEventLoop,
        sel: Option<qs_core::Rect>,
        img: image::RgbaImage,
        kind: ShotKind,
        saved: Option<PathBuf>,
        msg: &str,
    ) {
        self.toast = None;
        if sel.is_some() {
            let l = Land {
                img,
                kind,
                path: saved,
                sel,
                instant: false,
            };
            match self.chrome.land(el, l) {
                Ok(()) => return,
                Err(l) => {
                    self.open_card(el, l.img, msg, l.path);
                    return;
                }
            }
        }
        self.open_card(el, img, msg, saved);
    }

    /// muda context menu over a stack thumb — Reveal/Copy/Pin/Delete.
    fn chrome_menu(&mut self, id: WindowId, idx: usize, x: f64, y: f64) {
        use muda::{ContextMenu, Menu, MenuItem};
        self.chrome_ctx = Some((id, idx));
        let has_path = self
            .chrome
            .shot(id, idx)
            .is_some_and(|s| s.path.is_some());
        let menu = Menu::new();
        let _ = menu.append(&MenuItem::with_id(
            chrome::MENU_REVEAL,
            "Reveal in Finder",
            has_path,
            None,
        ));
        let _ = menu.append(&MenuItem::with_id(
            chrome::MENU_SAVE,
            "Save to Screenshots",
            !has_path,
            None,
        ));
        let _ = menu.append(&MenuItem::with_id(chrome::MENU_COPY, "Copy Image", true, None));
        let _ = menu.append(&MenuItem::with_id(chrome::MENU_PIN, "Pin", true, None));
        let _ = menu.append(&muda::PredefinedMenuItem::separator());
        let _ = menu.append(&MenuItem::with_id(chrome::MENU_DEL, "Delete", true, None));
        let _ = menu.append(&MenuItem::with_id(
            chrome::MENU_CLEAR,
            "Clear All",
            true,
            None,
        ));
        let view = self
            .chrome
            .wins
            .iter()
            .find(|w| w.id() == id)
            .map(|w| w.view_ptr());
        #[cfg(target_os = "macos")]
        if let Some(v) = view {
            unsafe {
                menu.show_context_menu_for_nsview(
                    v,
                    Some(winit::dpi::Position::Logical(
                        winit::dpi::LogicalPosition::new(x, y),
                    )),
                );
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = view;
    }

    /// Run a chrome context-menu command against the stored thumb target.
    fn on_chrome_menu(&mut self, el: &ActiveEventLoop, id: &str) {
        let Some((wid, idx)) = self.chrome_ctx else { return };
        match id {
            chrome::MENU_REVEAL => {
                if let Some(p) = self.chrome.shot(wid, idx).and_then(|s| s.path.clone()) {
                    actions::open_path(&p);
                }
            }
            chrome::MENU_COPY => {
                if let Some(img) = self.chrome.shot(wid, idx).map(|s| s.img.clone()) {
                    actions::copy_image(&img, false);
                }
            }
            chrome::MENU_SAVE => {
                let img = self.chrome.shot(wid, idx).map(|s| s.img.clone());
                if let Some(img) = img {
                    match actions::save_to_dir(&img, &self.cfg.save_dir()) {
                        Ok(p) => {
                            if let Some(w) =
                                self.chrome.wins.iter_mut().find(|w| w.id() == wid)
                            {
                                if let Some(s) = w.stack_mut().shot_mut(idx) {
                                    s.path = Some(p);
                                }
                            }
                            self.rebuild_menu();
                            self.toast_done(el, "Screenshot saved".to_string(), true);
                        }
                        Err(e) => self.toast_done(el, format!("Save failed: {e}"), false),
                    }
                }
            }
            chrome::MENU_PIN => {
                if let Some(img) = self.chrome.shot(wid, idx).map(|s| s.img.clone()) {
                    match Pin::open(el, &img) {
                        Ok(p) => self.pins.push(p),
                        Err(e) => eprintln!("pin: {e:#}"),
                    }
                }
            }
            chrome::MENU_DEL => {
                self.chrome.remove(wid, idx);
                self.chrome.sweep();
            }
            chrome::MENU_CLEAR => {
                self.chrome.clear_all(wid);
                self.chrome.sweep();
            }
            _ => {}
        }
        self.chrome_ctx = None;
    }

    /// Open the post-capture thumbnail card; falls back to the plain toast
    /// when the window can't be created.
    fn open_card(
        &mut self,
        el: &ActiveEventLoop,
        img: image::RgbaImage,
        msg: &str,
        saved: Option<PathBuf>,
    ) {
        self.toast = None;
        match QuickAccess::open(
            el,
            img,
            msg,
            saved,
            self.cfg.accent_rgb(),
            true,
            &self.cfg.popup_pos,
        ) {
            Ok(c) => self.card = Some(c),
            Err(e) => {
                eprintln!("card: {e:#}");
                self.toast_done(el, msg.to_string(), true);
            }
        }
    }

    /// A click on the card's action pills — everything runs with the image
    /// the card still owns.
    fn on_card_action(&mut self, el: &ActiveEventLoop, act: QaAction) {
        if matches!(act, QaAction::Close) {
            self.card = None;
            return;
        }
        let mut rebuild = false;
        let mut pin = false;
        if let Some(c) = &mut self.card {
            match act {
                QaAction::Copy => {
                    actions::copy_image(c.image(), false);
                    c.notify("Copied to clipboard");
                }
                QaAction::SaveNow => match actions::save_to_dir(c.image(), &self.cfg.save_dir()) {
                    Ok(p) => {
                        c.set_saved(p);
                        c.notify("Screenshot saved");
                        rebuild = true;
                    }
                    Err(e) => c.notify(&format!("Save failed: {e}")),
                },
                QaAction::Reveal => {
                    if let Some(p) = c.saved_path() {
                        actions::open_path(&p);
                    }
                }
                QaAction::Beautify => c.beautify(),
                QaAction::Pin => pin = true,
                QaAction::Share => {
                    let sm = self
                        .share
                        .get_or_insert_with(crate::share::ShareManager::new)
                        .clone();
                    let img = c.image().clone();
                    let px = self.proxy.clone();
                    c.set_sharing(true);
                    std::thread::spawn(move || {
                        let res = sm.share(&img).map_err(|e| e.to_string());
                        let _ = px.send_event(HostEvent::QuickShare(res));
                    });
                }
                QaAction::Close => unreachable!(),
            }
        }
        if rebuild {
            self.rebuild_menu();
        }
        if pin {
            if let Some(c) = &self.card {
                match Pin::open(el, c.image()) {
                    Ok(p) => self.pins.push(p),
                    Err(e) => eprintln!("pin: {e:#}"),
                }
            }
        }
    }

    /// Mirror a finished action into the toast (opened for OCR; lazily
    /// created for failures so errors are visible even for fast actions).
    fn toast_done(&mut self, el: &ActiveEventLoop, msg: String, ok: bool) {
        match &mut self.toast {
            Some(t) => t.done(msg, ok),
            None => match Toast::open(el, &msg, self.cfg.accent_rgb(), &self.cfg.popup_pos) {
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

        // On Wayland the X11 grabs below never fire — register through the
        // compositor's own mechanism (gsettings / portal / swaymsg /
        // hyprctl) and skip the manager entirely when that works.
        #[cfg(target_os = "linux")]
        let try_manager = if crate::hotkey_linux::is_wayland() {
            match crate::hotkey_linux::register(self.proxy.clone(), &self.cfg.hotkey) {
                Ok(guard) => {
                    self.hotkey_guard = guard;
                    if self.hotkey_guard.is_none() {
                        eprintln!(
                            "note: no hotkey mechanism for this Wayland desktop — \
                             bind `quickshort --capture` in your compositor config"
                        );
                    }
                    false
                }
                Err(e) => {
                    eprintln!("hotkey: compositor registration failed: {e:#} — \
                               bind `quickshort --capture` in your compositor config");
                    true
                }
            }
        } else {
            true
        };
        #[cfg(not(target_os = "linux"))]
        let try_manager = true;

        let recents = actions::recent_captures(&self.cfg.save_dir(), self.cfg.recent_max as usize);
        // rehydrate the album from disk — recents are newest-first, so
        // land the tail five oldest→newest to end with the newest on top;
        // `instant` skips the fly-in
        #[cfg(target_os = "macos")]
        for p in recents.iter().take(5).rev() {
            if let Ok(img) = image::open(p) {
                let _ = self.chrome.land(
                    _el,
                    Land {
                        img: img.to_rgba8(),
                        kind: ShotKind::Saved,
                        path: Some(p.clone()),
                        sel: None,
                        instant: true,
                    },
                );
            }
        }
        #[cfg(target_os = "linux")]
        match tray::linux_tray::spawn(&self.cfg, &recents) {
            Ok(t) => self.tray = Some(t),
            Err(e) => eprintln!("tray icon failed: {e:#}"),
        }
        #[cfg(not(target_os = "linux"))]
        {
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
        }

        if try_manager {
            match self.cfg.hotkey.parse::<HotKey>() {
                Ok(hk) => {
                    match GlobalHotKeyManager::new().and_then(|m| {
                        m.register(hk)?;
                        Ok(m)
                    }) {
                        Ok(m) => {
                            self.hotkey = Some(hk);
                            self.hotkeys = Some(m);
                        }
                        Err(e) => {
                            eprintln!("hotkey register failed: {e:#}");
                            // the configured chord may be shell-owned (Windows
                            // claims PrintScreen for Snipping on fresh installs)
                            // — fall back to a chord that is always free
                            const FALLBACK: &str = "Ctrl+Shift+S";
                            if self.cfg.hotkey != FALLBACK {
                                if let Ok(fbk) = FALLBACK.parse::<HotKey>() {
                                    match GlobalHotKeyManager::new().and_then(|m| {
                                        m.register(fbk)?;
                                        Ok(m)
                                    }) {
                                        Ok(m) => {
                                            eprintln!("hotkey fell back to {FALLBACK}");
                                            self.hotkey = Some(fbk);
                                            self.hotkeys = Some(m);
                                        }
                                        Err(e) => eprintln!("fallback register failed: {e:#}"),
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => eprintln!("bad hotkey {:?}: {e}", self.cfg.hotkey),
            }
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
            HostEvent::Trigger => {
                // in-process hotkey sources (KDE portal signal thread)
                // must honor the recorder suspension the same way an
                // unregistered X11 grab does
                if !self.hotkey_suspended {
                    self.trigger(el);
                }
            }
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
                    let _ = update::relaunch(&app);
                    el.exit();
                }
                Err(e) => self.toast_done(el, format!("Update failed: {e}"), false),
            },
            HostEvent::Done(res) => {
                eprintln!("done: {:?}", res.as_ref().map(|d| &d.outcome));
                match res {
                    // image outcomes fly to the album stack; text-only
                    // feedback still rides the toast
                    Ok(d) => match d.outcome {
                        Outcome::Saved(p) => {
                            self.rebuild_menu();
                            let name = p
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            self.land_shot(
                                el,
                                Some(d.sel),
                                d.img,
                                ShotKind::Saved,
                                Some(p),
                                &format!("Saved {name}"),
                            );
                        }
                        Outcome::Copied => self.land_shot(
                            el,
                            Some(d.sel),
                            d.img,
                            ShotKind::Copied,
                            None,
                            "Copied to clipboard",
                        ),
                        Outcome::Shared(link) => {
                            self.land_shot(
                                el,
                                Some(d.sel),
                                d.img,
                                ShotKind::Shared,
                                None,
                                "Shared",
                            );
                            self.toast_done(
                                el,
                                format!("Link copied — expires in {}d", link.expires_s / 86400),
                                true,
                            );
                        }
                        Outcome::TextCopied(n) => {
                            self.land_shot(
                                el,
                                Some(d.sel),
                                d.img,
                                ShotKind::Ocr,
                                None,
                                &format!("Copied {n} chars"),
                            );
                            self.toast_done(el, format!("Copied {n} chars"), true);
                        }
                        Outcome::NoText => {
                            self.land_shot(
                                el,
                                Some(d.sel),
                                d.img,
                                ShotKind::Ocr,
                                None,
                                "No text found",
                            );
                            self.toast_done(el, "No text found".to_string(), false);
                        }
                    },
                    Err(e) => self.toast_done(el, format!("Failed: {e}"), false),
                }
            }
            HostEvent::ScrollDone(res, sel) => match res {
                Ok(img) => self.land_shot(
                    el,
                    Some(sel),
                    img,
                    ShotKind::Scroll,
                    None,
                    "Captured — pick an action",
                ),
                Err(e) => self.toast_done(el, format!("Scroll failed: {e}"), false),
            },
            HostEvent::RecDone(res, sel) => {
                self.rec = None;
                match res {
                    Ok((img, path)) => self.land_shot(
                        el,
                        Some(sel),
                        img,
                        ShotKind::Record,
                        Some(path),
                        "Recording saved",
                    ),
                    Err(e) => self.toast_done(el, format!("Recording failed: {e}"), false),
                }
            }
            HostEvent::QuickShare(res) => {
                let Some(c) = &mut self.card else { return };
                c.set_sharing(false);
                match res {
                    Ok(link) => {
                        actions::copy_text(&link.url, false);
                        c.notify(&format!(
                            "Link copied — expires in {}d",
                            link.expires_s / 86400
                        ));
                    }
                    Err(e) => c.notify(&format!("Share failed: {e}")),
                }
            }
            HostEvent::Chrome(id, ev) => {
                let Some((wid, cmd)) = self.chrome.event(id, ev) else {
                    return;
                };
                match cmd {
                    ChromeCmd::Menu { idx, x, y } => self.chrome_menu(wid, idx, x, y),
                    ChromeCmd::Remove(i) => {
                        self.chrome.remove(wid, i);
                        self.chrome.sweep();
                    }
                    ChromeCmd::ClearAll => {
                        self.chrome.clear_all(wid);
                        self.chrome.sweep();
                    }
                    ChromeCmd::Redraw => {} // handled inside Chrome::event
                }
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.chrome.window_event(id, &event) {
            return;
        }
        if let Some(i) = self.pins.iter().position(|p| p.win_id() == id) {
            if self.pins[i].window_event(event) {
                self.pins.remove(i);
            }
            return;
        }
        if let Some(c) = &mut self.card {
            if c.win_id() == id {
                if let Some(act) = c.window_event(event) {
                    self.on_card_action(el, act);
                }
                return;
            }
        }
        if let Some(t) = &mut self.toast {
            if t.win_id() == id {
                if t.window_event(event) {
                    self.toast = None;
                }
                return;
            }
        }
        if let Some((r, stop)) = &mut self.rec {
            if r.win_id() == id {
                if r.window_event(event) {
                    stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    self.rec = None; // encoder finalizes → RecDone follows
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
            // scrolling capture drives its own capture loop and reports
            // the stitched image through ScrollDone
            if action == qs_overlay::Action::Scroll {
                match Toast::open(
                    el,
                    "Scrolling capture — scroll the page; it ends when it stops",
                    self.cfg.accent_rgb(),
                    &self.cfg.popup_pos,
                ) {
                    Ok(t) => self.toast = Some(t),
                    Err(e) => eprintln!("toast: {e:#}"),
                }
                let px = self.proxy.clone();
                std::thread::spawn(move || {
                    let res = crate::scroll::run(sel).map_err(|e| e.to_string());
                    let _ = px.send_event(HostEvent::ScrollDone(res, sel));
                });
                return;
            }
            // recording keeps its own stream open and stops via the
            // floating RecStop card — RecDone carries (thumb, path)
            if action == qs_overlay::Action::Record {
                let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                // without the stop card there'd be no way to end the
                // recording — refuse to start instead of capturing forever
                let r = match crate::recstop::RecStop::open(el, &self.cfg.popup_pos) {
                    Ok(r) => r,
                    Err(e) => {
                        self.toast_done(el, format!("Recording failed: {e:#}"), false);
                        return;
                    }
                };
                self.rec = Some((r, stop.clone()));
                let px = self.proxy.clone();
                let dir = self.cfg.save_dir();
                let stop2 = stop.clone();
                std::thread::spawn(move || {
                    let res = crate::record::run(sel, &dir, stop2).map_err(|e| e.to_string());
                    let _ = px.send_event(HostEvent::RecDone(res, sel));
                });
                return;
            }
            // slow actions get a live toast while the worker runs
            if self.toast.is_none() {
                let wait_msg = match action {
                    qs_overlay::Action::Ocr => Some("Extracting text…"),
                    qs_overlay::Action::Share => Some("Publishing link…"),
                    _ => None,
                };
                if let Some(m) = wait_msg {
                    match Toast::open(el, m, self.cfg.accent_rgb(), &self.cfg.popup_pos) {
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
        // earliest wake wins — the settings poll (pack install) must not
        // be starved by a longer toast timeout, and vice versa
        let mut wake: Option<std::time::Instant> = None;
        if let Some(s) = &mut self.settings {
            s.about_to_wait();
            if s.poll_needed() {
                wake = Some(std::time::Instant::now() + std::time::Duration::from_millis(250));
            }
        }
        if let Some(t) = &mut self.toast {
            match t.pump() {
                Some(w) => wake = Some(wake.map_or(w, |x| x.min(w))),
                None => self.toast = None,
            }
        }
        if let Some(c) = &mut self.card {
            match c.pump() {
                Some(w) => wake = Some(wake.map_or(w, |x| x.min(w))),
                None => self.card = None,
            }
        }
        if let Some((r, _)) = &mut self.rec {
            let w = r.pump();
            wake = Some(wake.map_or(w, |x| x.min(w)));
        }
        if let Some(w) = self.chrome.pump() {
            wake = Some(wake.map_or(w, |x| x.min(w)));
        }
        el.set_control_flow(match wake {
            Some(w) => ControlFlow::WaitUntil(w),
            None => ControlFlow::Wait,
        });
    }
}
