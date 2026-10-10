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
use crate::quick_access::{QaAction, QuickAccess};
use crate::recstop::RecStop;
use crate::toast::Toast;

enum Ev {
    Done(Result<Dispatch, String>),
    ScrollDone(Result<RgbaImage, String>),
    RecDone(Result<(RgbaImage, PathBuf, u64), String>),
    ShareDone(Result<crate::share::SharedLink, String>),
}

struct Run {
    proxy: EventLoopProxy<Ev>,
    save_dir: PathBuf,
    accent: u32,
    popup_pos: String,
    target: SaveTarget,
    copy_after_save: bool,
    ocr_lang: String,
    // capture-session inputs
    overlay: Option<Overlay>,
    frames: Option<Vec<CapturedFrame>>,
    bounds: Option<Rect>,
    prefs: qs_overlay::Prefs,
    /// held until the overlay closes so a second --capture can't stack
    lock: Option<std::fs::File>,
    // live windows
    card: Option<QuickAccess>,
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
        let res = match self.frames.as_ref().zip(self.bounds) {
            Some((f, b)) => Overlay::spawn(el, f, b, &self.prefs),
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
                    Err(e) => self.notify(el, &e, false),
                }
            }
            Ev::ScrollDone(res) => {
                self.workers -= 1;
                self.toast = None;
                match res {
                    Ok(img) => self.open_card(el, img, "Captured — pick an action", None),
                    Err(e) => self.notify(el, &format!("Scroll failed: {e}"), false),
                }
            }
            Ev::RecDone(res) => {
                self.workers -= 1;
                self.rec = None;
                match res {
                    Ok((img, path, _dur)) => self.open_card(el, img, "Recording saved", Some(path)),
                    Err(e) => self.notify(el, &format!("Recording failed: {e}"), false),
                }
            }
            Ev::ShareDone(res) => {
                let Some(c) = &mut self.card else { return };
                c.set_sharing(false);
                match res {
                    Ok(l) => {
                        actions::copy_text(&l.url, true);
                        c.notify(&format!(
                            "Link copied — expires in {}d",
                            l.expires_s / 86400
                        ));
                    }
                    Err(e) => c.notify(&format!("Share failed: {e}")),
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
        // (Shared/Ocr) all funnel through here
        if self.frames_consumed()
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
    /// `frames` is Some until the overlay consumed them into a dispatch
    /// worker — before that, the exit check must wait for `resumed`.
    fn frames_consumed(&self) -> bool {
        self.frames.is_none()
    }

    fn start_action(
        &mut self,
        el: &ActiveEventLoop,
        sel: Rect,
        action: Action,
        annots: Vec<Annotation>,
    ) {
        // frames are only needed by dispatch — drop them either way so
        // the exit check can fire and the memory is freed early
        let frames = self.frames.take();
        match action {
            Action::Scroll => {
                match Toast::open(
                    el,
                    "Scrolling capture — scroll the page; it ends when it stops",
                    self.accent,
                    &self.popup_pos,
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
                // without the stop card there'd be no way to end the
                // recording — refuse to start instead of capturing forever
                let r = match RecStop::open(el, &self.popup_pos) {
                    Ok(r) => r,
                    Err(e) => {
                        self.notify(el, &format!("Recording failed: {e:#}"), false);
                        return;
                    }
                };
                self.rec = Some(r);
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

    /// Errors and plain-text outcomes land on a toast — one-shot mode
    /// has no tray, and an eprintln dies unseen in a GUI session.
    fn notify(&mut self, el: &ActiveEventLoop, msg: &str, ok: bool) {
        match &mut self.toast {
            Some(t) => t.done(msg.to_string(), ok),
            None => match Toast::open(el, msg, self.accent, &self.popup_pos) {
                Ok(mut t) => {
                    t.done(msg.to_string(), ok);
                    self.toast = Some(t);
                }
                Err(e) => eprintln!("toast: {e:#}"),
            },
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
                self.notify(
                    el,
                    &format!(
                        "shared — link copied (expires in {}d)\n{}",
                        l.expires_s / 86400,
                        l.url
                    ),
                    true,
                );
                return;
            }
            Outcome::TextCopied(n) => {
                self.notify(el, &format!("copied {n} chars"), true);
                return;
            }
            Outcome::NoText => {
                self.notify(el, "No text found", false);
                return;
            }
        };
        self.open_card(el, d.img, &msg, saved);
    }

    fn open_card(
        &mut self,
        el: &ActiveEventLoop,
        img: RgbaImage,
        msg: &str,
        saved: Option<PathBuf>,
    ) {
        match QuickAccess::open(el, img, msg, saved, self.accent, false, &self.popup_pos) {
            Ok(c) => self.card = Some(c),
            Err(e) => {
                eprintln!("card: {e:#}");
                self.notify(el, msg, true);
            }
        }
    }

    /// Card pill clicks — same semantics as `quick_access::standalone`
    /// (blocking clipboard variant, no Pin in one-shot).
    fn on_card_action(&mut self, act: QaAction) {
        if matches!(act, QaAction::Close) {
            self.card = None;
            return;
        }
        let Some(c) = &mut self.card else { return };
        match act {
            QaAction::Copy => {
                actions::copy_image(c.image(), true);
                c.notify("Copied to clipboard");
            }
            QaAction::SaveNow => match actions::save_to_dir(c.image(), &self.save_dir) {
                Ok(p) => {
                    c.set_saved(p);
                    c.notify("Screenshot saved");
                }
                Err(e) => c.notify(&format!("Save failed: {e}")),
            },
            QaAction::Reveal => {
                if let Some(p) = c.saved_path() {
                    actions::open_path(&p);
                }
            }
            QaAction::Beautify => c.beautify(),
            QaAction::Pin => unreachable!(),
            QaAction::Share => {
                let (sm, img, px) = (self.sm.clone(), c.image().clone(), self.proxy.clone());
                c.set_sharing(true);
                std::thread::spawn(move || {
                    let res = sm.share(&img).map_err(|e| e.to_string());
                    let _ = px.send_event(Ev::ShareDone(res));
                });
            }
            QaAction::Close => unreachable!(),
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
        popup_pos: cfg.popup_pos.clone(),
        target,
        copy_after_save: cfg.copy_after_save,
        ocr_lang: cfg.ocr_lang.clone(),
        overlay: None,
        frames: Some(frames),
        bounds: Some(bounds),
        prefs: cfg.overlay_prefs(),
        lock,
        card: None,
        rec: None,
        toast: None,
        stop: Arc::new(AtomicBool::new(false)),
        workers: 0,
        sm: crate::share::ShareManager::new(),
    };
    el.run_app(&mut run)?;
    Ok(())
}
