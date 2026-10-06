//! Wayland backend (KDE Plasma / wlroots / non-GNOME): the standard
//! org.freedesktop.portal.ScreenCast portal.
//!
//! Flow: CreateSession -> SelectSources(monitors, persist) -> Start ->
//! each stream reports (pipewire_node_id, {position, size}) -> one frame
//! per node through the same PipeWire grabber the mutter backend uses.
//!
//! Portals answer via a `Response` signal on a Request object living at
//! a predictable path (request/<sender>/<token>) — pass our own
//! `handle_token` and subscribe BEFORE the call, or the response can
//! land first. `persist_mode=2` + a stored `restore_token` lets
//! compositors that support it (KDE) skip the consent dialog after the
//! first capture; where unsupported the dialog appears every time —
//! acceptable for a fallback path.

use anyhow::{bail, Context, Result};
use qs_core::{CapturedFrame, Rect};
use std::collections::HashMap;
use std::path::PathBuf;
use zbus::blocking::{Connection, Proxy};
use zbus::proxy::SignalStream;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use super::wayland::{grab_frames, mk_frame, DamagePoke};
use crate::ScreenCapture;

const DESKTOP: &str = "org.freedesktop.portal.Desktop";
const ROOT: &str = "/org/freedesktop/portal/desktop";
const CAST: &str = "org.freedesktop.portal.ScreenCast";
const SESSION: &str = "org.freedesktop.portal.Session";
const REQUEST: &str = "org.freedesktop.portal.Request";
/// user consent can take a while — don't cut it short
const RESPONSE_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

type Dict = HashMap<String, OwnedValue>;
type Opts = HashMap<String, Value<'static>>;

pub struct PortalCapture;

impl ScreenCapture for PortalCapture {
    fn capture_all(&self) -> Result<Vec<CapturedFrame>> {
        let conn = Connection::session().context("session dbus")?;
        let sc = Proxy::new(&conn, DESKTOP, ROOT, CAST)?;

        // --- CreateSession -------------------------------------------------
        let req_tok = token();
        let mut rx = response_rx(&conn, &request_path(&conn, &req_tok))?;
        let mut opts = Opts::new();
        opts.insert("handle_token".into(), vstr(&req_tok));
        opts.insert("session_handle_token".into(), vstr(&token()));
        let _: OwnedObjectPath = sc
            .call("CreateSession", &(opts,))
            .context("portal CreateSession")?;
        let results = wait_response(&mut rx, "CreateSession")?;
        let session_path: String = results
            .get("session_handle")
            .and_then(|v| String::try_from(v.clone()).ok())
            .context("CreateSession gave no session_handle")?;
        let session_obj =
            OwnedObjectPath::try_from(session_path.clone()).context("bad session_handle")?;

        // --- SelectSources --------------------------------------------------
        // returns a Request handle: subscribe first, then wait for its
        // Response like every other portal call
        let req_tok = token();
        let mut rx = response_rx(&conn, &request_path(&conn, &req_tok))?;
        let mut opts = Opts::new();
        opts.insert("handle_token".into(), vstr(&req_tok));
        opts.insert("types".into(), Value::from(1u32)); // monitors
        opts.insert("multiple".into(), Value::from(true));
        // a cursor_mode the portal doesn't advertise is a hard
        // InvalidArgument — GNOME reports 0 available modes, so omit it
        let modes: u32 = sc.get_property("AvailableCursorModes").unwrap_or(0);
        if modes != 0 {
            for m in [1u32, 2, 4] {
                if modes & m != 0 {
                    opts.insert("cursor_mode".into(), Value::from(m));
                    break;
                }
            }
        }
        opts.insert("persist_mode".into(), Value::from(2u32)); // until revoked
        if let Some(t) = load_restore_token() {
            opts.insert("restore_token".into(), vstr(&t));
        }
        let _: OwnedObjectPath = sc
            .call("SelectSources", &(session_obj.clone(), opts))
            .context("portal SelectSources")?;
        let _ = wait_response(&mut rx, "SelectSources")?;

        // --- Start ------------------------------------------------------------
        let req_tok = token();
        let mut rx = response_rx(&conn, &request_path(&conn, &req_tok))?;
        let mut opts = Opts::new();
        opts.insert("handle_token".into(), vstr(&req_tok));
        let _: OwnedObjectPath = sc
            .call("Start", &(session_obj, "", opts))
            .context("portal Start")?;
        let results = wait_response(&mut rx, "Start")?;

        if let Some(t) = results
            .get("restore_token")
            .and_then(|v| String::try_from(v.clone()).ok())
        {
            save_restore_token(&t);
        }
        let streams: Vec<(u32, Dict)> = Vec::try_from(
            results
                .get("streams")
                .context("Start gave no streams")?
                .clone(),
        )
        .context("parse streams")?;

        let mut nodes = Vec::with_capacity(streams.len());
        let mut logicals = Vec::with_capacity(streams.len());
        let mut centers = Vec::with_capacity(streams.len());
        for (node, props) in &streams {
            eprintln!("portal stream node={node} props={props:?}");
            let pos = props
                .get("position")
                .and_then(|v| {
                    <(i32, i32)>::try_from(v.clone())
                        .or_else(|_| {
                            <(u32, u32)>::try_from(v.clone()).map(|(a, b)| (a as i32, b as i32))
                        })
                        .ok()
                })
                .unwrap_or((0, 0));
            let size = props
                .get("size")
                .and_then(|v| {
                    <(u32, u32)>::try_from(v.clone())
                        .or_else(|_| {
                            <(i32, i32)>::try_from(v.clone()).map(|(a, b)| (a as u32, b as u32))
                        })
                        .ok()
                })
                .context("stream missing size")?;
            nodes.push(*node);
            logicals.push(Rect::new(pos.0, pos.1, size.0, size.1));
            centers.push((pos.0 + size.0 as i32 / 2, pos.1 + size.1 as i32 / 2));
        }
        if nodes.is_empty() {
            bail!("portal returned zero streams");
        }

        // same starvation quirk as mutter: producers may wait for damage
        let _poke = DamagePoke::start(centers);
        let expects: Vec<(u32, u32)> = logicals.iter().map(|r| (r.w, r.h)).collect();
        let imgs = grab_frames(&nodes, &expects)?;

        // best-effort cleanup — the compositor reaps the session with us anyway
        let _ = Proxy::new(&conn, DESKTOP, session_path.as_str(), SESSION)
            .and_then(|p| p.call::<_, _, ()>("Close", &()));

        let mut frames = Vec::with_capacity(imgs.len());
        for (img, rect) in imgs.into_iter().zip(logicals) {
            frames.push(mk_frame(img, rect));
        }
        Ok(frames)
    }
}

