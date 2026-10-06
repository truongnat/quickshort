//! One-shot capture flow. winit allows exactly one event loop per
//! process, so the overlay, the recording stop card, the scroll toast
//! and the result card all live on this single loop — a slim `Host`
//! without tray/hotkeys. `quickshort --capture` runs this.

use anyhow::Result;
use image::RgbaImage;
use qs_core::{CapturedFrame, Rect};
use qs_overlay::{Action, Annotation, Overlay, OverlayEvent};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::WindowId;

use crate::actions::{self, Dispatch, Outcome, SaveTarget};
use crate::config::Config;
use crate::quick_access::{CardEnv, CardPost, QaAction, QuickAccess};
use crate::recstop::RecStop;
use crate::toast::Toast;

enum Ev {
    Done(Result<Dispatch, String>),
    ScrollDone(Result<RgbaImage, String>),
    RecDone(Result<(RgbaImage, PathBuf), String>),
    ShareDone(Result<crate::share::SharedLink, String>),
}

struct Run {
    proxy: EventLoopProxy<Ev>,
    save_dir: PathBuf,
    accent: u32,
    target: SaveTarget,
    copy_after_save: bool,
    ocr_lang: String,
    // capture-session inputs
    overlay: Option<Overlay>,
    frames: Option<Vec<CapturedFrame>>,
    bounds: Rect,
    prefs: qs_overlay::Prefs,
    /// held until the overlay closes so a second --capture can't stack
    lock: Option<std::fs::File>,
    // live windows
    card: Option<QuickAccess>,
    /// the overlay's selection — the card docks on its monitor
    sel: Option<Rect>,
    rec: Option<RecStop>,
    toast: Option<Toast>,
    stop: Arc<AtomicBool>,
    /// dispatch/scroll/record workers still running
    workers: u8,
    sm: Arc<crate::share::ShareManager>,
}

impl ApplicationHandler<Ev> for Run {
    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, ev: WindowEvent) {
        if let Some(ov) = &mut self.overlay {
            if let Some(out) = ov.window_event(id, ev) {
                self.overlay = None; // dropping destroys the overlay windows
                self.lock = None; // selection done — a new --capture may start
                match out {
                    OverlayEvent::Done(sel, action, annots) => {
                        self.start_action(el, sel, action, annots)
                    }
                    OverlayEvent::Cancelled => self.frames = None,
                }
            }
            return;
        }
        if let Some(c) = &mut self.card {
            if c.win_id() == id {
                if let Some(act) = c.window_event(ev) {
                    self.on_card_action(act);
                }
                return;
            }
        }
        if let Some(r) = &mut self.rec {
            if r.win_id() == id {
                if r.window_event(ev) {
                    self.stop.store(true, Ordering::Relaxed);
                    self.rec = None;
                    // cover the encoder's finalize gap until RecDone
                    if let Ok(t) = Toast::open(el, "Saving recording…", self.accent) {
                        self.toast = Some(t);
                    }
                }
                return;
            }
        }
        if let Some(t) = &mut self.toast {
            if t.win_id() == id && t.window_event(ev) {
                self.toast = None;
            }
        }
    }

    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.overlay.is_some() || self.frames.is_none() {
            return;
        }
        let res = match self.frames.as_ref() {
            Some(f) => Overlay::spawn(el, f, self.bounds, &self.prefs),
            None => return,
        };
        match res {
            Ok(ov) => self.overlay = Some(ov),
            Err(e) => {
                eprintln!("overlay failed: {e:#}");
                el.exit();
            }
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, ev: Ev) {
        match ev {
            Ev::Done(res) => {
                self.workers -= 1;
                match res {
                    Ok(d) => self.on_done(el, d),
                    Err(e) => crate::notify(&e),
                }
            }
            Ev::ScrollDone(res) => {
                self.workers -= 1;
                self.toast = None;
                match res {
                    Ok(img) => self.open_card(el, img, "Captured — pick an action", None),
                    Err(e) => crate::notify(&format!("Scroll failed: {e}")),
                }
            }
            Ev::RecDone(res) => {
                self.workers -= 1;
                self.rec = None;
                match res {
                    Ok((img, path)) => self.open_card(el, img, "Recording saved", Some(path)),
                    Err(e) => crate::notify(&format!("Recording failed: {e}")),
                }
            }
            Ev::ShareDone(res) => {
                if let Some(c) = &mut self.card {
                    crate::quick_access::share_finished(c, res, true);
                }
            }
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if let Some(ov) = &mut self.overlay {
            ov.about_to_wait();
        }
        let mut wake: Option<Instant> = None;
        if let Some(c) = &mut self.card {
            match c.pump() {
                Some(w) => wake = Some(w),
                None => self.card = None,
            }
        }
        if let Some(r) = &mut self.rec {
            let w = r.pump();
            wake = Some(wake.map_or(w, |x| x.min(w)));
        }
        if let Some(t) = &mut self.toast {
            match t.pump() {
                Some(w) => wake = Some(wake.map_or(w, |x| x.min(w))),
                None => self.toast = None,
            }
        }
        // the session is over once nothing is on screen and no worker
        // is still out — card countdown, esc-cancel and plain outcomes
        // (Shared/Ocr) all funnel through here. `frames` stays Some
        // until the overlay consumed them, so it doubles as the
        // "has the session started yet" guard.
        if self.frames.is_none()
            && self.overlay.is_none()
            && self.card.is_none()
            && self.rec.is_none()
            && self.toast.is_none()
            && self.workers == 0
        {
            el.exit();
            return;
        }
        el.set_control_flow(match wake {
            Some(w) => ControlFlow::WaitUntil(w),
            None => ControlFlow::Wait,
        });
    }
}

