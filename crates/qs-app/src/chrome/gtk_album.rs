//! GNOME corner **shelf** — one small GTK utility window at the dock
//! corner (no fullscreen canvas). Fly-in is instant on this path; the
//! window is only shelf-sized so the rest of the desktop stays usable.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use gdk::prelude::*;
use gtk::prelude::*;
use winit::window::WindowId;

use super::{ChromeEv, Corner};

static TX: OnceLock<Sender<AlbumMsg>> = OnceLock::new();

static CHROME_PUSH: OnceLock<Arc<dyn Fn(WindowId, ChromeEv) + Send + Sync>> = OnceLock::new();

thread_local! {
    static RX: RefCell<Option<mpsc::Receiver<AlbumMsg>>> = const { RefCell::new(None) };
    static SURFACES: RefCell<HashMap<u64, AlbumSurface>> = RefCell::new(HashMap::new());
}

pub fn spawn_channel() {
    let (tx, rx) = mpsc::channel();
    let _ = TX.set(tx);
    RX.with(|r| *r.borrow_mut() = Some(rx));
}

/// Host → gtk thread: forward stack input into `HostEvent::Chrome`.
pub fn set_chrome_push(f: Arc<dyn Fn(WindowId, ChromeEv) + Send + Sync>) {
    let _ = CHROME_PUSH.set(f);
}

pub fn drain_pending() {
    RX.with(|slot| {
        let rx = slot.borrow();
        let Some(rx) = rx.as_ref() else { return };
        while let Ok(m) = rx.try_recv() {
            if let Err(e) = dispatch(m) {
                eprintln!("chrome gtk-album: {e:#}");
            }
        }
    });
}

enum AlbumMsg {
    Create {
        corner: Corner,
        sw: f64,
        sh: f64,
        scale: f64,
        mon_name: Option<String>,
        mon_x: i32,
        mon_y: i32,
        reply: mpsc::Sender<Result<(u64, f64, f64, f64)>>,
    },
    Bind {
        id: u64,
        wid: WindowId,
    },
    Present {
        id: u64,
        buf: Vec<u32>,
        pw: usize,
        ph: usize,
        visible: bool,
    },
    Destroy {
        id: u64,
    },
}

