//! Single-instance IPC over a Unix socket: `quickshort --capture`
//! forwards to the resident host when one is alive, so the GNOME
//! keybinding always lands in the flow that owns the tray — recording
//! indicator, result card and menus. No socket → caller runs standalone.

use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

/// XDG_RUNTIME_DIR is per-user and dies with the session, so the socket
/// is both private and self-cleaning.
fn sock_path() -> Result<PathBuf> {
    let dir = std::env::var("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR unset")?;
    Ok(PathBuf::from(dir).join("quickshort.sock"))
}

/// CLI side: send a one-word command ("capture", "gallery") to the
/// resident host. `true` = the host took it → caller may exit.
pub fn delegate(cmd: &str) -> bool {
    let Ok(path) = sock_path() else {
        return false;
    };
    let Ok(mut s) = UnixStream::connect(&path) else {
        return false;
    };
    writeln!(s, "{cmd}").is_ok()
}

/// Resident side: accept command lines forever, each handed to
/// `on_cmd()` on the listener thread. Dies with the process.
pub fn listen<F: Fn(String) + Send + 'static>(on_cmd: F) {
    let Ok(path) = sock_path() else {
        return;
    };
    let _ = std::fs::remove_file(&path); // stale socket from a dead host
    let Ok(listener) = UnixListener::bind(&path) else {
        return;
    };
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            if let Some(Ok(cmd)) = BufReader::new(conn).lines().next() {
                on_cmd(cmd);
            }
        }
    });
}