impl Run {
    fn start_action(&mut self, el: &ActiveEventLoop, sel: Rect, action: Action, annots: Vec<Annotation>) {
        self.sel = Some(sel);
        // frames are only needed by dispatch — drop them either way so
        // the exit check can fire and the memory is freed early
        let frames = self.frames.take();
        match action {
            Action::Scroll => {
                match Toast::open(
                    el,
                    "Scrolling capture — scroll the page; it ends when it stops",
                    self.accent,
                ) {
                    Ok(t) => self.toast = Some(t),
                    Err(e) => eprintln!("toast: {e:#}"),
                }
                let px = self.proxy.clone();
                self.workers += 1;
                std::thread::spawn(move || {
                    let res = crate::scroll::run(sel).map_err(|e| e.to_string());
                    let _ = px.send_event(Ev::ScrollDone(res));
                });
            }
            Action::Record => {
                match RecStop::open(el, Some(sel)) {
                    Ok(r) => self.rec = Some(r),
                    Err(e) => eprintln!("rec: {e:#}"),
                }
                let px = self.proxy.clone();
                let dir = self.save_dir.clone();
                let stop = self.stop.clone();
                self.workers += 1;
                std::thread::spawn(move || {
                    let res = crate::record::run(sel, &dir, stop).map_err(|e| e.to_string());
                    let _ = px.send_event(Ev::RecDone(res));
                });
            }
            _ => {
                let px = self.proxy.clone();
                let Some(frames) = frames else { return };
                let target = self.target.clone();
                let share = (action == Action::Share).then(|| self.sm.clone());
                let (copy, lang) = (self.copy_after_save, self.ocr_lang.clone());
                self.workers += 1;
                std::thread::spawn(move || {
                    let res = actions::dispatch(
                        frames, sel, action, &annots, &target, true, share, copy, &lang,
                    )
                    .map_err(|e| e.to_string());
                    let _ = px.send_event(Ev::Done(res));
                });
            }
        }
    }

    fn on_done(&mut self, el: &ActiveEventLoop, d: Dispatch) {
        let (msg, saved) = match &d.outcome {
            Outcome::Saved(p) => (
                format!(
                    "Saved {}",
                    p.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ),
                Some(p.clone()),
            ),
            Outcome::Copied => ("Copied to clipboard".to_string(), None),
            Outcome::Shared(l) => {
                crate::notify(&format!(
                    "shared — link copied (expires in {}d)\n{}",
                    l.expires_s / 86400,
                    l.url
                ));
                return;
            }
            Outcome::TextCopied(n) => {
                crate::notify(&format!("copied {n} chars"));
                return;
            }
            Outcome::NoText => {
                crate::notify("No text found");
                return;
            }
        };
        self.open_card(el, d.img, &msg, saved);
    }

    fn open_card(&mut self, el: &ActiveEventLoop, img: RgbaImage, msg: &str, saved: Option<PathBuf>) {
        self.toast = None; // result card replaces any in-flight toast
        match QuickAccess::open(el, img, msg, saved, self.accent, false, self.sel) {
            Ok(c) => self.card = Some(c),
            Err(e) => {
                eprintln!("card: {e:#}");
                crate::notify(msg);
            }
        }
    }

    /// Card pill clicks — blocking clipboard variant, no Pin in one-shot
    /// (that pill isn't rendered without the resident app).
    fn on_card_action(&mut self, act: QaAction) {
        let post = self.card.as_mut().and_then(|c| {
            CardEnv {
                save_dir: &self.save_dir,
                copy_wait: true,
                share: &self.sm,
                proxy: &self.proxy,
                share_done: Ev::ShareDone,
            }
            .action(c, act)
        });
        if post == Some(CardPost::Closed) {
            self.card = None;
        }
    }
}

/// `quickshort --capture` — overlay, action workers and result card on
/// the process's single event loop. `lock` (the capture singleton file)
/// is held until the overlay closes so a second --capture can't stack.
pub fn run(
    frames: Vec<CapturedFrame>,
    bounds: Rect,
    target: SaveTarget,
    cfg: &Config,
    lock: Option<std::fs::File>,
) -> Result<()> {
    let el = EventLoop::<Ev>::with_user_event().build()?;
    let mut run = Run {
        proxy: el.create_proxy(),
        save_dir: cfg.save_dir(),
        accent: cfg.accent_rgb(),
        target,
        copy_after_save: cfg.copy_after_save,
        ocr_lang: cfg.ocr_lang.clone(),
        overlay: None,
        frames: Some(frames),
        bounds,
        prefs: cfg.overlay_prefs(),
        lock,
        card: None,
        sel: None,
        rec: None,
        toast: None,
        stop: Arc::new(AtomicBool::new(false)),
        workers: 0,
        sm: crate::share::ShareManager::new(),
    };
    el.run_app(&mut run)?;
    Ok(())
}
