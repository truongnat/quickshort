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

use crate::actions::{self, SaveTarget};
use crate::config::Config;
use crate::tray;

/// Everything that wakes the loop besides window events.
enum HostEvent {
    /// Tray "Capture Region" or the global hotkey.
    Trigger,
    Menu(MenuId),
    /// A save worker finished — refresh the recents submenu.
    Saved(PathBuf),
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
    _hotkeys: Option<GlobalHotKeyManager>,
}

impl Host {
    fn new(cfg: Config, proxy: EventLoopProxy<HostEvent>) -> Self {
        Self {
            cfg,
            proxy,
            overlay: None,
            frames: None,
            tray: None,
            _hotkeys: None,
        }
    }

    fn rebuild_menu(&self) {
        if let Some(t) = &self.tray {
            let recents = actions::recent_captures(&self.cfg.save_dir(), tray::RECENT_MAX);
            t.set_menu(Some(Box::new(tray::build_menu(&self.cfg, &recents))));
        }
    }

    /// Capture every monitor and spawn the overlay session.
    fn trigger(&mut self, el: &ActiveEventLoop) {
        if self.overlay.is_some() {
            return; // a selection is already in progress
        }
        let frames = match qs_capture::backend().and_then(|b| b.capture_all()) {
            Ok(f) if !f.is_empty() => f,
            Ok(_) => return eprintln!("no monitors captured"),
            Err(e) => return eprintln!("capture failed: {e:#}"),
        };
        let bounds = qs_core::virtual_bounds(&frames);
        match Overlay::spawn(el, &frames, bounds) {
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

        let recents = actions::recent_captures(&self.cfg.save_dir(), tray::RECENT_MAX);
        let builder = tray_icon::TrayIcon::builder()
            .with_tooltip("quickshort — region screenshot")
            .with_menu(Box::new(tray::build_menu(&self.cfg, &recents)));
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
                Ok(m) => self._hotkeys = Some(m),
                Err(e) => eprintln!("hotkey register failed: {e:#}"),
            },
            Err(e) => eprintln!("bad hotkey {:?}: {e}", self.cfg.hotkey),
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, ev: HostEvent) {
        match ev {
            HostEvent::Trigger => self.trigger(el),
            HostEvent::Menu(id) => self.on_menu(el, id),
            HostEvent::Saved(_p) => self.rebuild_menu(),
        }
    }

    fn window_event(&mut self, _el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(ov) = &mut self.overlay else { return };
        let Some(out) = ov.window_event(id, event) else {
            return;
        };
        self.overlay = None; // dropping destroys the overlay windows
        let Some(frames) = self.frames.take() else {
            return;
        };
        if let OverlayEvent::Done(sel, action, annots) = out {
            // stitch + encode off the UI thread; Saved updates the menu later
            let px = self.proxy.clone();
            let target = SaveTarget::Dir(self.cfg.save_dir());
            std::thread::spawn(move || {
                if let Ok(Some(p)) = actions::dispatch(frames, sel, action, &annots, &target, false)
                {
                    let _ = px.send_event(HostEvent::Saved(p));
                }
            });
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(ov) = &mut self.overlay {
            ov.about_to_wait();
        }
    }
}
