//! `org.freedesktop.Notifications` over the session D-Bus — the same API
//! `notify-send` uses, without spawning a subprocess or a winit window.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::Context;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::Value;

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const IFACE: &str = "org.freedesktop.Notifications";
const APP: &str = "quickshort";
/// Reuse one slot so rapid saves replace instead of stacking.
const REPLACES_ID: u32 = 0x71_71_71;

static BUS: std::sync::OnceLock<Result<Connection, String>> = std::sync::OnceLock::new();

fn session() -> anyhow::Result<&'static Connection> {
    BUS.get_or_init(|| Connection::session().map_err(|e| format!("{e:#}")))
        .as_ref()
        .map_err(|e| anyhow::anyhow!("session dbus: {e}"))
}

fn send_sync(msg: &str, ok: bool) -> anyhow::Result<()> {
    let conn = session()?;
    let proxy = Proxy::new(conn, SERVICE, PATH, IFACE)?;
    let mut hints: HashMap<&str, Value> = HashMap::new();
    hints.insert("urgency", Value::from(if ok { 0u8 } else { 2u8 }));
    hints.insert("desktop-entry", Value::from("quickshort"));
    hints.insert("category", Value::from("image-added"));
    let _: u32 = proxy
        .call(
            "Notify",
            &(
                APP,
                REPLACES_ID,
                "",
                "Quickshort",
                msg,
                Vec::<&str>::new(),
                hints,
                4000i32,
            ),
        )
        .context("Notify")?;
    Ok(())
}

static IN_FLIGHT: AtomicU32 = AtomicU32::new(0);

/// Fire-and-forget; safe from any thread.
pub fn send(msg: &str, ok: bool) {
    let msg = msg.to_string();
    IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
    std::thread::spawn(move || {
        if let Err(e) = send_sync(&msg, ok) {
            eprintln!("notification (freedesktop): {e:#}");
        }
        IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
    });
}
