//! Overlay: one borderless-fullscreen window per monitor covering the whole
//! virtual screen (works on X11 *and* Wayland — Wayland has no global window
//! positioning). Renders the captured frames dimmed, tracks a drag selection
//! in virtual coords, returns it on mouse release. Pointer grab semantics keep
//! events flowing to the grab window when the cursor crosses monitors.

use anyhow::Result;
use image::imageops;
use qs_core::{CapturedFrame, Point, Rect};
use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::rc::Rc;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, Window, WindowId, WindowLevel};

const MIN_SELECT: u32 = 3;

/// Show the overlay. `bounds` = virtual screen bounds. Returns the selection
/// in virtual coords, or `None` on Esc / click-without-drag / window close.
pub fn select_region(frames: &[CapturedFrame], bounds: Rect) -> Result<Option<Rect>> {
    let (bright, dim) = composite(frames, bounds);
    let el = EventLoop::new()?;
    el.set_control_flow(ControlFlow::Wait);
    let mut app = App {
        bounds,
        bright,
        dim,
        ..App::default_state()
    };
    el.run_app(&mut app)?;
    Ok(app.result)
}

/// Blit all frames into one virtual-space canvas (0xRRGGBB), downscaling
/// hi-DPI frames to logical size for display.
fn composite(frames: &[CapturedFrame], bounds: Rect) -> (Vec<u32>, Vec<u32>) {
    let (w, h) = (bounds.w as usize, bounds.h as usize);
    let mut canvas = vec![0u32; w * h];
    for f in frames {
        let mon = f.geometry.logical_rect();
        let img = if (f.geometry.scale - 1.0).abs() > f64::EPSILON {
            imageops::resize(&f.pixels, mon.w, mon.h, imageops::FilterType::Triangle)
        } else {
            f.pixels.clone()
        };
        for row in 0..img.height() {
            let dy = mon.y - bounds.y + row as i32;
            if dy < 0 || dy >= bounds.h as i32 {
                continue;
            }
            for col in 0..img.width() {
                let dx = mon.x - bounds.x + col as i32;
                if dx < 0 || dx >= bounds.w as i32 {
                    continue;
                }
                let p = img.get_pixel(col, row);
                canvas[dy as usize * w + dx as usize] =
                    ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32;
            }
        }
    }
    let dim = canvas.iter().map(|&p| (p & 0xFEFEFE) >> 1).collect();
    (canvas, dim)
}

struct WinCtx {
    win: Rc<Window>,
    surf: Surface<Rc<Window>, Rc<Window>>,
    /// this monitor's rect in virtual coords
    vrect: Rect,
}

struct App {
    bounds: Rect,
    bright: Vec<u32>,
    dim: Vec<u32>,
    anchor: Option<Point>, // virtual coords
    cursor: Point,         // virtual coords (inside the grabbing window)
    result: Option<Rect>,
    done: bool,
    dirty: bool,
    wins: Vec<WinCtx>,
}

impl App {
    fn default_state() -> Self {
        Self {
            bounds: Rect::default(),
            bright: Vec::new(),
            dim: Vec::new(),
            anchor: None,
            cursor: Point::default(),
            result: None,
            done: false,
            dirty: false,
            wins: Vec::new(),
        }
    }

    fn finish(&mut self, el: &ActiveEventLoop, result: Option<Rect>) {
        self.result = result;
        self.done = true;
        el.exit();
    }

    fn redraw_all(&self) {
        for w in &self.wins {
            w.win.request_redraw();
        }
    }

    /// MonitorHandle -> monitor rect in virtual coords.
    /// winit reports position/size in physical px; divide by scale_factor.
    fn vrect_of(m: &MonitorHandle) -> Rect {
        let s = m.scale_factor();
        let p = m.position();
        let sz = m.size();
        Rect::new(
            (p.x as f64 / s).round() as i32,
            (p.y as f64 / s).round() as i32,
            (sz.width as f64 / s).round() as u32,
            (sz.height as f64 / s).round() as u32,
        )
    }

