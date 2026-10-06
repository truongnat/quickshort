//! Quick-access card — a small always-on-top thumbnail popup shown after a
//! resident capture completes. Gives one-click follow-up actions (Copy /
//! Save / Share) without reopening the overlay; auto-dismisses on a
//! countdown that resets whenever the user interacts with it.

use anyhow::{Context, Result};
use fontdue::Font;
use image::RgbaImage;
use qs_overlay::sdf::{fill_cov, rrect_sdf, stroke_cov, Canvas};
use softbuffer::{Context as SbContext, Surface};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId, WindowLevel};

use crate::settings::system_font;

const W: f64 = 424.0;
const H: f64 = 108.0;
const TTL: Duration = Duration::from_secs(8);
const THUMB: usize = 80;

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
}

pub struct QuickAccess {
    win: Rc<Window>,
    surf: Surface<Rc<Window>, Rc<Window>>,
    img: RgbaImage,
    st: CardState,
}

impl QuickAccess {
    /// Card docked bottom-right of the primary monitor.
    /// `pin` enables the Pin pill — only the resident host can keep a
    /// pinned window alive.
    pub fn open(
        el: &ActiveEventLoop,
        img: RgbaImage,
        msg: &str,
        saved: Option<PathBuf>,
        accent: u32,
        pin: bool,
    ) -> Result<Self> {
        let attrs = Window::default_attributes()
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(LogicalSize::new(W, H));
        let win = Rc::new(el.create_window(attrs).context("card window")?);
        if let Some(m) = el
            .primary_monitor()
            .or_else(|| el.available_monitors().next())
        {
            let sf = m.scale_factor();
            let p = m.position();
            let s = m.size();
            let x = p.x as f64 + s.width as f64 - (W + 20.0) * sf;
            let y = p.y as f64 + s.height as f64 - (H + 20.0) * sf;
            win.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
        }
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
            },
            img,
            win,
            surf,
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
        self.st.deadline = Instant::now() + Duration::from_secs(4);
        self.win.request_redraw();
    }

    pub fn set_sharing(&mut self, on: bool) {
        self.st.sharing = on;
        self.win.request_redraw();
    }

    /// Host's `about_to_wait` hook — `None` means the card expired.
    pub fn pump(&mut self) -> Option<Instant> {
        if Instant::now() >= self.st.deadline {
            return None;
        }
        // the countdown strip animates — repaint every tick
        self.win.request_redraw();
        Some(Instant::now() + Duration::from_millis(120))
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
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => match self.st.hover {
                Some(99) => Some(QaAction::Close),
                // a share already in flight must not spawn a second upload
                Some(i) if matches!(self.st.btns[i], Btn::Share) && self.st.sharing => None,
                Some(i) => Some(self.st.btns[i].action(self.st.saved.is_some())),
                None => None,
            },
            _ => None,
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
        for py in 0..sh as i32 {
            for px in 0..sw as i32 {
                let d = rrect_sdf(
                    px as f64 / scale,
                    py as f64 / scale,
                    W / 2.0,
                    H / 2.0,
                    W / 2.0,
                    H / 2.0,
                    11.0,
                );
                if d > 0.0 {
                    cv.blend(px, py, FRINGE, 0.35 * (1.0 - d / 2.5).clamp(0.0, 1.0));
                    continue;
                }
                cv.blend(px, py, CARD, fill_cov(d, scale));
                let rim = stroke_cov(d, 0.7, scale);
                if rim > 0.0 {
                    cv.blend(px, py, BORDER, rim);
                }
            }
        }

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

        // action pills
        for (i, b) in self.btns.iter().enumerate() {
            let (bx, by, bw, bh) = b.rect();
            let hot = self.hover == Some(i);
            let dim = matches!(b, Btn::Share) && self.sharing;
            for py in (by * scale) as i32..((by + bh) * scale) as i32 {
                for px in (bx * scale) as i32..((bx + bw) * scale) as i32 {
                    let d = rrect_sdf(
                        px as f64 / scale,
                        py as f64 / scale,
                        bx + bw / 2.0,
                        by + bh / 2.0,
                        bw / 2.0,
                        bh / 2.0,
                        7.0,
                    );
                    if d <= 0.0 {
                        let c = if dim {
                            0.5
                        } else if hot {
                            1.0
                        } else {
                            0.95
                        };
                        cv.blend(px, py, if hot { BTN_HOT } else { BTN_BG }, fill_cov(d, scale) * c);
                        let rim = stroke_cov(d, 0.7, scale);
                        if rim > 0.0 {
                            cv.blend(px, py, BORDER, rim);
                        }
                    }
                }
            }
            let label = if dim { "…" } else { b.label(self.saved.is_some()) };
            let tw = text_width(&self.font, scale, 11.0, label);
            t.draw(&mut cv, bx + (bw - tw) / 2.0, by + 17.5, 11.0, label, FG);
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

        // countdown strip along the bottom
        let rem = (self.deadline - Instant::now()).as_secs_f64() / TTL.as_secs_f64();
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
            match QuickAccess::open(el, img, &self.msg, self.saved.take(), self.accent, false) {
                Ok(c) => self.card = Some(c),
                Err(e) => {
                    eprintln!("card: {e:#}");
                    el.exit();
                }
            }
        }
        fn window_event(&mut self, _el: &ActiveEventLoop, id: WindowId, ev: WindowEvent) {
            let act = match &mut self.card {
                Some(c) if c.win_id() == id => c.window_event(ev),
                _ => None,
            };
            if matches!(act, Some(QaAction::Close)) {
                self.card = None;
                return;
            }
            let (Some(act), Some(c)) = (act, &mut self.card) else {
                return;
            };
            match act {
                QaAction::Copy => {
                    crate::actions::copy_image(c.image(), self.wait);
                    c.notify("Copied to clipboard");
                }
                QaAction::SaveNow => {
                    match crate::actions::save_to_dir(c.image(), &self.save_dir) {
                        Ok(p) => {
                            c.set_saved(p);
                            c.notify("Screenshot saved");
                        }
                        Err(e) => c.notify(&format!("Save failed: {e}")),
                    }
                }
                QaAction::Reveal => {
                    if let Some(p) = c.saved_path() {
                        crate::actions::open_path(&p);
                    }
                }
                QaAction::Beautify => c.beautify(),
                // Pin is resident-only — the pill isn't rendered here
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
        fn user_event(&mut self, _el: &ActiveEventLoop, ev: Ev) {
            let Ev::ShareDone(res) = ev;
            let Some(c) = &mut self.card else { return };
            c.set_sharing(false);
            match res {
                Ok(l) => {
                    crate::actions::copy_text(&l.url, self.wait);
                    c.notify(&format!("Link copied — expires in {}d", l.expires_s / 86400));
                }
                Err(e) => c.notify(&format!("Share failed: {e}")),
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

fn text_width(font: &Font, scale: f64, size: f64, s: &str) -> f64 {
    let px = (size * scale) as f32;
    s.chars().map(|c| font.metrics(c, px).advance_width).sum::<f32>() as f64 / scale
}

struct Txt<'a> {
    font: &'a Font,
    scale: f64,
}

impl Txt<'_> {
    fn draw(&self, cv: &mut Canvas, x: f64, y: f64, size: f64, s: &str, rgb: u32) {
        let px = size * self.scale;
        let mut pen = x * self.scale;
        let baseline = y * self.scale;
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
    }
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
