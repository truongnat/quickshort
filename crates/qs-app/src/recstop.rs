//! Recording indicator: a tiny always-on-top card showing "● REC 0:07"
//! and a Stop pill. On Wayland the global hotkey can't fire, so the
//! click IS the stop mechanism — it's also the visible proof that a
//! recording is running.

use anyhow::{Context, Result};
use fontdue::Font;
use qs_overlay::sdf::{ellipse_sdf, fill_cov, Canvas};
use softbuffer::{Context as SbContext, Surface};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId, WindowLevel};

use crate::settings::system_font;
use crate::widgets::{card_chrome, monitor_of, pill, Txt};

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
    /// Top-center of the monitor holding `anchor` (the region being
    /// recorded) — visible but out of the way, GNOME-indicator style.
    pub fn open(el: &ActiveEventLoop, anchor: Option<qs_core::Rect>) -> Result<Self> {
        let attrs = Window::default_attributes()
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(LogicalSize::new(W, H));
        let win = Rc::new(el.create_window(attrs).context("rec window")?);
        if let Some(m) = monitor_of(el, anchor) {
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

        card_chrome(&mut cv, W, H, 11.0, CARD, BORDER, FRINGE);

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
        let (bx, by, bw, _bh) = STOP;
        pill(
            &mut cv,
            STOP,
            7.0,
            if self.hover_stop { STOP_HOT } else { STOP_BG },
            BORDER,
            1.0,
        );
        let tw = t.width(11.0, "Stop");
        t.draw(&mut cv, bx + (bw - tw) / 2.0, by + 17.5, 11.0, "Stop", FG);

        let _ = buf.present();
    }
}