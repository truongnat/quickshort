//! GNOME/Mutter: full-monitor **gtk-layer-shell** overlay (macOS-style —
//! stack in the corner).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use gtk::prelude::*;
use gtk_layer_shell::{Edge, Layer, LayerShell};

use super::Corner;

static TX: OnceLock<Sender<GtkMsg>> = OnceLock::new();

thread_local! {
    static RX: RefCell<Option<mpsc::Receiver<GtkMsg>>> = const { RefCell::new(None) };
    static SURFACES: RefCell<HashMap<u64, Surface>> = RefCell::new(HashMap::new());
}

/// Wire the album GTK commands (call once from the tray thread after `gtk::init`).
pub fn spawn_channel() {
    let (tx, rx) = mpsc::channel();
    let _ = TX.set(tx);
    RX.with(|r| *r.borrow_mut() = Some(rx));
}

pub fn drain_pending() {
    RX.with(|slot| {
        let rx = slot.borrow();
        let Some(rx) = rx.as_ref() else { return };
        while let Ok(m) = rx.try_recv() {
            if let Err(e) = dispatch(m) {
                eprintln!("chrome gtk-layer: {e:#}");
            }
        }
    });
}

enum GtkMsg {
    Create {
        lw: f64,
        lh: f64,
        scale: f64,
        mon_name: Option<String>,
        mon_x: i32,
        mon_y: i32,
        reply: mpsc::Sender<Result<(u64, f64, f64, f64)>>,
    },
    Present {
        id: u64,
        buf: Vec<u32>,
        pw: usize,
        ph: usize,
    },
    Destroy {
        id: u64,
    },
}

struct Surface {
    area: gtk::DrawingArea,
    pixels: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    pw: i32,
    ph: i32,
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn dispatch(msg: GtkMsg) -> Result<()> {
    match msg {
        GtkMsg::Create {
            lw,
            lh,
            scale,
            mon_name,
            mon_x,
            mon_y,
            reply,
        } => {
            let r = create_surface(lw, lh, scale, mon_name.as_deref(), mon_x, mon_y);
            let _ = reply.send(r);
        }
        GtkMsg::Present { id, buf, pw, ph } => present(id, buf, pw, ph)?,
        GtkMsg::Destroy { id } => {
            SURFACES.with(|m| {
                m.borrow_mut().remove(&id);
            });
        }
    }
    Ok(())
}

fn create_surface(
    lw: f64,
    lh: f64,
    scale: f64,
    mon_name: Option<&str>,
    mon_x: i32,
    mon_y: i32,
) -> Result<(u64, f64, f64, f64)> {
    if !gtk_layer_shell::is_supported() {
        anyhow::bail!(
            "gtk_layer_is_supported() false — GNOME gtk_shell1 needs a newer compositor binding; use winit fullscreen"
        );
    }
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_namespace("quickshort-album");
    window.set_keyboard_mode(gtk_layer_shell::KeyboardMode::OnDemand);
    for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
        window.set_anchor(edge, true);
        window.set_layer_shell_margin(edge, 0);
    }
    let mut lw = lw;
    let mut lh = lh;
    let mut scale = scale;
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
                window.set_monitor(&mon);
                lw = geo.width() as f64;
                lh = geo.height() as f64;
                scale = mon.scale_factor() as f64;
                break;
            }
        }
    }
    let pw = (lw * scale).round().max(1.0) as i32;
    let ph = (lh * scale).round().max(1.0) as i32;
    window.set_default_size(pw, ph);

    let pixels = std::sync::Arc::new(std::sync::Mutex::new(vec![0u8; pw as usize * ph as usize * 4]));
    let area = gtk::DrawingArea::new();
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
    window.add(&area);
    window.show_all();
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    SURFACES.with(|m| {
        m.borrow_mut().insert(
            id,
            Surface {
                area,
                pixels,
                pw,
                ph,
            },
        );
    });
    eprintln!("chrome: gtk-layer album docked ({}×{} logical)", lw, lh);
    Ok((id, lw, lh, scale))
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

fn present(id: u64, buf: Vec<u32>, pw: usize, ph: usize) -> Result<()> {
    SURFACES.with(|m| {
        let map = m.borrow();
        let Some(surf) = map.get(&id) else {
            return Ok(());
        };
        if buf.len() == pw * ph {
            *surf.pixels.lock().unwrap() = pack_pixels(&buf, pw, ph);
        }
        surf.area.queue_draw();
        Ok(())
    })
}

pub struct GtkLayer {
    id: u64,
    /// GDK monitor logical size — stack layout must match the gtk surface.
    pub lw: f64,
    pub lh: f64,
    pub scale: f64,
}

impl GtkLayer {
    pub fn new(
        _corner: Corner,
        lw: f64,
        lh: f64,
        scale: f64,
        mon_name: Option<String>,
        mon_x: i32,
        mon_y: i32,
    ) -> Result<(Self, f64, f64, f64)> {
        let tx = TX.get().context("gtk-layer channel")?;
        let (reply_tx, reply_rx) = mpsc::channel();
        tx.send(GtkMsg::Create {
            lw,
            lh,
            scale,
            mon_name,
            mon_x,
            mon_y,
            reply: reply_tx,
        })
        .context("gtk-layer send")?;
        crate::tray::linux_tray::wake();
        let (id, lw, lh, scale) = reply_rx
            .recv()
            .context("gtk-layer reply")?
            .context("gtk-layer create")?;
        Ok((Self { id, lw, lh, scale }, lw, lh, scale))
    }

    pub fn present(
        &mut self,
        buf: &[u32],
        pw: usize,
        ph: usize,
        _rects: &[super::FRect],
        _scale: f64,
        _dirty: Option<super::FRect>,
    ) {
        if let Some(tx) = TX.get() {
            let _ = tx.send(GtkMsg::Present {
                id: self.id,
                buf: buf.to_vec(),
                pw,
                ph,
            });
            crate::tray::linux_tray::wake();
        }
    }

    pub fn set_layer_corner(&mut self, _corner: Corner) {}
}

impl Drop for GtkLayer {
    fn drop(&mut self) {
        if let Some(tx) = TX.get() {
            let _ = tx.send(GtkMsg::Destroy { id: self.id });
            crate::tray::linux_tray::wake();
        }
    }
}
