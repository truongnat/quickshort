//! Chrome stub for platforms without a backend yet — `land` declines and
//! the host falls back to the quick-access card. Windows/X11/Wayland
//! backends slot in next to this file.

use super::{Chrome, ChromeCmd, ChromeEv, Land, Stack};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

/// Never constructed — `land` bails before creating a window.
pub struct ChromeWin {
    _uninhabited: core::convert::Infallible,
}

impl ChromeWin {
    pub fn id(&self) -> WindowId {
        unreachable!()
    }
    pub fn stack(&self) -> &Stack {
        unreachable!()
    }
    pub fn stack_mut(&mut self) -> &mut Stack {
        unreachable!()
    }
    pub fn event(&mut self, _ev: ChromeEv) -> Option<ChromeCmd> {
        None
    }
    pub fn redraw(&mut self) {}
    pub fn window_event(&mut self, _ev: &WindowEvent) -> bool {
        false
    }
    pub fn pump(&mut self) -> Option<std::time::Instant> {
        None
    }
    pub fn view_ptr(&self) -> *const std::ffi::c_void {
        unreachable!()
    }
}

pub fn land(_chrome: &mut Chrome, _el: &ActiveEventLoop, l: Land) -> Result<(), Land> {
    Err(l)
}

pub fn standalone(_pos: &str, _accent: u32) -> anyhow::Result<()> {
    anyhow::bail!("--chrome demo is implemented on macOS only, so far")
}
