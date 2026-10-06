//! Quick-access card — a small always-on-top thumbnail popup shown after a
//! resident capture completes. Gives one-click follow-up actions (Copy /
//! Save / Share) without reopening the overlay; auto-dismisses on a
//! countdown that resets whenever the user interacts with it.

use anyhow::{Context, Result};
use fontdue::Font;
use image::RgbaImage;
use qs_overlay::sdf::{fill_cov, rrect_sdf, Canvas};
use softbuffer::{Context as SbContext, Surface};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

use crate::settings::system_font;
use crate::widgets::{card_chrome, monitor_of, pill, shade, Txt};

const W: f64 = 424.0;
const H: f64 = 108.0;
const TTL: Duration = Duration::from_secs(8);
const THUMB: usize = 80;
/// Dock margin from the monitor's bottom-right edge, logical px.
const EDGE: f64 = 20.0;
const SLIDE_IN: f32 = 0.20;
const SLIDE_OUT: f32 = 0.16;
const SLIDE_PX: f64 = 70.0;
const FRAME: Duration = Duration::from_millis(16);

const CARD: u32 = 0xFFFFFF;
const BORDER: u32 = 0xCBD0D8;
const FRINGE: u32 = 0x1A1A1F;
const FG: u32 = 0x24272D;
const BTN_BG: u32 = 0xF2F4F7;
const BTN_HOT: u32 = 0xE4E8EE;

/// What the host should do in response to a card click.
#[derive(Clone, Copy)]
pub enum QaAction {
    Copy,
    /// card shows Save (image only exists in memory)
    SaveNow,
    /// card shows Open (image is already on disk)
    Reveal,
    Share,
    /// pad + gradient + shadow — mutates the image the card holds
    Beautify,
    /// float the image in an always-on-top window (resident app only)
    Pin,
    Close,
}

enum Btn {
    Copy,
    SaveOrReveal,
    Share,
    Beautify,
    Pin,
}

impl Btn {
    fn rect(&self) -> (f64, f64, f64, f64) {
        match self {
            Btn::Copy => (104.0, 66.0, 56.0, 26.0),
            Btn::SaveOrReveal => (168.0, 66.0, 56.0, 26.0),
            Btn::Share => (232.0, 66.0, 56.0, 26.0),
            Btn::Beautify => (296.0, 66.0, 68.0, 26.0),
            Btn::Pin => (372.0, 66.0, 44.0, 26.0),
        }
    }
    fn label(&self, saved: bool) -> &'static str {
        match self {
            Btn::Copy => "Copy",
            Btn::SaveOrReveal => {
                if saved {
                    "Open"
                } else {
                    "Save"
                }
            }
            Btn::Share => "Share",
            Btn::Beautify => "Beautify",
            Btn::Pin => "Pin",
        }
    }
    fn action(&self, saved: bool) -> QaAction {
        match self {
            Btn::Copy => QaAction::Copy,
            Btn::SaveOrReveal => {
                if saved {
                    QaAction::Reveal
                } else {
                    QaAction::SaveNow
                }
            }
            Btn::Share => QaAction::Share,
            Btn::Beautify => QaAction::Beautify,
            Btn::Pin => QaAction::Pin,
        }
    }
}

/// Everything a card host must supply so the pill actions can run:
/// where SaveNow writes, whether the clipboard path must block (one-shot
/// processes exit right after), and how a finished share maps into the
/// app's user-event type.
pub struct CardEnv<'a, E: 'static> {
    pub save_dir: &'a std::path::Path,
    pub copy_wait: bool,
    pub share: &'a std::sync::Arc<crate::share::ShareManager>,
    pub proxy: &'a winit::event_loop::EventLoopProxy<E>,
    /// HostEvent::QuickShare / oneshot Ev::ShareDone — the variant the
    /// share worker reports through.
    pub share_done: fn(Result<crate::share::SharedLink, String>) -> E,
}

/// Follow-ups the shared handler can't perform itself: `Pin` needs the
/// app's event loop, `Saved` refreshes the tray's recent menu, `Closed`
/// drops the card.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CardPost {
    Saved,
    Pin,
    Closed,
}

