//! Unix-socket relay so compositor hotkeys (`quickshort --capture`) can wake
//! the resident tray app instead of starting a one-shot process. GNOME
//! gsettings, sway, and Hyprland all spawn the CLI; KDE already delivers
//! hotkeys in-process via the GlobalShortcuts portal.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

use winit::event_loop::EventLoopProxy;

use crate::host::HostEvent;

const REQ_REGION: u8 = b'r';
const REQ_FULL: u8 = b'f';
const ACK: u8 = b'o';

#[derive(Clone, Copy)]
pub enum Request {
    Region,
    Full,
}

pub fn socket_path() -> PathBuf {
    dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("quickshort")
        .join("ipc.sock")
}

/// Ask the running resident app to capture. Returns `true` when the request
/// was delivered (the CLI should exit immediately).
pub fn try_delegate(req: Request) -> bool {
    let path = socket_path();
    for attempt in 0..8 {
        match UnixStream::connect(&path) {
            Ok(mut s) => {
                let byte = match req {
                    Request::Region => REQ_REGION,
                    Request::Full => REQ_FULL,
                };
                if s.write_all(&[byte]).is_err() {
                    return false;
                }
                let _ = s.set_read_timeout(Some(Duration::from_millis(800)));
                let mut buf = [0u8; 1];
                return s.read_exact(&mut buf).is_ok() && buf[0] == ACK;
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    || e.kind() == std::io::ErrorKind::ConnectionRefused =>
            {
                if attempt + 1 < 8 {
                    // resident may still be binding the socket on startup
                    std::thread::sleep(Duration::from_millis(40));
                    continue;
                }
                return false;
            }
            Err(_) => return false,
        }
    }
    false
}

/// Listen for `--capture` / `--capture-full` relays from hotkey spawns.
pub fn spawn_listener(proxy: EventLoopProxy<HostEvent>) -> std::io::Result<()> {
    let path = socket_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::thread::Builder::new()
        .name("qs-ipc".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else {
                    break;
                };
                let mut byte = [0u8; 1];
                if s.read_exact(&mut byte).is_err() {
                    continue;
                }
                let ev = match byte[0] {
                    REQ_FULL => HostEvent::TriggerFull,
                    _ => HostEvent::Trigger,
                };
                if proxy.send_event(ev).is_ok() {
                    let _ = s.write_all(&[ACK]);
                }
            }
        })?;
    Ok(())
}
