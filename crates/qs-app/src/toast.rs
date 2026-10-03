//! Transient status toast — a small undecorated always-on-top window that
//! reports long-running action progress (OCR) and auto-dismisses.

use anyhow::{Context, Result};
use fontdue::Font;
use qs_overlay::sdf::{ellipse_sdf, fill_cov, glass_card, seg_sdf, stroke_cov, Canvas};
use softbuffer::{Context as SbContext, Surface};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId, WindowLevel};

use crate::settings::system_font;

const W: f64 = 272.0;
const H: f64 = 58.0;
const DONE_TTL: Duration = Duration::from_millis(1800);

const CARD_TOP: u32 = 0x3A3A41;
const CARD_BOT: u32 = 0x26262C;
const FG: u32 = 0xF2F2F7;
const FG_DIM: u32 = 0xA9A9B1;
const ACCENT: u32 = 0x0A84FF;
const OK: u32 = 0x30D158;
const ERR: u32 = 0xFF453A;

#[derive(Clone, Copy, PartialEq)]
enum State {
    /// spinner running — an action is still in flight
    Working,
    Ok,
    Err,
}

pub struct Toast {
    win: Rc<Window>,
    surf: Surface<Rc<Window>, Rc<Window>>,
    font: Font,
    msg: String,
    state: State,
    pulse: f64,
    deadline: Option<Instant>,
    dirty: bool,
}

impl Toast {
    /// Floating card, bottom-center of the focused monitor.
    pub fn open(el: &ActiveEventLoop, msg: &str) -> Result<Self> {
        let attrs = Window::default_attributes()
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(LogicalSize::new(W, H));
        let win = Rc::new(el.create_window(attrs).context("toast window")?);
        // bottom-center of the primary monitor
        if let Some(m) = el
            .primary_monitor()
            .or_else(|| el.available_monitors().next())
        {
            let sf = m.scale_factor();
            let p = m.position();
            let s = m.size();
            let x = p.x as f64 + (s.width as f64 - W * sf) / 2.0;
            let y = p.y as f64 + s.height as f64 - (H + 96.0) * sf;
            win.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
        }
        let surf = SbContext::new(win.clone())
            .and_then(|c| Surface::new(&c, win.clone()))
            .map_err(|e| anyhow::anyhow!("toast surface: {e}"))?;
        Ok(Self {
            win,
            surf,
            font: system_font()?,
            msg: msg.to_string(),
            state: State::Working,
            pulse: 0.0,
            deadline: None,
            dirty: true,
        })
    }

    pub fn win_id(&self) -> WindowId {
        self.win.id()
    }

    /// Switch to the finished look; the toast lingers for `DONE_TTL`.
    pub fn done(&mut self, msg: String, ok: bool) {
        self.msg = msg;
        self.state = if ok { State::Ok } else { State::Err };
        self.deadline = Some(Instant::now() + DONE_TTL);
        self.dirty = true;
        self.win.request_redraw();
    }

    pub fn expired(&self) -> bool {
        self.deadline.is_some_and(|d| Instant::now() >= d)
    }

    /// Called from the host's `about_to_wait`. Returns the next wake instant
    /// while alive, `None` once expired — the host should then drop the toast.
    pub fn pump(&mut self) -> Option<Instant> {
        if self.expired() {
            return None;
        }
        if self.dirty || self.state == State::Working || self.deadline.is_some() {
            self.win.request_redraw();
        }
        match self.state {
            State::Working => Some(Instant::now() + Duration::from_millis(60)),
            _ => Some(Instant::now() + Duration::from_millis(100)),
        }
    }

    /// returns true when the window should be torn down
    pub fn window_event(&mut self, ev: WindowEvent) -> bool {
        match ev {
            WindowEvent::CloseRequested => true,
            WindowEvent::RedrawRequested => {
                self.draw();
                false
            }
            _ => false,
        }
    }

    fn draw(&mut self) {
        let working = self.state == State::Working;
        self.pulse += 0.12;
        self.dirty = working;

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
        let mut cv = Canvas {
            buf: &mut buf,
            w: sw,
            h: sh,
            scale,
        };

        // softbuffer ignores per-pixel alpha — the card must fill the whole
        // window or the uncovered corners render as an opaque black frame.
        glass_card(&mut cv, (0.0, 0.0, W, H), 2.0, CARD_TOP, CARD_BOT);

        // state icon: filled circle, glyph inside
        let (cx, cy, rad) = (28.0, H / 2.0 - 1.5, 11.0);
        match self.state {
            State::Working => {
                let a = 0.45 + 0.55 * self.pulse.sin().abs();
                fill_circle(&mut cv, cx, cy, rad, ACCENT, a, scale);
                ring(&mut cv, cx, cy, rad + 3.5, ACCENT, a * 0.35, scale);
            }
            State::Ok => {
                fill_circle(&mut cv, cx, cy, rad, OK, 1.0, scale);
                tick(&mut cv, cx, cy, scale);
            }
            State::Err => {
                fill_circle(&mut cv, cx, cy, rad, ERR, 1.0, scale);
                cross(&mut cv, cx, cy, scale);
            }
        }

        // message
        let t = Txt {
            font: &self.font,
            scale,
        };
        t.draw(
            &mut cv,
            50.0,
            H / 2.0 + 4.0,
            13.0,
            &self.msg,
            if working { FG_DIM } else { FG },
        );

        // countdown bar: accent strip along the bottom edge while done
        if let Some(d) = self.deadline {
            let rem = (d - Instant::now()).as_secs_f64() / DONE_TTL.as_secs_f64();
            let frac = rem.clamp(0.0, 1.0);
            let col = if self.state == State::Err { ERR } else { OK };
            let bw = (W - 24.0) * frac;
            let y = H - 4.0;
            for py in (y * scale) as i32..((y + 2.0) * scale) as i32 {
                for px in (12.0 * scale) as i32..((12.0 + bw) * scale) as i32 {
                    cv.blend(px, py, col, 0.85);
                }
            }
        }

        let _ = buf.present();
    }
}