impl<E: Send + 'static> CardEnv<'_, E> {
    /// One pill click — every action works on the image the card owns.
    /// Single implementation shared by host, one-shot and standalone.
    pub fn action(&self, card: &mut QuickAccess, act: QaAction) -> Option<CardPost> {
        match act {
            QaAction::Close => return Some(CardPost::Closed),
            QaAction::Copy => {
                crate::actions::copy_image(card.image(), self.copy_wait);
                card.notify("Copied to clipboard");
            }
            QaAction::SaveNow => match crate::actions::save_to_dir(card.image(), self.save_dir) {
                Ok(p) => {
                    card.set_saved(p);
                    card.notify("Screenshot saved");
                    return Some(CardPost::Saved);
                }
                Err(e) => card.notify(&format!("Save failed: {e}")),
            },
            QaAction::Reveal => {
                if let Some(p) = card.saved_path() {
                    crate::actions::open_path(&p);
                }
            }
            QaAction::Beautify => card.beautify(),
            QaAction::Pin => return Some(CardPost::Pin),
            QaAction::Share => {
                let (sm, img, px, map) = (
                    self.share.clone(),
                    card.image().clone(),
                    self.proxy.clone(),
                    self.share_done,
                );
                card.set_sharing(true);
                std::thread::spawn(move || {
                    let res = sm.share(&img).map_err(|e| e.to_string());
                    let _ = px.send_event(map(res));
                });
            }
        }
        None
    }
}

/// Share worker reported — restore the pill and hand the URL to the
/// clipboard. Shared by every host's ShareDone event arm.
pub fn share_finished(
    card: &mut QuickAccess,
    res: Result<crate::share::SharedLink, String>,
    copy_wait: bool,
) {
    card.set_sharing(false);
    match res {
        Ok(l) => {
            crate::actions::copy_text(&l.url, copy_wait);
            card.notify(&format!("Link copied — expires in {}d", l.expires_s / 86400));
        }
        Err(e) => card.notify(&format!("Share failed: {e}")),
    }
}

const BTNS_PIN: &[Btn] = &[
    Btn::Copy,
    Btn::SaveOrReveal,
    Btn::Share,
    Btn::Beautify,
    Btn::Pin,
];
/// one-shot mode: no Pin — the process exits and the window would die
const BTNS: &[Btn] = &[Btn::Copy, Btn::SaveOrReveal, Btn::Share, Btn::Beautify];
const CLOSE: (f64, f64, f64) = (W - 26.0, 14.0, 9.0); // cx, cy, r

/// Everything `render` needs — kept apart from the window/surface so
/// `draw` can borrow `surf` mutably while the state stays immutable.
struct CardState {
    font: Font,
    thumb: Vec<u32>,
    msg: String,
    accent: u32,
    saved: Option<PathBuf>,
    /// a share upload is in flight — Share stays dimmed
    sharing: bool,
    deadline: Instant,
    /// which pills to show — Pin only exists where the host outlives it
    btns: &'static [Btn],
    hover: Option<usize>, // index into btns; 99 = close button
    /// countdown paused while the cursor rests on the card
    pause_left: Option<Duration>,
}

/// Where the card is in its docked lifecycle — In/Live/Out drives the
/// slide animation so it reads as a screen-edge popup, not a floater.
enum Phase {
    In(Instant),
    Live,
    /// `from` = y offset at close time, so mid-slide-in dismissals stay
    /// continuous instead of snapping to the dock first.
    Out { t0: Instant, from: f64 },
}

pub struct QuickAccess {
    win: Rc<Window>,
    surf: Surface<Rc<Window>, Rc<Window>>,
    img: RgbaImage,
    st: CardState,
    phase: Phase,
    /// resting position in physical px — slide animations offset from it
    dock: (f64, f64),
    /// current downward offset from the dock, physical px
    y_off: f64,
}

