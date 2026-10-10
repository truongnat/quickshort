//! Chrome stub for platforms without a backend yet — `new` bails so the
//! shared `land` hands the `Land` back and the host falls back to the
//! quick-access card.

use super::{ChromeCmd, ChromeEv, Stack};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

/// Never constructed — `new` bails before creating a window.
pub struct ChromeWin {
    _uninhabited: core::convert::Infallible,
}

impl ChromeWin {
    /// Lets the shared `land` path fail like any other backend —
    /// `Chrome::land` hands the `Land` back and the host opens the card.
    pub(super) fn new(
        _el: &ActiveEventLoop,
        _mon: &winit::monitor::MonitorHandle,
        _corner: super::Corner,
        _font: std::rc::Rc<fontdue::Font>,
        _accent: u32,
    ) -> anyhow::Result<Self> {
        anyhow::bail!("no chrome backend on this platform")
    }

    pub fn id(&self) -> WindowId {
        unreachable!()
    }
    pub(super) fn mon_key(&self) -> &str {
        unreachable!()
    }
    pub(super) fn set_sink(&self, _sink: std::rc::Rc<dyn Fn(ChromeEv)>) {}
    pub(super) fn land(&mut self, _l: super::Land) {
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
}
