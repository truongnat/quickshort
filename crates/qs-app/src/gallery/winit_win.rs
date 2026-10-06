//! winit-window backend — a plain borderless window hosting the grid.
//! Used on non-Linux platforms and as the fallback when the compositor
//! lacks `zwlr_layer_shell_v1`. On Wayland it can't position itself
//! (compositor centers it) — that's what `layer.rs` fixes.

use anyhow::{Context, Result};
use fontdue::Font;
use qs_overlay::sdf::Canvas;
use softbuffer::{Context as SbContext, Surface};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::Instant;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

use super::{CellData, GalleryEvent, Hit, Pump, State, Tick, FRAME, H, W};
use crate::settings::system_font;
use crate::widgets::{monitor_of, Txt};

pub(crate) struct WinGallery {
    win: Rc<Window>,
    surf: Surface<Rc<Window>, Rc<Window>>,
    font: Font,
    st: State,
}

impl WinGallery {
    /// Window centred on the primary monitor; `cells` arrive decoded.
    pub fn open(el: &ActiveEventLoop, cells: Vec<CellData>, accent: u32) -> Result<Self> {
        let attrs = Window::default_attributes()
            .with_title("Quickshort — Recent Captures")
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(LogicalSize::new(W, H));
        let win = Rc::new(el.create_window(attrs).context("gallery window")?);
        if let Some(m) = monitor_of(el, None) {
            let p = m.position();
            let s = m.size();
            let sf = m.scale_factor();
            win.set_outer_position(winit::dpi::PhysicalPosition::new(
                p.x as f64 + (s.width as f64 - W * sf) / 2.0,
                p.y as f64 + (s.height as f64 - H * sf) / 2.0,
            ));
        }
        let surf = SbContext::new(win.clone())
            .and_then(|c| Surface::new(&c, win.clone()))
            .map_err(|e| anyhow::anyhow!("gallery surface: {e}"))?;
        Ok(Self {
            st: State::new(cells, accent),
            font: system_font()?,
            win,
            surf,
        })
    }

    pub fn win_id(&self) -> WindowId {
        self.win.id()
    }

    /// `Some(event)` = host should act; window keeps living for most.
    pub fn window_event(&mut self, ev: WindowEvent) -> Option<GalleryEvent> {
        match ev {
            WindowEvent::CloseRequested => Some(GalleryEvent::Closed),
            WindowEvent::Focused(false) => {
                // popover semantics — losing focus dismisses
                self.st.begin_close();
                None
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed =>
            {
                if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
                    self.st.escape();
                }
                None
            }
            WindowEvent::CursorMoved { position, .. } => {
                let sf = self.win.scale_factor();
                if self.st.moved(position.x / sf, position.y / sf) {
                    self.win.request_redraw();
                }
                None
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let d = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64 * 42.0,
                    MouseScrollDelta::PixelDelta(p) => p.y / self.win.scale_factor(),
                };
                self.st.wheel(d);
                None
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                match self.st.clicked() {
                    Hit::Event(e) => return Some(e),
                    Hit::Header => {
                        let _ = self.win.drag_window();
                    }
                    // Closed begins the slide-out; Miss may have armed a
                    // delete — either way a frame is due
                    Hit::Closed | Hit::Miss => self.win.request_redraw(),
                }
                None
            }
            WindowEvent::RedrawRequested => {
                self.draw();
                None
            }
            _ => None,
        }
    }

    /// Animation driver — Wake while anything is moving, Dead when the
    /// close slide finished (host drops us).
    pub fn pump(&mut self) -> Pump {
        let now = Instant::now();
        match self.st.tick(now) {
            Tick::Dead => Pump::Dead,
            Tick::Idle => Pump::Idle,
            Tick::Busy => {
                self.win.request_redraw();
                Pump::Wake(now + FRAME)
            }
        }
    }

    fn draw(&mut self) {
        let size = self.win.inner_size();
        let (sw, sh) = match (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            (Some(w), Some(h)) => (w.get() as usize, h.get() as usize),
            _ => return,
        };
        // read all grid state up front — once buffer_mut borrows the
        // surface exclusively, &self methods are off-limits
        let now = Instant::now();
        let gdy = self.st.gdy(now);
        let g = self.st.grid();
        let _ = self.surf.resize(
            NonZeroU32::new(sw as u32).unwrap(),
            NonZeroU32::new(sh as u32).unwrap(),
        );
        let Ok(mut buf) = self.surf.buffer_mut() else {
            return;
        };
        let scale = self.win.scale_factor();
        let mut cv = Canvas {
            buf: &mut buf,
            w: sw,
            h: sh,
            scale,
        };
        let t = Txt {
            font: &self.font,
            scale,
        };
        g.render(&mut cv, &t, now, gdy);
        let _ = buf.present();
    }
}