impl QuickAccess {
    /// Card docked bottom-right of the monitor holding `anchor` (the
    /// selection; `None` → primary). `pin` enables the Pin pill — only
    /// the resident host can keep a pinned window alive.
    pub fn open(
        el: &ActiveEventLoop,
        img: RgbaImage,
        msg: &str,
        saved: Option<PathBuf>,
        accent: u32,
        pin: bool,
        anchor: Option<qs_core::Rect>,
    ) -> Result<Self> {
        let attrs = Window::default_attributes()
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(LogicalSize::new(W, H));
        let win = Rc::new(el.create_window(attrs).context("card window")?);
        let mut dock = (0.0, 0.0);
        if let Some(m) = monitor_of(el, anchor) {
            let sf = m.scale_factor();
            let p = m.position();
            let s = m.size();
            dock = (
                p.x as f64 + s.width as f64 - (W + EDGE) * sf,
                p.y as f64 + s.height as f64 - (H + EDGE) * sf,
            );
            // start offset below the dock — pump() slides it home
            win.set_outer_position(PhysicalPosition::new(dock.0, dock.1 + SLIDE_PX * sf));
        }
        let y_off = SLIDE_PX * win.scale_factor();
        let surf = SbContext::new(win.clone())
            .and_then(|c| Surface::new(&c, win.clone()))
            .map_err(|e| anyhow::anyhow!("card surface: {e}"))?;
        Ok(Self {
            st: CardState {
                font: system_font()?,
                thumb: make_thumb(&img),
                msg: msg.to_string(),
                accent,
                saved,
                sharing: false,
                deadline: Instant::now() + TTL,
                btns: if pin { BTNS_PIN } else { BTNS },
                hover: None,
                pause_left: None,
            },
            img,
            win,
            surf,
            phase: Phase::In(Instant::now()),
            dock,
            y_off,
        })
    }

    pub fn win_id(&self) -> WindowId {
        self.win.id()
    }

    /// Pixels for the follow-up actions — the card owns the capture.
    pub fn image(&self) -> &RgbaImage {
        &self.img
    }

    pub fn saved_path(&self) -> Option<PathBuf> {
        self.st.saved.clone()
    }

    /// A Save click landed — swap the button to Open and report success.
    pub fn set_saved(&mut self, p: PathBuf) {
        self.st.saved = Some(p);
        self.win.request_redraw();
    }

    /// Apply the backdrop transform in place — the saved file (if any)
    /// still holds the un-beautified shot, so Save becomes meaningful
    /// again.
    pub fn beautify(&mut self) {
        self.img = qs_core::beautify(&self.img);
        self.st.thumb = make_thumb(&self.img);
        self.st.saved = None;
        self.notify("Background applied — Save to keep it");
    }

    /// New status line + a fresh countdown; used for action feedback.
    pub fn notify(&mut self, msg: &str) {
        self.st.msg = msg.to_string();
        self.st.set_ttl(Duration::from_secs(4));
        self.win.request_redraw();
    }

    pub fn set_sharing(&mut self, on: bool) {
        self.st.sharing = on;
        self.win.request_redraw();
    }

    /// Host's `about_to_wait` hook — `None` means the card finished its
    /// slide-out and should be dropped.
    pub fn pump(&mut self) -> Option<Instant> {
        let now = Instant::now();
        match self.phase {
            Phase::In(t0) => {
                let t = (now.duration_since(t0).as_secs_f32() / SLIDE_IN).min(1.0);
                let ease = 1.0 - (1.0 - t).powi(3);
                self.y_off = (1.0 - ease as f64) * SLIDE_PX * self.win.scale_factor();
                self.win.set_outer_position(PhysicalPosition::new(
                    self.dock.0,
                    self.dock.1 + self.y_off,
                ));
                if t >= 1.0 {
                    self.phase = Phase::Live;
                }
                return Some(now + FRAME);
            }
            Phase::Out { t0, from } => {
                let t = (now.duration_since(t0).as_secs_f32() / SLIDE_OUT).min(1.0);
                self.win.set_outer_position(PhysicalPosition::new(
                    self.dock.0,
                    self.dock.1 + from + t as f64 * SLIDE_PX * 2.0 * self.win.scale_factor(),
                ));
                return (t < 1.0).then(|| now + FRAME);
            }
            Phase::Live => {}
        }
        if self.st.expired() {
            self.begin_close();
            return Some(now + FRAME);
        }
        // the countdown strip animates — repaint every tick
        self.win.request_redraw();
        Some(now + Duration::from_millis(120))
    }

