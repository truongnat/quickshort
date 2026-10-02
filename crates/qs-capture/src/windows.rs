//! Windows backend — GDI BitBlt.
//!
//! The process opts into PER_MONITOR_AWARE_V2 at startup (see
//! [`crate::platform_init`]) so EnumDisplayMonitors reports each monitor's
//! rect in *physical* pixels; logical coords = rect / (GetDpiForMonitor / 96).
//! DXGI Desktop Duplication only if GDI proves too slow.

use std::mem::size_of;

use anyhow::{bail, Context, Result};
use image::RgbaImage;
use qs_core::{CapturedFrame, MonitorGeometry, Point, Size};
use windows::core::BOOL;
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, EnumDisplayMonitors,
    GetDC, GetDIBits, GetMonitorInfoW, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HDC, HGDIOBJ, HMONITOR, MONITORINFOEXW, SRCCOPY,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};

use crate::ScreenCapture;

pub struct GdiCapture;

/// Must run before any UI call; after this, GDI coords are physical pixels.
pub fn enable_dpi_awareness() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

struct Monitor {
    /// rect in virtual desktop coords — physical px under PMv2 awareness
    rect: RECT,
    /// dpi / 96
    scale: f64,
}

unsafe extern "system" fn enum_proc(
    hmon: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let out = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    if unsafe { GetMonitorInfoW(hmon, &mut info as *mut _ as *mut _) }.as_bool() {
        let (mut dx, mut dy) = (0u32, 0u32);
        let scale = match unsafe { GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) } {
            Ok(()) => dx as f64 / 96.0,
            Err(_) => 1.0,
        };
        out.push(Monitor {
            rect: info.monitorInfo.rcMonitor,
            scale,
        });
    }
    BOOL(1)
}

impl ScreenCapture for GdiCapture {
    fn capture_all(&self) -> Result<Vec<CapturedFrame>> {
        let mut monitors: Vec<Monitor> = Vec::new();
        unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(enum_proc),
                LPARAM(&raw mut monitors as isize),
            )
            .ok()
            .context("EnumDisplayMonitors")?;
            let screen = GetDC(None);
            if screen.is_invalid() {
                bail!("GetDC failed");
            }
            let r = capture_monitors(screen, &monitors);
            ReleaseDC(None, screen);
            r
        }
    }
}

fn capture_monitors(screen: HDC, monitors: &[Monitor]) -> Result<Vec<CapturedFrame>> {
    unsafe {
        let mem = CreateCompatibleDC(Some(screen));
        if mem.is_invalid() {
            bail!("CreateCompatibleDC failed");
        }
        let mut frames = Vec::with_capacity(monitors.len());
        for m in monitors {
            let w = (m.rect.right - m.rect.left) as u32;
            let h = (m.rect.bottom - m.rect.top) as u32;
            let bmp = CreateCompatibleBitmap(screen, w as i32, h as i32);
            let old = SelectObject(mem, HGDIOBJ(bmp.0));
            BitBlt(
                mem,
                0,
                0,
                w as i32,
                h as i32,
                Some(screen),
                m.rect.left,
                m.rect.top,
                SRCCOPY,
            )
            .ok()
            .context("BitBlt")?;

            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(h as i32), // negative -> top-down rows
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut data = vec![0u8; (w * h * 4) as usize];
            if GetDIBits(
                mem,
                bmp,
                0,
                h,
                Some(data.as_mut_ptr().cast()),
                &mut bmi,
                DIB_RGB_COLORS,
            ) == 0
            {
                bail!("GetDIBits failed");
            }
            SelectObject(mem, old);
            let _ = DeleteObject(HGDIOBJ(bmp.0));

            for px in data.chunks_exact_mut(4) {
                px.swap(0, 2); // BGRA -> RGBA
                px[3] = 255; // GDI leaves alpha zero
            }
            let s = m.scale;
            frames.push(CapturedFrame {
                geometry: MonitorGeometry {
                    origin: Point::new(
                        (m.rect.left as f64 / s).round() as i32,
                        (m.rect.top as f64 / s).round() as i32,
                    ),
                    size: Size::new(
                        (w as f64 / s).round() as u32,
                        (h as f64 / s).round() as u32,
                    ),
                    scale: s,
                },
                pixels: RgbaImage::from_raw(w, h, data).context("capture size mismatch")?,
            });
        }
        let _ = DeleteDC(mem);
        Ok(frames)
    }
}