/// Dev harness: `quickshort --toast` shows a working->done toast cycle.
pub fn standalone() -> Result<()> {
    use winit::application::ApplicationHandler;
    use winit::event_loop::{ControlFlow, EventLoop};

    struct Run {
        t: Option<Toast>,
        t0: Instant,
    }
    impl ApplicationHandler for Run {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            match Toast::open(el, "Extracting text…") {
                Ok(t) => self.t = Some(t),
                Err(e) => {
                    eprintln!("toast: {e:#}");
                    el.exit();
                }
            }
        }
        fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, ev: WindowEvent) {
            if let Some(t) = &mut self.t {
                if t.win_id() == id && t.window_event(ev) {
                    self.t = None;
                    el.exit();
                }
            }
        }
        fn about_to_wait(&mut self, el: &ActiveEventLoop) {
            if self.t0.elapsed() > Duration::from_millis(900) {
                if let Some(t) = &mut self.t {
                    if t.deadline.is_none() {
                        t.done("Copied 683 chars".to_string(), true);
                    }
                }
            }
            match self.t.as_mut().and_then(|t| t.pump()) {
                Some(wake) => el.set_control_flow(ControlFlow::WaitUntil(wake)),
                None => el.exit(),
            }
        }
    }
    let el = EventLoop::new()?;
    let mut run = Run {
        t: None,
        t0: Instant::now(),
    };
    el.run_app(&mut run)?;
    Ok(())
}

// --- tiny SDF helpers (logical coords, self-contained) ----------------------

fn fill_circle(cv: &mut Canvas, cx: f64, cy: f64, r: f64, rgb: u32, a: f64, s: f64) {
    for py in ((cy - r) * s) as i32..((cy + r) * s) as i32 {
        for px in ((cx - r) * s) as i32..((cx + r) * s) as i32 {
            let d = ellipse_sdf(px as f64 / s, py as f64 / s, cx, cy, r, r);
            let c = fill_cov(d, s) * a;
            if c > 0.0 {
                cv.blend(px, py, rgb, c);
            }
        }
    }
}

fn ring(cv: &mut Canvas, cx: f64, cy: f64, r: f64, rgb: u32, a: f64, s: f64) {
    for py in ((cy - r - 2.0) * s) as i32..((cy + r + 2.0) * s) as i32 {
        for px in ((cx - r - 2.0) * s) as i32..((cx + r + 2.0) * s) as i32 {
            let d = ellipse_sdf(px as f64 / s, py as f64 / s, cx, cy, r, r);
            let c = stroke_cov(d, 1.0, s) * a;
            if c > 0.0 {
                cv.blend(px, py, rgb, c);
            }
        }
    }
}

fn tick(cv: &mut Canvas, cx: f64, cy: f64, s: f64) {
    for py in ((cy - 6.0) * s) as i32..((cy + 6.0) * s) as i32 {
        for px in ((cx - 6.0) * s) as i32..((cx + 6.0) * s) as i32 {
            let lx = px as f64 / s;
            let ly = py as f64 / s;
            let d = seg_sdf(lx, ly, cx - 4.0, cy, cx - 1.0, cy + 3.0).min(seg_sdf(
                lx,
                ly,
                cx - 1.0,
                cy + 3.0,
                cx + 4.5,
                cy - 3.5,
            ));
            let c = stroke_cov(d, 1.6, s);
            if c > 0.0 {
                cv.blend(px, py, 0xFFFFFF, c);
            }
        }
    }
}

fn cross(cv: &mut Canvas, cx: f64, cy: f64, s: f64) {
    for py in ((cy - 6.0) * s) as i32..((cy + 6.0) * s) as i32 {
        for px in ((cx - 6.0) * s) as i32..((cx + 6.0) * s) as i32 {
            let lx = px as f64 / s;
            let ly = py as f64 / s;
            let d = seg_sdf(lx, ly, cx - 3.0, cy - 3.0, cx + 3.0, cy + 3.0).min(seg_sdf(
                lx,
                ly,
                cx - 3.0,
                cy + 3.0,
                cx + 3.0,
                cy - 3.0,
            ));
            let c = stroke_cov(d, 1.6, s);
            if c > 0.0 {
                cv.blend(px, py, 0xFFFFFF, c);
            }
        }
    }
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