    /// Route a window event; `Some(action)` means the host should act on it.
    pub fn window_event(&mut self, ev: WindowEvent) -> Option<QaAction> {
        match ev {
            WindowEvent::CloseRequested => Some(QaAction::Close),
            WindowEvent::RedrawRequested => {
                self.draw();
                None
            }
            WindowEvent::CursorMoved { position, .. } => {
                let sf = self.win.scale_factor();
                let h = self.st.hit(position.x / sf, position.y / sf);
                if h != self.st.hover {
                    self.st.hover = h;
                    self.win.request_redraw();
                }
                None
            }
            WindowEvent::CursorEntered { .. } => {
                self.st.pause();
                None
            }
            WindowEvent::CursorLeft { .. } => {
                self.st.hover = None;
                self.st.resume();
                self.win.request_redraw();
                None
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed =>
            {
                self.key(&event.logical_key)
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => match self.st.hover {
                Some(99) => {
                    self.begin_close();
                    None
                }
                // a share already in flight must not spawn a second upload
                Some(i) if matches!(self.st.btns[i], Btn::Share) && self.st.sharing => None,
                Some(i) => Some(self.st.btns[i].action(self.st.saved.is_some())),
                None => None,
            },
            _ => None,
        }
    }

    /// Keyboard shortcuts while the card is focused — CleanShot parity:
    /// c copy / s,o save-or-open / u share / b beautify / p pin / w,q,esc close.
    fn key(&mut self, k: &Key) -> Option<QaAction> {
        match k {
            Key::Named(NamedKey::Escape) => {
                self.begin_close();
                None
            }
            Key::Character(c) => match c.as_str() {
                "c" => Some(QaAction::Copy),
                "s" | "o" => Some(if self.st.saved.is_some() {
                    QaAction::Reveal
                } else {
                    QaAction::SaveNow
                }),
                "u" if !self.st.sharing => Some(QaAction::Share),
                "b" => Some(QaAction::Beautify),
                "p" if self.st.btns.iter().any(|b| matches!(b, Btn::Pin)) => Some(QaAction::Pin),
                "w" | "q" => {
                    self.begin_close();
                    None
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// All dismissals funnel here — the card slides below the dock and
    /// `pump` reports it done, so close always feels symmetrical to open.
    fn begin_close(&mut self) {
        if !matches!(self.phase, Phase::Out { .. }) {
            self.phase = Phase::Out {
                t0: Instant::now(),
                from: self.y_off,
            };
        }
    }

    fn draw(&mut self) {
        let size = self.win.inner_size();
        let (sw, sh) = match (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            (Some(w), Some(h)) => (w.get() as usize, h.get() as usize),
            _ => return,
        };
        let _ = self.surf.resize(
            NonZeroU32::new(sw as u32).unwrap(),
            NonZeroU32::new(sh as u32).unwrap(),
        );
        let Ok(mut buf) = self.surf.buffer_mut() else {
            return;
        };
        buf.fill(0);
        let scale = self.win.scale_factor();
        self.st.render(&mut buf, sw, sh, scale);
        let _ = buf.present();
    }
}

impl CardState {
    /// Freeze the countdown while the cursor rests on the card.
    fn pause(&mut self) {
        if self.pause_left.is_none() {
            self.pause_left = Some(self.deadline.saturating_duration_since(Instant::now()));
        }
    }

    fn resume(&mut self) {
        if let Some(left) = self.pause_left.take() {
            self.deadline = Instant::now() + left;
        }
    }

    fn expired(&self) -> bool {
        self.pause_left.is_none() && Instant::now() >= self.deadline
    }

    fn set_ttl(&mut self, d: Duration) {
        match self.pause_left {
            Some(_) => self.pause_left = Some(d),
            None => self.deadline = Instant::now() + d,
        }
    }

    /// Hit-test logical coords; 99 marks the close circle.
    fn hit(&self, x: f64, y: f64) -> Option<usize> {
        let (cx, cy, r) = CLOSE;
        if (x - cx).powi(2) + (y - cy).powi(2) <= r * r {
            return Some(99);
        }
        self.btns.iter().position(|b| {
            let (bx, by, bw, bh) = b.rect();
            x >= bx && x <= bx + bw && y >= by && y <= by + bh
        })
    }

    /// Paints the whole card into an ARGB buffer — split from `draw` so a
    /// test can render to a file without a window.
    fn render(&self, buf: &mut [u32], sw: usize, sh: usize, scale: f64) {
        let mut cv = Canvas { buf, w: sw, h: sh, scale };

        // card body + fake shadow fringe (same trick as the toast)
        card_chrome(&mut cv, W, H, 11.0, CARD, BORDER, FRINGE);

        // thumbnail: cover-cropped square with rounded corners
        let (tx, ty, tr) = (12.0_f64, (H - THUMB as f64) / 2.0, 8.0_f64);
        for py in 0..sh as i32 {
            for px in 0..sw as i32 {
                let lx = px as f64 / scale;
                let ly = py as f64 / scale;
                if lx < tx || ly < ty || lx >= tx + THUMB as f64 || ly >= ty + THUMB as f64 {
                    continue;
                }
                let d = rrect_sdf(
                    lx,
                    ly,
                    tx + THUMB as f64 / 2.0,
                    ty + THUMB as f64 / 2.0,
                    THUMB as f64 / 2.0,
                    THUMB as f64 / 2.0,
                    tr,
                );
                let cov = fill_cov(d, scale);
                if cov <= 0.0 {
                    continue;
                }
                let i = ((ly - ty) as usize) * THUMB + (lx - tx) as usize;
                cv.blend(px, py, self.thumb[i], cov);
            }
        }

        let t = Txt {
            font: &self.font,
            scale,
        };
        t.draw(&mut cv, 104.0, 40.0, 13.0, &self.msg, FG);

        // action pills — Copy is primary (filled accent); the rest stay
        // ghost so the card reads one-click-deep at a glance
        for (i, b) in self.btns.iter().enumerate() {
            let (bx, by, bw, bh) = b.rect();
            let hot = self.hover == Some(i);
            let dim = matches!(b, Btn::Share) && self.sharing;
            let alpha = if dim { 0.5 } else if hot { 1.0 } else { 0.95 };
            let primary = matches!(b, Btn::Copy);
            let (bg, fg) = if primary {
                (if hot { shade(self.accent, 0.82) } else { self.accent }, 0xFFFFFF)
            } else if hot {
                (BTN_HOT, FG)
            } else {
                (BTN_BG, FG)
            };
            pill(&mut cv, (bx, by, bw, bh), 7.0, bg, if primary { bg } else { BORDER }, alpha);
            let label = if dim { "…" } else { b.label(self.saved.is_some()) };
            let tw = t.width(11.0, label);
            t.draw(&mut cv, bx + (bw - tw) / 2.0, by + 17.5, 11.0, label, fg);
        }

        // close: small circle with an ×
        let (cx, cy, r) = CLOSE;
        for py in ((cy - r) * scale) as i32..((cy + r) * scale) as i32 {
            for px in ((cx - r) * scale) as i32..((cx + r) * scale) as i32 {
                let lx = px as f64 / scale;
                let ly = py as f64 / scale;
                let d = qs_overlay::sdf::ellipse_sdf(lx, ly, cx, cy, r, r);
                if d <= 0.0 {
                    let hot = self.hover == Some(99);
                    cv.blend(px, py, if hot { BTN_HOT } else { BTN_BG }, fill_cov(d, scale));
                }
            }
        }
        t.draw(&mut cv, cx - 3.4, cy + 4.6, 11.0, "×", FG);

        // countdown strip along the bottom — frozen while hovered
        let left = self
            .pause_left
            .unwrap_or_else(|| self.deadline.saturating_duration_since(Instant::now()));
        let rem = left.as_secs_f64() / TTL.as_secs_f64();
        let bw = (W - 24.0) * rem.clamp(0.0, 1.0);
        for py in ((H - 4.0) * scale) as i32..((H - 2.0) * scale) as i32 {
            for px in (12.0 * scale) as i32..((12.0 + bw) * scale) as i32 {
                cv.blend(px, py, self.accent, 0.8);
            }
        }
    }
}

/// One-shot harness: `quickshort --capture` exits after dispatch, so it
/// runs the card on a private event loop — the card's actions then behave
/// exactly like the resident app's, with the blocking clipboard variant.
pub fn standalone(
    img: RgbaImage,
    msg: &str,
    saved: Option<PathBuf>,
    save_dir: PathBuf,
    accent: u32,
    wait_clipboard: bool,
) -> Result<()> {
    use winit::application::ApplicationHandler;
    use winit::event_loop::{ControlFlow, EventLoop, EventLoopProxy};

    enum Ev {
        ShareDone(Result<crate::share::SharedLink, String>),
    }
    struct Run {
        card: Option<QuickAccess>,
        sm: std::sync::Arc<crate::share::ShareManager>,
        proxy: EventLoopProxy<Ev>,
        save_dir: PathBuf,
        wait: bool,
        img: Option<RgbaImage>,
        msg: String,
        saved: Option<PathBuf>,
        accent: u32,
    }
    impl ApplicationHandler<Ev> for Run {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            let img = self.img.take().expect("card opened twice");
            match QuickAccess::open(el, img, &self.msg, self.saved.take(), self.accent, false, None) {
                Ok(c) => self.card = Some(c),
                Err(e) => {
                    eprintln!("card: {e:#}");
                    el.exit();
                }
            }
        }
        fn window_event(&mut self, _el: &ActiveEventLoop, id: WindowId, ev: WindowEvent) {
            let post = match &mut self.card {
                Some(c) if c.win_id() == id => c.window_event(ev).and_then(|act| {
                    CardEnv {
                        save_dir: &self.save_dir,
                        copy_wait: self.wait,
                        share: &self.sm,
                        proxy: &self.proxy,
                        share_done: Ev::ShareDone,
                    }
                    .action(c, act)
                }),
                _ => None,
            };
            if post == Some(CardPost::Closed) {
                self.card = None;
            }
        }
        fn user_event(&mut self, _el: &ActiveEventLoop, ev: Ev) {
            let Ev::ShareDone(res) = ev;
            if let Some(c) = &mut self.card {
                share_finished(c, res, self.wait);
            }
        }
        fn about_to_wait(&mut self, el: &ActiveEventLoop) {
            match self.card.as_mut().and_then(|c| c.pump()) {
                Some(wake) => el.set_control_flow(ControlFlow::WaitUntil(wake)),
                None => el.exit(),
            }
        }
    }
    let el = EventLoop::<Ev>::with_user_event().build()?;
    let mut run = Run {
        card: None,
        sm: crate::share::ShareManager::new(),
        proxy: el.create_proxy(),
        save_dir,
        wait: wait_clipboard,
        img: Some(img),
        msg: msg.to_string(),
        saved,
        accent,
    };
    el.run_app(&mut run)?;
    Ok(())
}

/// Cover-crop `img` down to a THUMB×THUMB ARGB square (nearest neighbour —
/// it's a preview, not the shipped asset).
fn make_thumb(img: &RgbaImage) -> Vec<u32> {
    let (iw, ih) = (img.width() as usize, img.height() as usize);
    let s = iw.max(ih) as f64 / THUMB as f64;
    let (ox, oy) = ((iw as f64 - THUMB as f64 * s).max(0.0) / 2.0, (ih as f64 - THUMB as f64 * s).max(0.0) / 2.0);
    let mut t = vec![0u32; THUMB * THUMB];
    for y in 0..THUMB {
        for x in 0..THUMB {
            let sx = (ox + x as f64 * s) as usize;
            let sy = (oy + y as f64 * s) as usize;
            let p = img.get_pixel(sx.min(iw - 1) as u32, sy.min(ih - 1) as u32);
            t[y * THUMB + x] =
                ((p[3] as u32) << 24) | ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32;
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test -p qs-app card_png -- --ignored` writes a full-card
    /// render so layout changes can be eyeballed without a display.
    #[test]
    #[ignore]
    fn card_png() {
        let img = RgbaImage::from_fn(800, 500, |x, y| {
            image::Rgba([(x % 255) as u8, (y % 255) as u8, 160, 255])
        });
        let st = CardState {
            font: system_font().unwrap(),
            thumb: make_thumb(&img),
            msg: "Saved quickshort-demo.png".into(),
            accent: 0x1677DF,
            saved: Some("/tmp/x.png".into()),
            sharing: false,
            deadline: Instant::now() + Duration::from_secs(6),
            btns: BTNS_PIN,
            hover: Some(0),
            pause_left: None,
        };
        let (w, h) = (W as usize, H as usize);
        let mut buf = vec![0u32; w * h];
        st.render(&mut buf, w, h, 1.0);
        let mut out = RgbaImage::new(w as u32, h as u32);
        for (i, px) in buf.iter().enumerate() {
            out.put_pixel(
                (i % w) as u32,
                (i / w) as u32,
                image::Rgba([(px >> 16) as u8, (px >> 8) as u8, *px as u8, 255]),
            );
        }
        out.save("/tmp/quickshort-card.png").unwrap();
    }
}
