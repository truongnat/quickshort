//! X11 backend: monitor layout via RandR, pixels via GetImage on the root
//! window. X11 has no per-monitor DPI scaling (scale is always 1.0).

use anyhow::{Context, Result};
use image::RgbaImage;
use qs_core::{CapturedFrame, MonitorGeometry, Point, Size};
use x11rb::connection::Connection;
use x11rb::protocol::randr::{self, ConnectionExt as RandrExt};
use x11rb::protocol::xproto::{ConnectionExt as XprotoExt, ImageFormat};

use crate::ScreenCapture;

pub struct X11Capture;

#[derive(Clone)]
struct Mon {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
}

impl ScreenCapture for X11Capture {
    fn capture_all(&self) -> Result<Vec<CapturedFrame>> {
        let (conn, screen_num) = x11rb::connect(None).context("cannot connect to X11")?;
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let depth = screen.root_depth;

        // XWayland has no real framebuffer: GetImage on the root window always
        // fails BadMatch. Detect it (outputs are named XWAYLAND*) and fail
        // with a useful message instead of a raw protocol error.
        if is_xwayland(&conn, root) {
            anyhow::bail!(
                "connected to XWayland, which does not allow capturing the root window; \
                 run under a real X11 session (or use the Wayland backend once implemented)"
            );
        }

        // Authoritative drawable bounds. RandR may report rects outside the
        // root (compositors can lie about output sizes) -> GetImage BadMatch.
        let geo = conn.get_geometry(root)?.reply()?;
        let root_rect = Mon {
            x: 0,
            y: 0,
            w: geo.width as u32,
            h: geo.height as u32,
        };

        let mut monitors = randr_monitors(&conn, root)?
            .into_iter()
            .filter_map(|m| m.intersect(&root_rect))
            .collect::<Vec<_>>();
        if monitors.is_empty() {
            monitors.push(root_rect.clone());
        }

        let mut frames = Vec::with_capacity(monitors.len());
        for m in &monitors {
            match grab(&conn, root, m, depth) {
                Ok(f) => frames.push(f),
                Err(e) => {
                    // e.g. XWayland: RandR geometry != capturable area.
                    // Better one whole-root frame than a hard failure.
                    eprintln!("monitor {}x{} at ({},{}) failed: {e}; capturing whole root instead",
                              m.w, m.h, m.x, m.y);
                    return Ok(vec![grab(&conn, root, &root_rect, depth)
                        .context("whole-root capture failed")?]);
                }
            }
        }
        Ok(frames)
    }
}

fn grab(conn: &impl Connection, root: u32, m: &Mon, depth: u8) -> Result<CapturedFrame> {
    let reply = conn
        .get_image(
            ImageFormat::Z_PIXMAP,
            root,
            m.x as i16,
            m.y as i16,
            m.w as u16,
            m.h as u16,
            u32::MAX,
        )?
        .reply()?;
    Ok(CapturedFrame {
        geometry: MonitorGeometry {
            origin: Point::new(m.x, m.y),
            size: Size::new(m.w, m.h),
            scale: 1.0,
        },
        pixels: to_rgba(&reply.data, m.w, m.h, depth)?,
    })
}

impl Mon {
    fn intersect(&self, o: &Mon) -> Option<Mon> {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = (self.x + self.w as i32).min(o.x + o.w as i32);
        let b = (self.y + self.h as i32).min(o.y + o.h as i32);
        (r > x && b > y).then(|| Mon { x, y, w: (r - x) as u32, h: (b - y) as u32 })
    }
}

/// True when this X connection is really XWayland. Two signals: a Wayland
/// session is active (WAYLAND_DISPLAY set), or every connected RandR output
/// is an XWayland pseudo-output (mutter/kwin may rename them to real
/// connector names, so the env check is the reliable one).
fn is_xwayland(conn: &impl Connection, root: u32) -> bool {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return true;
    }
    let Ok(cookie) = conn.randr_get_screen_resources_current(root) else {
        return false;
    };
    let Ok(res) = cookie.reply() else {
        return false;
    };
    let mut connected = 0usize;
    let mut xwayland = 0usize;
    for &out in res.outputs.iter().take(8) {
        let Ok(cookie) = conn.randr_get_output_info(out, res.timestamp) else {
            continue;
        };
        let Ok(info) = cookie.reply() else {
            continue;
        };
        if info.connection != randr::Connection::CONNECTED {
            continue;
        }
        connected += 1;
        if info.name.starts_with(b"XWAYLAND") {
            xwayland += 1;
        }
    }
    connected > 0 && xwayland == connected
}

/// RandR 1.5 monitor list (gives virtual coords directly). Falls back to the
/// union of active CRTCs on older servers.
fn randr_monitors(conn: &impl Connection, root: u32) -> Result<Vec<Mon>> {
    if let Ok(reply) = conn.randr_get_monitors(root, true)?.reply() {
        let mons = reply
            .monitors
            .iter()
            .map(|m| Mon {
                x: m.x as i32,
                y: m.y as i32,
                w: m.width as u32,
                h: m.height as u32,
            })
            .collect::<Vec<_>>();
        if !mons.is_empty() {
            return Ok(mons);
        }
    }

    let res = conn.randr_get_screen_resources_current(root)?.reply()?;
    let mut out = Vec::new();
    for &crtc in &res.crtcs {
        let info = conn.randr_get_crtc_info(crtc, res.timestamp)?.reply()?;
        if info.width > 0 && info.height > 0 {
            out.push(Mon {
                x: info.x as i32,
                y: info.y as i32,
                w: info.width as u32,
                h: info.height as u32,
            });
        }
    }
    Ok(out)
}

/// Z_PIXMAP at depth 24/32 arrives as little-endian BGRX/BGRA.
fn to_rgba(data: &[u8], w: u32, h: u32, depth: u8) -> Result<RgbaImage> {
    let px = (w * h) as usize;
    match depth {
        24 | 32 if data.len() >= px * 4 => {
            let mut out = RgbaImage::new(w, h);
            for (dst, src) in out.pixels_mut().zip(data.chunks_exact(4)) {
                *dst = image::Rgba([src[2], src[1], src[0], 255]);
            }
            Ok(out)
        }
        16 if data.len() >= px * 2 => {
            // RGB565
            let mut out = RgbaImage::new(w, h);
            for (dst, src) in out.pixels_mut().zip(data.chunks_exact(2)) {
                let v = u16::from_le_bytes([src[0], src[1]]);
                let r = ((v >> 11) & 0x1f) as u8;
                let g = ((v >> 5) & 0x3f) as u8;
                let b = (v & 0x1f) as u8;
                *dst = image::Rgba([r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2, 255]);
            }
            Ok(out)
        }
        _ => anyhow::bail!("unsupported root depth {depth} ({} bytes)", data.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Needs a live X server: cargo test -p qs-capture -- --ignored
    #[test]
    #[ignore]
    fn captures_frames_matching_reported_geometry() {
        match X11Capture.capture_all() {
            Ok(frames) => {
                assert!(!frames.is_empty());
                for f in &frames {
                    assert_eq!(f.pixels.width(), f.geometry.size.w);
                    assert_eq!(f.pixels.height(), f.geometry.size.h);
                }
            }
            // XWayland (Wayland session) must fail with a clear error
            Err(e) if std::env::var_os("WAYLAND_DISPLAY").is_some() => {
                assert!(e.to_string().contains("XWayland"), "unexpected error: {e}");
            }
            Err(e) => panic!("real X11 capture failed: {e}"),
        }
    }
}
