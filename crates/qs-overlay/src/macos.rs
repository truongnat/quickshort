//! macOS overlay specifics. `Fullscreen::Borderless` would transition each
//! window into a *new Space* — useless for a screen-region picker. Instead a
//! plain borderless window is stretched over the monitor and lifted above the
//! menu bar via NSWindow level.

use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSScreen, NSScreenSaverWindowLevel, NSView, NSWindow, NSWindowCollectionBehavior,
};
use objc2_foundation::NSString;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::monitor::MonitorHandle;
use winit::platform::macos::MonitorHandleExtMacOS;
use winit::window::Window;

pub fn prepare_overlay_window(win: &Window, m: &MonitorHandle) {
    let Ok(handle) = win.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return;
    };
    let ns_view: &NSView = unsafe { &*(h.ns_view.as_ptr() as *const NSView) };
    let Some(ns_win) = ns_view.window() else {
        return;
    };
    let ns_win: &NSWindow = &ns_win;

    ns_win.setLevel(NSScreenSaverWindowLevel);
    ns_win.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary,
    );
    ns_win.setHasShadow(false);

    // Snap the frame to the matching NSScreen rect (AppKit coords, bottom-left
    // origin) so the window covers the menu bar as well.
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    for screen in NSScreen::screens(mtm).iter() {
        let desc = screen.deviceDescription();
        let key = NSString::from_str("NSScreenNumber");
        let id = desc.objectForKey(&key).and_then(|o| {
            o.downcast_ref::<objc2_foundation::NSNumber>()
                .map(|n| n.as_u32())
        });
        if id == Some(m.native_id()) {
            ns_win.setFrame_display(screen.frame(), true);
            break;
        }
    }
}
