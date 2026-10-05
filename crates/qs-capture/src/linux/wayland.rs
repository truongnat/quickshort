//! Wayland backend (GNOME/mutter): org.gnome.Mutter.ScreenCast.
//!
//! Private but dialog-free API (the same one the portal backend uses):
//! CreateSession -> RecordMonitor per connector -> Start -> each stream
//! exposes a PipeWireNodeId -> grab one frame by connecting to the PipeWire
//! node directly (in-process, shm buffers). Monitor layout in logical coords comes
//! from org.gnome.Mutter.DisplayConfig. Session objects are bound to our
//! dbus connection lifetime, so everything stays on one `Connection`.

use anyhow::{bail, Context, Result};
use pipewire as pw;
use pw::spa;
use qs_core::{CapturedFrame, MonitorGeometry, Point, Rect, Size};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::ScreenCapture;

const BUS: &str = "org.gnome.Mutter.ScreenCast";
const ROOT: &str = "/org/gnome/Mutter/ScreenCast";
const CFG: &str = "org.gnome.Mutter.DisplayConfig";
const CFG_PATH: &str = "/org/gnome/Mutter/DisplayConfig";

type Dict = HashMap<String, OwnedValue>;

pub struct MutterCapture;

impl ScreenCapture for MutterCapture {
    fn capture_all(&self) -> Result<Vec<CapturedFrame>> {
        let conn = Connection::session().context("session dbus")?;
        let mons = display_config(&conn)?;
        if mons.is_empty() {
            bail!("DisplayConfig reports no logical monitors");
        }

        let sc = Proxy::new(&conn, BUS, ROOT, BUS)?;
        let session: OwnedObjectPath = sc.call("CreateSession", &(empty_opts(),)).context(
            "CreateSession failed (screen locked? mutter inhibits screencast while locked)",
        )?;

        let r = run_session(&conn, &session, &mons);
        let _ = Proxy::new(
            &conn,
            BUS,
            session.as_str(),
            "org.gnome.Mutter.ScreenCast.Session",
        )
        .and_then(|p| p.call::<_, _, ()>("Stop", &()));
        r
    }
}

fn empty_opts() -> HashMap<String, Value<'static>> {
    HashMap::new()
}

fn run_session(
    conn: &Connection,
    session: &OwnedObjectPath,
    mons: &[Mon],
) -> Result<Vec<CapturedFrame>> {
    let sess = Proxy::new(
        conn,
        BUS,
        session.as_str(),
        "org.gnome.Mutter.ScreenCast.Session",
    )?;

    // RecordMonitor per connector; each stream object then exposes its
    // PipeWire node id as a property.
    let mut streams = Vec::with_capacity(mons.len());
    for m in mons {
        let stream_path: OwnedObjectPath = sess.call(
            "RecordMonitor",
            &(
                m.connector.as_str(),
                opts(&[("cursor-mode", Value::from(0u32))]), // no cursor
            ),
        )?;
        streams.push((m, stream_path));
    }

    // The PipeWire node id is emitted per stream object as
    // PipeWireStreamAdded(u node_id), unicast to us, right after Start().
    // Subscribe via the async half of this same connection — the blocking
    // SignalIterator has no timeout and a mute compositor would hang the
    // capture forever.
    let mut receivers = Vec::with_capacity(streams.len());
    for (_, spath) in &streams {
        receivers.push(async_io::block_on(async {
            let sp = zbus::Proxy::new(
                conn.inner(),
                BUS,
                spath.as_str(),
                "org.gnome.Mutter.ScreenCast.Stream",
            )
            .await?;
            sp.receive_signal("PipeWireStreamAdded").await
        })?);
    }
    sess.call::<_, _, ()>("Start", &())?;

    let mut nodes = Vec::with_capacity(streams.len());
    for ((m, _), mut rx) in streams.iter().zip(receivers) {
        use futures_lite::StreamExt;
        let msg = async_io::block_on(futures_lite::future::or(
            async move { rx.next().await },
            async {
                async_io::Timer::after(std::time::Duration::from_secs(10)).await;
                None
            },
        ))
        .with_context(|| format!("no PipeWireStreamAdded for {} (timeout)", m.connector))?;
        let (node,): (u32,) = msg.body().deserialize()?;
        eprintln!("PipeWireStreamAdded: {} -> node {node}", m.connector);
        nodes.push(node);
    }

    // Grab all monitor frames on ONE PipeWire connection + mainloop —
    // streams multiplex naturally, frames arrive concurrently.
    let _poke = DamagePoke::start(
        streams
            .iter()
            .map(|(m, _)| (m.logical.x as i16, m.logical.y as i16))
            .collect(),
    );
    let imgs = grab_frames(
        &nodes,
        &streams.iter().map(|(m, _)| m.phys).collect::<Vec<_>>(),
    );
    let imgs = imgs?;

    let mut frames = Vec::with_capacity(streams.len());
    for ((m, _), img) in streams.iter().zip(imgs) {
        frames.push(mk_frame(img, m.logical));
    }
    Ok(frames)
}