    fn draw(&mut self, id: WindowId) {
        let Some(i) = self.wins.iter().position(|w| w.win.id() == id) else {
            return;
        };
        let wctx = &mut self.wins[i];
        let size = wctx.win.inner_size();
        let (sw, sh) = match (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            (Some(w), Some(h)) => (w, h),
            _ => return,
        };
        if wctx.surf.resize(sw, sh).is_err() {
            return;
        }
        let Ok(mut buf) = wctx.surf.buffer_mut() else { return };
        let (sw, sh) = (sw.get() as usize, sh.get() as usize);
        // canvas offset of this monitor; canvas coords vs window coords
        let (ox, oy) = (wctx.vrect.x - self.bounds.x, wctx.vrect.y - self.bounds.y);
        let (cw, ch) = (self.bounds.w as usize, self.bounds.h as usize);
        for y in 0..sh {
            let cy = oy + y as i32;
            if cy < 0 || cy >= ch as i32 {
                continue;
            }
            let cx0 = ox.max(0); // first canvas column visible in this window
            let wx0 = (cx0 - ox) as usize; // same column in window coords
            let n = (sw - wx0).min(cw - cx0 as usize);
            if n == 0 {
                continue;
            }
            buf[y * sw + wx0..y * sw + wx0 + n]
                .copy_from_slice(&self.dim[cy as usize * cw + cx0 as usize..cy as usize * cw + cx0 as usize + n]);
        }
        // undim the part of the selection inside this monitor + border
        if let Some(a) = self.anchor {
            let sel = Rect::from_drag(a, self.cursor);
            if let Some(local) = sel
                .intersection(&wctx.vrect)
                .map(|r| r.translated(-wctx.vrect.x, -wctx.vrect.y))
            {
                let (x0, y0) = (local.x.max(0) as usize, local.y.max(0) as usize);
                let x1 = (local.right() as usize).min(sw);
                let y1 = (local.bottom() as usize).min(sh);
                for y in y0..y1 {
                    let cy = oy + y as i32;
                    if cy < 0 || cy >= ch as i32 {
                        continue;
                    }
                    let cx = ox + x0 as i32;
                    if cx < 0 || cx >= cw as i32 {
                        continue;
                    }
                    let n = (x1 - x0).min(cw - cx as usize).min(sw - x0);
                    let src = cy as usize * cw + cx as usize;
                    buf[y * sw + x0..y * sw + x0 + n].copy_from_slice(&self.bright[src..src + n]);
                }
                // 2px white border around the local rect
                for i in 0..2i32 {
                    for x in local.x..local.right() {
                        for yy in [local.y + i, local.bottom() - 1 - i] {
                            if x >= 0 && yy >= 0 && (x as usize) < sw && (yy as usize) < sh {
                                buf[yy as usize * sw + x as usize] = 0xFFFFFF;
                            }
                        }
                    }
                    for y in local.y..local.bottom() {
                        for xx in [local.x + i, local.right() - 1 - i] {
                            if xx >= 0 && y >= 0 && (xx as usize) < sw && (y as usize) < sh {
                                buf[y as usize * sw + xx as usize] = 0xFFFFFF;
                            }
                        }
                    }
                }
            }
        }
        let _ = buf.present();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if !self.wins.is_empty() {
            return;
        }
        for m in el.available_monitors() {
            let attrs = Window::default_attributes()
                .with_title("quickshort")
                .with_fullscreen(Some(Fullscreen::Borderless(Some(m.clone()))))
                .with_decorations(false)
                .with_resizable(false)
                .with_active(true)
                .with_window_level(WindowLevel::AlwaysOnTop);
            let Ok(win) = el.create_window(attrs) else { continue };
            let win = Rc::new(win);
            // launched via global hotkey -> no activation token; nudge the
            // compositor so keyboard focus lands on us (Esc must reach us)
            win.request_user_attention(Some(winit::window::UserAttentionType::Critical));
            let surf = Context::new(win.clone()).and_then(|c| Surface::new(&c, win.clone()));
            if let Ok(surf) = surf {
                self.wins.push(WinCtx {
                    win,
                    surf,
                    vrect: Self::vrect_of(&m),
                });
            }
        }
        if self.wins.is_empty() {
            self.finish(el, None);
        } else {
            self.redraw_all();
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.done {
            return;
        }
        let Some(i) = self.wins.iter().position(|w| w.win.id() == id) else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => self.finish(el, None),
            WindowEvent::RedrawRequested => self.draw(id),
            WindowEvent::CursorMoved { position, .. } => {
                let v = self.wins[i].vrect;
                self.cursor = Point::new(
                    v.x + position.x.round() as i32,
                    v.y + position.y.round() as i32,
                );
                if self.anchor.is_some() {
                    self.dirty = true;
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => match state {
                ElementState::Pressed => {
                    // anchor-only change paints nothing yet; the first
                    // motion event will redraw — skip the sync fullscreen
                    // redraw here (it stalls the drag start)
                    self.anchor = Some(self.cursor);
                }
                ElementState::Released => {
                    let Some(a) = self.anchor.take() else { return };
                    let sel = Rect::from_drag(a, self.cursor);
                    self.finish(
                        el,
                        (sel.w >= MIN_SELECT && sel.h >= MIN_SELECT).then_some(sel),
                    );
                }
            },
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && matches!(event.logical_key, Key::Named(NamedKey::Escape)) =>
            {
                self.finish(el, None);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        // Coalesce bursts of motion events into one redraw per loop pass.
        if self.dirty && !self.done {
            self.dirty = false;
            self.redraw_all();
        }
    }
}
