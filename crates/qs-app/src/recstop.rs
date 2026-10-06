//! Recording indicator: a tiny always-on-top card showing "● REC 0:07"
//! and a Stop pill. On Wayland the global hotkey can't fire, so the
//! click IS the stop mechanism — it's also the visible proof that a
//! recording is running.

use anyhow::{Context, Result};
use fontdue::Font;
use qs_overlay::sdf::{ellipse_sdf, fill_cov, rrect_sdf, stroke_cov, Canvas};
use softbuffer::{Context as SbContext, Surface};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId, WindowLevel};

use crate::settings::system_font;

const W: f64 = 172.0;
const H: f64 = 46.0;
const STOP: (f64, f64, f64, f64) = (108.0, 10.0, 52.0, 26.0);

const CARD: u32 = 0x24272D;
const BORDER: u32 = 0x444A55;
const FRINGE: u32 = 0x000000;
const FG: u32 = 0xF2F4F7;
const RED: u32 = 0xE5484D;
const STOP_BG: u32 = 0x3A3F47;
const STOP_HOT: u32 = 0xE5484D;

pub struct RecStop {
    win: Rc<Window>,
    surf: Surface<Rc<Window>, Rc<Window>>,
    font: Font,
    t0: Instant,
    hover_stop: bool,
}

impl RecStop {
    /// Top-center of the primary monitor — visible but out of the way.
    pub fn open(el: &ActiveEventLoop) -> Result<Self> {
        let attrs = Window::default_attributes()
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(LogicalSize::new(W, H));
        let win = Rc::new(el.create_window(attrs).context("rec window")?);
        if let Some(m) = el
            .primary_monitor()
            .or_else(|| el.available_monitors().next())
        {
            let sf = m.scale_factor();
            let p = m.position();
            let s = m.size();
            let x = p.x as f64 + (s.width as f64 - W * sf) / 2.0;
            let y = p.y as f64 + 16.0 * sf;
            win.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
        }
        let surf = SbContext::new(win.clone())
            .and_then(|c| Surface::new(&c, win.clone()))
            .map_err(|e| anyhow::anyhow!("rec surface: {e}"))?;
        Ok(Self {
            win,
            surf,
            font: system_font()?,
            t0: Instant::now(),
            hover_stop: false,
        })
    }

    pub fn win_id(&self) -> WindowId {
        self.win.id()
    }

    /// Host's `about_to_wait` hook — the timer redraws twice a second.
    pub fn pump(&mut self) -> Instant {
        self.win.request_redraw();
        Instant::now() + Duration::from_millis(500)
    }

    /// `true` when the user clicked Stop (host flips the stop flag).
    pub fn window_event(&mut self, ev: WindowEvent) -> bool {
        match ev {
            WindowEvent::CloseRequested => true,
            WindowEvent::RedrawRequested => {
                self.draw();
                false
            }
            WindowEvent::CursorMoved { position, .. } => {
                let sf = self.win.scale_factor();
                let (x, y) = (position.x / sf, position.y / sf);
                let (bx, by, bw, bh) = STOP;
                let hot = x >= bx && x <= bx + bw && y >= by && y <= by + bh;
                if hot != self.hover_stop {
                    self.hover_stop = hot;
                    self.win.request_redraw();
                }
                false
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => self.hover_stop,
            _ => false,
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
        let mut cv = Canvas {
            buf: &mut buf,
            w: sw,
            h: sh,
            scale,
        };

        // dark card + fringe (same trick as the toast/card)
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

        // pulsing record dot
        let pulse = 0.55 + 0.45 * (self.t0.elapsed().as_secs_f64() * 3.0).sin().abs();
        for py in ((H / 2.0 - 5.0) * scale) as i32..((H / 2.0 + 5.0) * scale) as i32 {
            for px in (9.0 * scale) as i32..(19.0 * scale) as i32 {
                let d = ellipse_sdf(px as f64 / scale, py as f64 / scale, 14.0, H / 2.0, 5.0, 5.0);
                let cov = fill_cov(d, scale) * pulse;
                if cov > 0.0 {
                    cv.blend(px, py, RED, cov);
                }
            }
        }

        // elapsed time
        let secs = self.t0.elapsed().as_secs();
        let label = format!("REC {}:{:02}", secs / 60, secs % 60);
        let t = Txt {
            font: &self.font,
            scale,
        };
        t.draw(&mut cv, 26.0, H / 2.0 + 4.5, 12.0, &label, FG);

        // Stop pill
        let (bx, by, bw, bh) = STOP;
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
                    cv.blend(
                        px,
                        py,
                        if self.hover_stop { STOP_HOT } else { STOP_BG },
                        fill_cov(d, scale),
                    );
                    let rim = stroke_cov(d, 0.7, scale);
                    if rim > 0.0 {
                        cv.blend(px, py, BORDER, rim);
                    }
                }
            }
        }
        let tw = text_width(&self.font, scale, 11.0, "Stop");
        t.draw(&mut cv, bx + (bw - tw) / 2.0, by + 17.5, 11.0, "Stop", FG);

        let _ = buf.present();
    }
}

fn text_width(font: &Font, scale: f64, size: f64, s: &str) -> f64 {
    let px = (size * scale) as f32;
    s.chars().map(|c| font.metrics(c, px).advance_width).sum::<f32>() as f64 / scale
}

/// One-shot runner: blocks with the stop card on screen until the user
/// clicks Stop, then flips `stop` and returns — the caller's worker
/// finalizes the encode.
pub fn standalone(stop: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Result<()> {
    use winit::application::ApplicationHandler;
    use winit::event_loop::{ControlFlow, EventLoop};

    struct Run {
        rec: Option<RecStop>,
        wake: Instant,
        done: bool,
    }
    impl ApplicationHandler<()> for Run {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            self.rec = RecStop::open(el).map_err(|e| eprintln!("rec: {e:#}")).ok();
        }
        fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, ev: WindowEvent) {
            let Some(r) = &mut self.rec else { return };
            if r.win_id() == id && r.window_event(ev) {
                self.rec = None;
                self.done = true;
                el.exit();
            }
        }
        fn about_to_wait(&mut self, el: &ActiveEventLoop) {
            if let Some(r) = &mut self.rec {
                self.wake = r.pump();
            }
            el.set_control_flow(ControlFlow::WaitUntil(self.wake));
        }
    }

    let el = EventLoop::<()>::with_user_event().build()?;
    let mut run = Run {
        rec: None,
        wake: Instant::now() + Duration::from_millis(500),
        done: false,
    };
    el.run_app(&mut run)?;
    if run.done {
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(())
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
