//! Shared softbuffer widgets for the small always-on-top windows
//! (toast, quick-access card, recording stop). fontdue rasterization
//! and the card/pill SDF shapes live here once — every popup draws
//! itself on top of these.

use fontdue::Font;
use qs_overlay::sdf::{fill_cov, rrect_sdf, stroke_cov, Canvas};
use winit::dpi::PhysicalPosition;
use winit::monitor::MonitorHandle;

/// The monitor popups anchor to — the primary display. winit doesn't
/// expose the global cursor position at window-creation time, so this
/// is the stable, predictable choice.
pub fn popup_monitor(el: &winit::event_loop::ActiveEventLoop) -> Option<MonitorHandle> {
    el.primary_monitor()
        .or_else(|| el.available_monitors().next())
}

/// Physical-px outer position that docks a `w`×`h` logical popup at
/// `pos` (a key from `crate::config::POPUP_POS`) inside monitor `m`.
/// Unknown keys fall back to top-right.
pub fn dock_pos(pos: &str, m: &MonitorHandle, w: f64, h: f64) -> PhysicalPosition<f64> {
    let pos = if crate::config::POPUP_POS.iter().any(|(k, _)| *k == pos) {
        pos
    } else {
        "top-right"
    };
    let sf = m.scale_factor();
    let mp = m.position();
    let ms = m.size();
    let side = 20.0 * sf;
    // macOS keeps a menu bar pinned to the top edge of every screen
    let top = if cfg!(target_os = "macos") {
        44.0 * sf
    } else {
        20.0 * sf
    };
    let x = if pos.ends_with("left") {
        mp.x as f64 + side
    } else if pos.ends_with("right") {
        mp.x as f64 + ms.width as f64 - w * sf - side
    } else {
        mp.x as f64 + (ms.width as f64 - w * sf) / 2.0
    };
    let y = if pos.starts_with("top") {
        mp.y as f64 + top
    } else {
        mp.y as f64 + ms.height as f64 - h * sf - side
    };
    PhysicalPosition::new(x, y)
}

/// Clip a borderless popup window to its rounded-card shape. softbuffer
/// presents an opaque CGImage on macOS (alpha byte skipped), so without
/// a layer mask the space outside the card's rounded corners renders as
/// a solid black rectangle behind it.
#[cfg(target_os = "macos")]
pub fn round_window(win: &winit::window::Window, radius: f64) {
    use objc2_app_kit::NSView;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(h) = win.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(h) = h.as_raw() else {
        return;
    };
    let view: &NSView = unsafe { &*(h.ns_view.as_ptr() as *const NSView) };
    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        layer.setCornerRadius(radius);
        layer.setMasksToBounds(true);
    }
}

/// Same clip on Windows: softbuffer's DIB is opaque too, so the window
/// region is rounded with SetWindowRgn instead.
#[cfg(target_os = "windows")]
pub fn round_window(win: &winit::window::Window, radius: f64) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};
    let Ok(h) = win.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = h.as_raw() else {
        return;
    };
    let hwnd = HWND(h.hwnd.get() as *mut _);
    let size = win.inner_size();
    let r = (radius * win.scale_factor()).round() as i32;
    unsafe {
        let rgn = CreateRoundRectRgn(
            0,
            0,
            size.width as i32 + 1,
            size.height as i32 + 1,
            r * 2,
            r * 2,
        );
        // SetWindowRgn takes ownership of the region — never delete it
        let _ = SetWindowRgn(hwnd, Some(rgn), true);
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn round_window(_win: &winit::window::Window, _radius: f64) {}

/// Text rasterizer: `x`/`y` in logical px, `y` = glyph baseline.
pub struct Txt<'a> {
    pub font: &'a Font,
    pub scale: f64,
}

impl Txt<'_> {
    pub fn draw(&self, cv: &mut Canvas, x: f64, y: f64, size: f64, s: &str, rgb: u32) {
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

    pub fn width(&self, size: f64, s: &str) -> f64 {
        let px = (size * self.scale) as f32;
        s.chars()
            .map(|c| self.font.metrics(c, px).advance_width)
            .sum::<f32>() as f64
            / self.scale
    }
}

/// Window-sized rounded card: fill + rim inside, soft fringe outside
/// (stands in for the drop shadow a transparent window can't show —
/// softbuffer ignores per-pixel alpha, so the whole surface is painted).
pub fn card_chrome(cv: &mut Canvas, w: f64, h: f64, rad: f64, fill: u32, border: u32, fringe: u32) {
    let scale = cv.scale;
    for py in 0..cv.h as i32 {
        for px in 0..cv.w as i32 {
            let d = rrect_sdf(
                px as f64 / scale,
                py as f64 / scale,
                w / 2.0,
                h / 2.0,
                w / 2.0,
                h / 2.0,
                rad,
            );
            if d > 0.0 {
                cv.blend(px, py, fringe, 0.35 * (1.0 - d / 2.5).clamp(0.0, 1.0));
                continue;
            }
            cv.blend(px, py, fill, fill_cov(d, scale));
            let rim = stroke_cov(d, 0.7, scale);
            if rim > 0.0 {
                cv.blend(px, py, border, rim);
            }
        }
    }
}

/// Rounded button background + rim. `alpha` dims the fill (disabled).
pub fn pill(
    cv: &mut Canvas,
    rect: (f64, f64, f64, f64),
    rad: f64,
    bg: u32,
    border: u32,
    alpha: f64,
) {
    let (bx, by, bw, bh) = rect;
    let scale = cv.scale;
    for py in (by * scale) as i32..((by + bh) * scale) as i32 {
        for px in (bx * scale) as i32..((bx + bw) * scale) as i32 {
            let d = rrect_sdf(
                px as f64 / scale,
                py as f64 / scale,
                bx + bw / 2.0,
                by + bh / 2.0,
                bw / 2.0,
                bh / 2.0,
                rad,
            );
            if d <= 0.0 {
                cv.blend(px, py, bg, fill_cov(d, scale) * alpha);
                let rim = stroke_cov(d, 0.7, scale);
                if rim > 0.0 {
                    cv.blend(px, py, border, rim);
                }
            }
        }
    }
}