struct AlbumSurface {
    window: gtk::Window,
    area: gtk::DrawingArea,
    pixels: Arc<Mutex<Vec<u8>>>,
    scale: f64,
    wid: Option<WindowId>,
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn dispatch(msg: AlbumMsg) -> Result<()> {
    match msg {
        AlbumMsg::Create {
            corner,
            sw,
            sh,
            scale,
            mon_name,
            mon_x,
            mon_y,
            reply,
        } => {
            let r = create_window(corner, sw, sh, scale, mon_name.as_deref(), mon_x, mon_y);
            let _ = reply.send(r);
        }
        AlbumMsg::Bind { id, wid } => {
            SURFACES.with(|m| {
                if let Some(s) = m.borrow_mut().get_mut(&id) {
                    s.wid = Some(wid);
                }
            });
        }
        AlbumMsg::Present {
            id,
            buf,
            pw,
            ph,
            visible,
        } => present(id, buf, pw, ph, visible)?,
        AlbumMsg::Destroy { id } => {
            SURFACES.with(|m| {
                if let Some(s) = m.borrow_mut().remove(&id) {
                    s.window.close();
                }
            });
        }
    }
    Ok(())
}

fn shelf_pos_physical(
    geo: &gdk::Rectangle,
    corner: Corner,
    sw: f64,
    sh: f64,
    scale: f64,
) -> (i32, i32, i32, i32) {
    let side = (20.0 * scale).round() as i32;
    let top = side;
    let sw_p = (sw * scale).round() as i32;
    let sh_p = (sh * scale).round() as i32;
    let x = if corner.right() {
        geo.x() + geo.width() - sw_p - side
    } else if corner == Corner::TC || corner == Corner::BC {
        geo.x() + (geo.width() - sw_p) / 2
    } else {
        geo.x() + side
    };
    let y = if corner.bottom() {
        geo.y() + geo.height() - sh_p - side
    } else {
        geo.y() + top
    };
    (x, y, sw_p, sh_p)
}

fn push_ev(wid: WindowId, ev: ChromeEv) {
    if let Some(f) = CHROME_PUSH.get() {
        f(wid, ev);
    }
}

fn create_window(
    corner: Corner,
    sw: f64,
    sh: f64,
    scale: f64,
    mon_name: Option<&str>,
    mon_x: i32,
    mon_y: i32,
) -> Result<(u64, f64, f64, f64)> {
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_decorated(false);
    window.set_resizable(false);
    window.set_title("Quickshort");
    window.set_skip_taskbar_hint(true);
    window.set_skip_pager_hint(true);
    window.set_type_hint(gdk::WindowTypeHint::Utility);
    window.set_app_paintable(true);
    if let Some(screen) = gdk::Screen::default() {
        if let Some(visual) = screen.rgba_visual().or_else(|| screen.system_visual()) {
            window.set_visual(Some(&visual));
        }
    }

    let mut scale = scale;
    let mut pos = (0i32, 0i32, (sw * scale).round() as i32, (sh * scale).round() as i32);
    if let Some(display) = gdk::Display::default() {
        for i in 0..display.n_monitors() {
            let mon = display.monitor(i).context("monitor")?;
            let model = mon.model().map(|s| s.to_string());
            let geo = mon.geometry();
            let hit = mon_name
                .as_deref()
                .zip(model.as_deref())
                .is_some_and(|(a, b)| a == b)
                || mon_name.is_none() && geo.x() == mon_x && geo.y() == mon_y;
            if hit {
                scale = mon.scale_factor() as f64;
                pos = shelf_pos_physical(&geo, corner, sw, sh, scale);
                break;
            }
        }
    }
    let (px, py, pw, ph) = pos;

    let pixels = Arc::new(Mutex::new(vec![0u8; pw as usize * ph as usize * 4]));
    let area = gtk::DrawingArea::new();
    area.set_size_request(pw, ph);
    let px_draw = pixels.clone();
    let pw_d = pw;
    let ph_d = ph;
    area.connect_draw(move |_, cr| {
        let data = px_draw.lock().unwrap();
        use gtk::glib::Propagation;
        if data.len() < (pw_d * ph_d * 4) as usize {
            return Propagation::Proceed;
        }
        let stride = cairo::Format::ARgb32
            .stride_for_width(pw_d as u32)
            .unwrap_or(pw_d * 4);
        let copy = data.clone();
        drop(data);
        if let Ok(surface) = cairo::ImageSurface::create_for_data(
            copy,
            cairo::Format::ARgb32,
            pw_d,
            ph_d,
            stride,
        ) {
            cr.set_source_surface(surface, 0.0, 0.0).ok();
            cr.paint().ok();
        }
        Propagation::Proceed
    });

    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let scale_in = scale;
    area.add_events(
        gdk::EventMask::POINTER_MOTION_MASK
            | gdk::EventMask::BUTTON_PRESS_MASK
            | gdk::EventMask::BUTTON_RELEASE_MASK
            | gdk::EventMask::LEAVE_NOTIFY_MASK,
    );
    area.connect_motion_notify_event({
        let id = id;
        move |_, event| {
            let (x, y) = event.position();
            let x = x / scale_in;
            let y = y / scale_in;
            SURFACES.with(|m| {
                if let Some(wid) = m.borrow().get(&id).and_then(|s| s.wid) {
                    push_ev(wid, ChromeEv::Move { x, y });
                }
            });
            gtk::glib::Propagation::Proceed
        }
    });
    area.connect_leave_notify_event({
        let id = id;
        move |_, _| {
            SURFACES.with(|m| {
                if let Some(wid) = m.borrow().get(&id).and_then(|s| s.wid) {
                    push_ev(wid, ChromeEv::Leave);
                }
            });
            gtk::glib::Propagation::Proceed
        }
    });
    area.connect_button_press_event({
        let id = id;
        move |_, event| {
            let (x, y) = event.position();
            let x = x / scale_in;
            let y = y / scale_in;
            let btn = event.button();
            SURFACES.with(|m| {
                if let Some(wid) = m.borrow().get(&id).and_then(|s| s.wid) {
                    if btn == 1 {
                        push_ev(wid, ChromeEv::Down { x, y });
                    } else if btn == 3 {
                        push_ev(wid, ChromeEv::RightDown { x, y });
                    }
                }
            });
            gtk::glib::Propagation::Proceed
        }
    });
    area.connect_button_release_event({
        let id = id;
        move |_, event| {
            let (x, y) = event.position();
            let x = x / scale_in;
            let y = y / scale_in;
            if event.button() == 1 {
                SURFACES.with(|m| {
                    if let Some(wid) = m.borrow().get(&id).and_then(|s| s.wid) {
                        push_ev(wid, ChromeEv::Up { x, y });
                    }
                });
            }
            gtk::glib::Propagation::Proceed
        }
    });

    window.add(&area);
    window.set_default_size(pw, ph);
    window.move_(px, py);
    window.show_all();
    window.hide();
    SURFACES.with(|m| {
        m.borrow_mut().insert(
            id,
            AlbumSurface {
                window: window.clone(),
                area,
                pixels,
                scale,
                wid: None,
            },
        );
    });
    eprintln!(
        "chrome: gtk-album shelf at ({px},{py}) {pw}×{ph}px ({sw}×{sh} logical)"
    );
    Ok((id, sw, sh, scale))
}

fn pack_pixels(buf: &[u32], pw: usize, ph: usize) -> Vec<u8> {
    let mut out = vec![0u8; pw * ph * 4];
    for (i, p) in buf.iter().enumerate().take(pw * ph) {
        let o = i * 4;
        out[o] = (p & 0xff) as u8;
        out[o + 1] = ((p >> 8) & 0xff) as u8;
        out[o + 2] = ((p >> 16) & 0xff) as u8;
        out[o + 3] = ((p >> 24) & 0xff) as u8;
    }
    out
}

fn present(id: u64, buf: Vec<u32>, pw: usize, ph: usize, visible: bool) -> Result<()> {
    SURFACES.with(|m| {
        let map = m.borrow();
        let Some(surf) = map.get(&id) else {
            return Ok(());
        };
        if buf.len() == pw * ph {
            *surf.pixels.lock().unwrap() = pack_pixels(&buf, pw, ph);
        }
        surf.area.queue_draw();
        if visible {
            surf.window.show_all();
        } else {
            surf.window.hide();
        }
        Ok(())
    })
}

pub struct GtkAlbum {
    pub id: u64,
    pub lw: f64,
    pub lh: f64,
    pub scale: f64,
}

impl GtkAlbum {
    pub fn new(
        corner: Corner,
        sw: f64,
        sh: f64,
        scale: f64,
        mon_name: Option<String>,
        mon_x: i32,
        mon_y: i32,
    ) -> Result<Self> {
        let tx = TX.get().context("gtk-album channel")?;
        let (reply_tx, reply_rx) = mpsc::channel();
        tx.send(AlbumMsg::Create {
            corner,
            sw,
            sh,
            scale,
            mon_name,
            mon_x,
            mon_y,
            reply: reply_tx,
        })
        .context("gtk-album send")?;
        crate::tray::linux_tray::wake();
        let (id, lw, lh, scale) = reply_rx
            .recv()
            .context("gtk-album reply")?
            .context("gtk-album create")?;
        Ok(Self { id, lw, lh, scale })
    }

    pub fn bind_winit(&self, wid: WindowId) {
        if let Some(tx) = TX.get() {
            let _ = tx.send(AlbumMsg::Bind {
                id: self.id,
                wid,
            });
            crate::tray::linux_tray::wake();
        }
    }

    pub fn present(
        &mut self,
        buf: &[u32],
        pw: usize,
        ph: usize,
        _rects: &[super::FRect],
        _scale: f64,
        _dirty: Option<super::FRect>,
        visible: bool,
    ) {
        if let Some(tx) = TX.get() {
            let _ = tx.send(AlbumMsg::Present {
                id: self.id,
                buf: buf.to_vec(),
                pw,
                ph,
                visible,
            });
            crate::tray::linux_tray::wake();
        }
    }
}

impl Drop for GtkAlbum {
    fn drop(&mut self) {
        if let Some(tx) = TX.get() {
            let _ = tx.send(AlbumMsg::Destroy { id: self.id });
            crate::tray::linux_tray::wake();
        }
    }
}