/// Mutter's screencast only emits a frame when that output has damage;
/// an idle desktop starves a fresh consumer indefinitely (cursor moves
/// live on a separate plane and don't count). A 1x1 override-redirect
/// XWayland window toggled map/unmap at each monitor's origin is a real
/// scene change — tiny, invisible, and damages every output. Optional:
/// if XWayland is absent we just rely on the watchdog/retry path.
struct DamagePoke {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl DamagePoke {
    fn start(origins: Vec<(i16, i16)>) -> Self {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop2 = stop.clone();
        let handle = std::thread::spawn(move || {
            use x11rb::connection::Connection as XConn;
            use x11rb::protocol::xproto::{ConnectionExt, CreateWindowAux, WindowClass};
            let run = || -> Result<()> {
                let (conn, sn) = x11rb::connect(None)?;
                let root = conn.setup().roots[sn].root;
                let mut wins = Vec::with_capacity(origins.len());
                for (x, y) in origins {
                    let w = conn.generate_id()?;
                    conn.create_window(
                        x11rb::COPY_DEPTH_FROM_PARENT,
                        w,
                        root,
                        x,
                        y,
                        1,
                        1,
                        0,
                        WindowClass::INPUT_OUTPUT,
                        0,
                        &CreateWindowAux::new().override_redirect(1),
                    )?;
                    wins.push(w);
                }
                while !stop2.load(std::sync::atomic::Ordering::Relaxed) {
                    for &w in &wins {
                        let _ = conn.map_window(w);
                    }
                    let _ = conn.flush();
                    std::thread::sleep(std::time::Duration::from_millis(120));
                    for &w in &wins {
                        let _ = conn.unmap_window(w);
                    }
                    let _ = conn.flush();
                    std::thread::sleep(std::time::Duration::from_millis(80));
                }
                for &w in &wins {
                    let _ = conn.destroy_window(w);
                }
                let _ = conn.flush();
                Ok(())
            };
            if let Err(e) = run() {
                eprintln!("damage poke died: {e}");
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for DamagePoke {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

// ------------------------------------------------------------- pipewire grab

struct ChanState {
    format: spa::param::video::VideoFormat,
    size: (u32, u32),
    frame: Option<Vec<u8>>,
    err: Option<String>,
    /// how many times process() ran / how many buffers were rejected —
    /// distinguishes "mutter never sent anything" from "frames arrived
    /// but were unusable" in the journal
    procs: u32,
    rejects: u32,
    /// last stream state — "Streaming" + 0 procs means mutter isn't
    /// producing (damage), "Connecting" means the node never came up
    state: &'static str,
}

/// One frame per PipeWire node, in-process via the `pipewire` crate.
/// Nodes are announced by mutter's PipeWireStreamAdded signal but may lag a
/// beat before they exist in the registry — retry the whole batch a few times.
fn grab_frames(nodes: &[u32], expects: &[(u32, u32)]) -> Result<Vec<image::RgbaImage>> {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(pw::init);

    // Mutter often doesn't push a frame to a first-time consumer until the
    // watchdog kills the attempt; the retry's reconnect lands on the warm
    // node and delivers immediately — so keep each attempt short and retry.
    let mut err = None;
    for attempt in 1..=5u32 {
        match grab_frames_once(nodes, expects) {
            Ok(v) => return Ok(v),
            Err(e) => {
                eprintln!("pw grab attempt {attempt}/5 failed: {e}");
                err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(150 * attempt as u64));
            }
        }
    }
    Err(err.unwrap())
}

/// Single attempt: EnumFormat negotiates a raw pixel format per stream, shm
/// buffers are copied row-wise and normalized to RGBA.
fn grab_frames_once(nodes: &[u32], expects: &[(u32, u32)]) -> Result<Vec<image::RgbaImage>> {
    use pw::properties::properties;
    use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    use spa::param::video::{VideoFormat, VideoInfoRaw};
    use spa::param::{format_utils, ParamType};
    use spa::pod::serialize::PodSerializer;
    use spa::pod::{ChoiceValue, Object, Property, Value};
    use spa::utils::{
        Choice, ChoiceEnum, ChoiceFlags, Direction, Fraction, Id, Rectangle, SpaTypes,
    };
    use std::cell::Cell;
    use std::io::Cursor;
    use std::time::Duration;

    let mainloop = pw::main_loop::MainLoop::new(None).context("pw mainloop")?;
    let context = pw::context::Context::new(&mainloop).context("pw context")?;
    let core = context.connect(None).context("pw connect")?;

    // PipeWireStreamAdded reports the node id mutter will create, but our
    // registry only learns about it asynchronously — connect() on a
    // not-yet-visible target leaves the stream parked in `unconnected`
    // forever. Warm the registry first: globals are enumerated on the
    // first run, so quit as soon as every wanted node has been seen.
    {
        let registry = core.get_registry().context("pw registry")?;
        let pending = Rc::new(RefCell::new(
            nodes.iter().copied().collect::<std::collections::HashSet<u32>>(),
        ));
        let weak = mainloop.downgrade();
        let _warm_listener = registry
            .add_listener_local()
            .global(move |g| {
                if !pending.borrow_mut().remove(&g.id) || !pending.borrow().is_empty() {
                    return;
                }
                if let Some(ml) = weak.upgrade() {
                    ml.quit();
                }
            })
            .register();
        let weak = mainloop.downgrade();
        let warm = mainloop.loop_().add_timer(move |_| {
            if let Some(ml) = weak.upgrade() {
                ml.quit();
            }
        });
        warm.update_timer(Some(Duration::from_secs(2)), None);
        mainloop.run();
        drop(warm);
    }

    let remaining = Rc::new(Cell::new(nodes.len()));
    let mut chans = Vec::with_capacity(nodes.len());
    let mut keepalive = Vec::new();

    for &expect in expects {
        let stream = pw::stream::Stream::new(
            &core,
            "quickshort-screencast",
            properties! {
                *pw::keys::MEDIA_TYPE => "Video",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Screen",
            },
        )
        .context("pw stream")?;

        let st = Rc::new(RefCell::new(ChanState {
            format: VideoFormat::Unknown,
            size: expect,
            frame: None,
            err: None,
            procs: 0,
            rejects: 0,
            state: "none",
        }));

        let listener = stream
            .add_local_listener::<()>()
            .state_changed({
                let weak = mainloop.downgrade();
                let st = st.clone();
                move |_, _, _, new| {
                    st.borrow_mut().state = match new {
                        pw::stream::StreamState::Error(e) => {
                            st.borrow_mut().err = Some(e.to_string());
                            if let Some(ml) = weak.upgrade() {
                                ml.quit();
                            }
                            "error"
                        }
                        pw::stream::StreamState::Streaming => "streaming",
                        pw::stream::StreamState::Paused => "paused",
                        pw::stream::StreamState::Connecting => "connecting",
                        pw::stream::StreamState::Unconnected => "unconnected",
                    };
                }
            })
            .param_changed({
                let st = st.clone();
                move |_, _, id, param| {
                    let Some(param) = param else { return };
                    if id != ParamType::Format.as_raw() {
                        return;
                    }
                    let Ok((mt, ms)) = format_utils::parse_format(param) else {
                        return;
                    };
                    if mt != MediaType::Video || ms != MediaSubtype::Raw {
                        return;
                    }
                    let mut info = VideoInfoRaw::new();
                    if info.parse(param).is_ok() {
                        let size = info.size();
                        let mut st = st.borrow_mut();
                        st.format = info.format();
                        st.size = (size.width, size.height);
                    }
                }
            })
            .process({
                let weak = mainloop.downgrade();
                let remaining = remaining.clone();
                let st = st.clone();
                move |stream, _| {
                    st.borrow_mut().procs += 1;
                    if st.borrow().frame.is_some() {
                        return;
                    }
                    let Some(mut buf) = stream.dequeue_buffer() else {
                        return;
                    };
                    let (w, h) = {
                        let s = st.borrow();
                        (s.size.0.max(1) as usize, s.size.1.max(1) as usize)
                    };
                    let mut got = false;
                    for data in buf.datas_mut() {
                        let chunk = data.chunk();
                        let (off, len, stride) = (
                            chunk.offset() as usize,
                            chunk.size() as usize,
                            (chunk.stride().unsigned_abs() as usize).max(w * 4),
                        );
                        // MemFd/DmaBuf frames arrive with a null data
                        // pointer — mmap the fd ourselves instead of
                        // dropping a perfectly good frame.
                        let mut mapped: Option<(*mut libc::c_void, usize)> = None;
                        let raw = match data.data() {
                            Some(d) => d,
                            None => {
                                let r = data.as_raw();
                                let total = (r.maxsize + r.mapoffset) as usize;
                                if r.fd < 0 || total == 0 {
                                    continue;
                                }
                                let p = unsafe {
                                    libc::mmap(
                                        std::ptr::null_mut(),
                                        total,
                                        libc::PROT_READ,
                                        libc::MAP_PRIVATE,
                                        r.fd as i32,
                                        0,
                                    )
                                };
                                if p == libc::MAP_FAILED {
                                    continue;
                                }
                                mapped = Some((p, total));
                                unsafe {
                                    std::slice::from_raw_parts(
                                        (p as *const u8).add(r.mapoffset as usize),
                                        r.maxsize as usize,
                                    )
                                }
                            }
                        };
                        let ok = len > 0 && raw.len() >= off + len;
                        if ok {
                            let s = &raw[off..off + len];
                            let mut out = vec![0u8; w * h * 4];
                            for y in 0..h {
                                let si = y * stride;
                                if si + w * 4 > s.len() {
                                    break;
                                }
                                out[y * w * 4..(y + 1) * w * 4]
                                    .copy_from_slice(&s[si..si + w * 4]);
                            }
                            st.borrow_mut().frame = Some(out);
                            got = true;
                        }
                        if let Some((p, total)) = mapped {
                            unsafe { libc::munmap(p, total) };
                        }
                        if got {
                            break;
                        }
                    }
                    drop(buf); // Buffer::drop returns it to the stream
                    if !got {
                        st.borrow_mut().rejects += 1;
                    }
                    if got {
                        let left = remaining.get().saturating_sub(1);
                        remaining.set(left);
                        if left == 0 {
                            if let Some(ml) = weak.upgrade() {
                                ml.quit();
                            }
                        }
                    }
                }
            })
            .register()
            .context("pw listener")?;

        // Offer every layout mutter might use; it picks one via EnumFormat.
        let formats = [
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA,
            VideoFormat::xRGB,
            VideoFormat::ARGB,
            VideoFormat::xBGR,
            VideoFormat::ABGR,
        ];
        let obj = Object {
            type_: SpaTypes::ObjectParamFormat.as_raw(),
            id: ParamType::EnumFormat.as_raw(),
            properties: vec![
                Property::new(
                    FormatProperties::MediaType.as_raw(),
                    Value::Id(Id(MediaType::Video.as_raw())),
                ),
                Property::new(
                    FormatProperties::MediaSubtype.as_raw(),
                    Value::Id(Id(MediaSubtype::Raw.as_raw())),
                ),
                Property::new(
                    FormatProperties::VideoFormat.as_raw(),
                    Value::Choice(ChoiceValue::Id(Choice(
                        ChoiceFlags::empty(),
                        ChoiceEnum::Enum {
                            default: Id(formats[0].as_raw()),
                            alternatives: formats[1..].iter().map(|f| Id(f.as_raw())).collect(),
                        },
                    ))),
                ),
                Property::new(
                    FormatProperties::VideoSize.as_raw(),
                    Value::Rectangle(Rectangle {
                        width: expect.0,
                        height: expect.1,
                    }),
                ),
                Property::new(
                    FormatProperties::VideoFramerate.as_raw(),
                    Value::Fraction(Fraction { num: 0, denom: 1 }),
                ),
            ],
        };
        let values = PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(obj))
            .context("serialize EnumFormat")?
            .0
            .into_inner();
        let pod = spa::pod::Pod::from_bytes(&values).context("pod bytes")?;
        let mut params = [pod];
        stream
            .connect(
                Direction::Input,
                Some(nodes[chans.len()]),
                pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
                &mut params,
            )
            .context("pw stream connect")?;



        keepalive.push((stream, listener));
        chans.push(st);
    }

    let keepalive = Rc::new(keepalive);

    // Mutter only produces a frame on damage — an idle screen can stall a
    // fresh consumer for seconds. Poke each stream so it asks for a frame
    // instead of waiting for the next screen update.
    let ka = Rc::clone(&keepalive);
    let poke = mainloop.loop_().add_timer(move |_| {
        for (s, _) in ka.iter() {
            let _ = s.trigger_process();
        }
    });
    poke.update_timer(
        Some(Duration::from_millis(250)),
        Some(Duration::from_millis(250)),
    );

    // Watchdog: quit the loop if frames never arrive.
    let weak = mainloop.downgrade();
    let timer = mainloop.loop_().add_timer(move |_| {
        if let Some(ml) = weak.upgrade() {
            ml.quit();
        }
    });
    timer.update_timer(Some(Duration::from_secs(2)), None);

    mainloop.run();
    drop(poke);
    drop(timer);
    for (stream, _) in keepalive.iter() {
        stream.disconnect().ok();
    }
    drop(keepalive);

    // Convert collected raw frames to RGBA images.
    let mut out = Vec::with_capacity(chans.len());
    for st in &chans {
        let st = st.borrow_mut();
        let (w, h) = st.size;
        let mut px = match (&st.frame, &st.err) {
            (Some(px), _) => px.clone(),
            (None, Some(e)) => bail!("pw stream error: {e}"),
            (None, None) => {
                eprintln!(
                    "pw node starved: state={} {} process() calls, {} rejected buffers",
                    st.state, st.procs, st.rejects
                );
                bail!("no frame received from PipeWire node (timeout)");
            }
        };
        let mut fmt = st.format;
        if fmt == VideoFormat::Unknown && px.len() == (w * h * 4) as usize {
            // param_changed never fired; mutter's screencast default is BGRx
            fmt = VideoFormat::BGRx;
        }
        use VideoFormat as F;
        match fmt {
            F::RGBA => {}
            F::RGBx | F::BGRx | F::xBGR | F::BGRA | F::ABGR | F::xRGB | F::ARGB => {
                for p in px.chunks_exact_mut(4) {
                    match fmt {
                        F::BGRA | F::BGRx | F::xBGR => p.swap(0, 2),
                        F::ABGR => {
                            p.copy_from_slice(&[p[3], p[2], p[1], p[0]]);
                        }
                        F::xRGB | F::ARGB => {
                            p.copy_from_slice(&[p[1], p[2], p[3], p[0]]);
                        }
                        _ => {}
                    }
                    p[3] = 255;
                }
            }
            other => bail!("unsupported negotiated format {other:?}"),
        }
        out.push(image::RgbaImage::from_raw(w, h, px).context("frame dims")?);
    }
    Ok(out)
}

// ------------------------------------------------------------------ geometry

/// org.gnome.Mutter.DisplayConfig.GetCurrentState:
/// (serial, monitors, logical_monitors, props)
/// monitor = ((connector,vendor,product,serial), modes, props)
/// mode    = (id, w, h, refresh, preferred_scale, supported_scales, props)
/// logical = (x, y, scale, transform, primary, [(connector,...)], props)
type Mode = (String, i32, i32, f64, f64, Vec<f64>, Dict);
type Monitor = ((String, String, String, String), Vec<Mode>, Dict);
type LogicalMon = (
    i32,
    i32,
    f64,
    u32,
    bool,
    Vec<(String, String, String, String)>,
    Dict,
);
type CurrentState = (u32, Vec<Monitor>, Vec<LogicalMon>, Dict);

struct Mon {
    connector: String,
    logical: Rect,
    phys: (u32, u32),
}

fn display_config(conn: &Connection) -> Result<Vec<Mon>> {
    let proxy = Proxy::new(&conn, CFG, CFG_PATH, CFG)?;
    let (_, monitors, logical, _): CurrentState = proxy.call("GetCurrentState", &())?;

    let mut mode_of: HashMap<String, (u32, u32)> = HashMap::new();
    for (spec, modes, _) in &monitors {
        if let Some(m) = modes.iter().find(|m| {
            m.6.get("is-current")
                .and_then(|v| bool::try_from(v.clone()).ok())
                .unwrap_or(false)
        }) {
            mode_of.insert(spec.0.clone(), (m.1 as u32, m.2 as u32));
        }
    }

    let mut out = Vec::new();
    for (x, y, scale, transform, _, specs, _) in &logical {
        let Some((connector, ..)) = specs.first() else {
            continue;
        };
        let Some(&(pw, ph)) = mode_of.get(connector) else {
            continue;
        };
        let (pw, ph) = if matches!(transform, 1 | 3 | 5 | 7) {
            (ph, pw)
        } else {
            (pw, ph)
        };
        out.push(Mon {
            connector: connector.clone(),
            phys: (pw, ph),
            logical: Rect::new(
                *x,
                *y,
                (pw as f64 / scale).round() as u32,
                (ph as f64 / scale).round() as u32,
            ),
        });
    }
    Ok(out)
}

fn mk_frame(img: image::RgbaImage, logical: Rect) -> CapturedFrame {
    let scale = if logical.w > 0 {
        img.width() as f64 / logical.w as f64
    } else {
        1.0
    };
    CapturedFrame {
        geometry: MonitorGeometry {
            origin: Point::new(logical.x, logical.y),
            size: Size::new(logical.w, logical.h),
            scale,
        },
        pixels: img,
    }
}

fn opts<'a>(entries: &'a [(&'a str, Value<'a>)]) -> HashMap<String, Value<'a>> {
    entries
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real session, no dialog: cargo test -p qs-capture wayland -- --ignored --nocapture
    #[test]
    #[ignore]
    fn mutter_screencast_returns_frames() {
        let frames = MutterCapture.capture_all().unwrap();
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

    // Dump each monitor's frame to /tmp/qs-dump-<i>.png — visual debugging.
    #[test]
    #[ignore]
    fn dump_screen_png() {
        for (i, f) in MutterCapture.capture_all().unwrap().iter().enumerate() {
            let p = format!("/tmp/qs-dump-{i}.png");
            f.pixels.save(&p).unwrap();
            eprintln!("saved {p}");
        }
    }
}