// ------------------------------------------------------------- helpers

fn token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "qs{nanos:x}{:x}",
        std::process::id() as u64 * 1000 + N.fetch_add(1, Ordering::Relaxed)
    )
}

fn vstr(s: &str) -> Value<'static> {
    Value::from(s.to_owned())
}

/// Request objects live at
/// `/org/freedesktop/portal/desktop/request/<sender>/<token>` where
/// `<sender>` is our unique name with `:` stripped and `.` -> `_`.
fn request_path(conn: &Connection, tok: &str) -> String {
    let sender = conn
        .unique_name()
        .map(|n| n.as_str().trim_start_matches(':').replace('.', "_"))
        .unwrap_or_default();
    format!("{ROOT}/request/{sender}/{tok}")
}

/// Subscribe to `Request::Response` at a predicted request path — must
/// happen BEFORE issuing the call the token belongs to.
fn response_rx(conn: &Connection, path: &str) -> Result<SignalStream<'static>> {
    Ok(async_io::block_on(async {
        let p = zbus::Proxy::new(conn.inner(), DESKTOP, path, REQUEST).await?;
        p.receive_signal("Response").await
    })?)
}

/// Block until the portal answers; response code 1 = user cancelled.
fn wait_response(rx: &mut SignalStream<'static>, what: &str) -> Result<Dict> {
    use futures_lite::StreamExt;
    let msg = async_io::block_on(futures_lite::future::or(
        async move { rx.next().await },
        async {
            async_io::Timer::after(RESPONSE_WAIT).await;
            None
        },
    ))
    .with_context(|| format!("portal {what}: no Response (timeout)"))?;
    let (code, results): (u32, Dict) = msg.body().deserialize()?;
    match code {
        0 => Ok(results),
        1 => bail!("portal {what}: user cancelled"),
        c => bail!("portal {what}: response code {c}"),
    }
}

fn token_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("quickshort/portal_restore_token"))
}

fn load_restore_token() -> Option<String> {
    std::fs::read_to_string(token_file()?)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn save_restore_token(t: &str) {
    if let Some(f) = token_file() {
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(f, t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real portal session — pops a consent dialog on desktops without
    // persist support. `QS_CAPTURE=portal` exercises this from --capture.
    #[test]
    #[ignore]
    fn portal_screencast_returns_frames() {
        let frames = PortalCapture.capture_all().unwrap();
        assert!(!frames.is_empty());
        for f in &frames {
            eprintln!(
                "frame: logical {}x{} @({},{}) scale {:.2}, pixels {}x{}",
                f.geometry.size.w,
                f.geometry.size.h,
                f.geometry.origin.x,
                f.geometry.origin.y,
                f.geometry.scale,
                f.pixels.width(),
                f.pixels.height()
            );
        }
    }
}
