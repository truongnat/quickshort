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
#[cfg(not(target_os = "macos"))]
use winit::window::Fullscreen;
use winit::window::{Window, WindowId, WindowLevel};

#[cfg(target_os = "macos")]
mod macos;

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
        // Surface coords are *physical* px; canvas/virtual coords are *logical*
        // px — scale_factor converts between them (2.0 on Retina).
        let scale = wctx.win.scale_factor();
        // canvas offset of this monitor; canvas coords vs window coords
        let (ox, oy) = (wctx.vrect.x - self.bounds.x, wctx.vrect.y - self.bounds.y);
        let (cw, ch) = (self.bounds.w as i32, self.bounds.h as i32);
        // physical (x,y) -> canvas (cx, cy); precompute per-row/col mapping
        let colmap: Vec<i32> = (0..sw).map(|x| ox + (x as f64 / scale) as i32).collect();
        let rowmap: Vec<i32> = (0..sh).map(|y| oy + (y as f64 / scale) as i32).collect();
        // Expand each canvas row to physical width once, then memcpy it into
        // every physical row that maps to it (consecutive rows share cy).
        let mut rowbuf: Vec<u32> = Vec::new();
        let mut rowbuf_cy = -1i32;
        for y in 0..sh {
            let cy = rowmap[y];
            if !(0..ch).contains(&cy) {
                continue;
            }
            if cy != rowbuf_cy {
                rowbuf.clear();
                rowbuf.reserve(sw);
                let src = cy as usize * cw as usize;
                for &cx in &colmap {
                    rowbuf.push(if (0..cw).contains(&cx) {
                        self.dim[src + cx as usize]
                    } else {
                        0
                    });
                }
                rowbuf_cy = cy;
            }
            buf[y * sw..y * sw + sw].copy_from_slice(&rowbuf);
        }
        // undim the part of the selection inside this monitor + border
        if let Some(a) = self.anchor {
            let sel = Rect::from_drag(a, self.cursor);
            if let Some(local) = sel
                .intersection(&wctx.vrect)
                .map(|r| r.translated(-wctx.vrect.x, -wctx.vrect.y))
            {
                // selection rect is logical; scale to physical px for drawing
                let x0 = (local.x as f64 * scale).max(0.0) as usize;
                let y0 = (local.y as f64 * scale).max(0.0) as usize;
                let x1 = ((local.right() as f64 * scale) as usize).min(sw);
                let y1 = ((local.bottom() as f64 * scale) as usize).min(sh);
                // same trick: expand the canvas slice once per canvas row
                let mut srow: Vec<u32> = Vec::new();
                let mut srow_cy = -1i32;
                for y in y0..y1 {
                    let cy = rowmap[y];
                    if !(0..ch).contains(&cy) {
                        continue;
                    }
                    if cy != srow_cy {
                        srow.clear();
                        srow.reserve(x1 - x0);
                        let src = cy as usize * cw as usize;
                        for &cx in &colmap[x0..x1] {
                            srow.push(if (0..cw).contains(&cx) {
                                self.bright[src + cx as usize]
                            } else {
                                0
                            });
                        }
                        srow_cy = cy;
                    }
                    buf[y * sw + x0..y * sw + x1].copy_from_slice(&srow);
                }
                // 2 logical px white border around the local rect
                let b = (2.0 * scale).round().max(1.0) as i32;
                let (lx0, ly0) = (
                    (local.x as f64 * scale) as i32,
                    (local.y as f64 * scale) as i32,
                );
                let (lx1, ly1) = (
                    (local.right() as f64 * scale) as i32,
                    (local.bottom() as f64 * scale) as i32,
                );
                for i in 0..b {
                    for x in lx0..lx1 {
                        for yy in [ly0 + i, ly1 - 1 - i] {
                            if x >= 0 && yy >= 0 && (x as usize) < sw && (yy as usize) < sh {
                                buf[yy as usize * sw + x as usize] = 0xFFFFFF;
                            }
                        }
                    }
                    for y in ly0..ly1 {
                        for xx in [lx0 + i, lx1 - 1 - i] {
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
                .with_decorations(false)
                .with_resizable(false)
                .with_active(true)
                .with_window_level(WindowLevel::AlwaysOnTop);
            // macOS: Borderless fullscreen enters a *new Space* — wrong for an
            // overlay. Cover the monitor with a plain borderless window whose
            // level/frame are set post-creation instead.
            #[cfg(target_os = "macos")]
            let attrs = attrs
                .with_position(m.position())
                .with_inner_size(m.size());
            #[cfg(not(target_os = "macos"))]
            let attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(Some(m.clone()))));
            let win = match el.create_window(attrs) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("overlay window creation failed: {e}");
                    continue;
                }
            };
            let win = Rc::new(win);
            #[cfg(target_os = "macos")]
            macos::prepare_overlay_window(&win, &m);
            // launched via global hotkey -> no activation token; nudge the
            // compositor so keyboard focus lands on us (Esc must reach us)
            win.request_user_attention(Some(winit::window::UserAttentionType::Critical));
            let surf = Context::new(win.clone()).and_then(|c| Surface::new(&c, win.clone()));
            match surf {
                Ok(surf) => self.wins.push(WinCtx {
                    win,
                    surf,
                    vrect: Self::vrect_of(&m),
                }),
                Err(e) => eprintln!("overlay surface creation failed: {e}"),
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
                // position is physical px; virtual coords are logical
                let s = self.wins[i].win.scale_factor();
                let v = self.wins[i].vrect;
                self.cursor = Point::new(
                    v.x + (position.x / s).round() as i32,
                    v.y + (position.y / s).round() as i32,
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
