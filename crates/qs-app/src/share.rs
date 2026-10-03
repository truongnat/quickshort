//! Link sharing without a backend: a tiny HTTP server bound to
//! 127.0.0.1 serves shared PNGs under an unguessable token path, and a
//! `ssh -R` reverse tunnel to tunnl.gg publishes that port to a public
//! HTTPS URL. The file never leaves the user's machine — requests just
//! travel down the SSH connection.
//!
//! `stable@` mode derives the subdomain from our SSH key, so the URL
//! survives ssh respawns (24h tunnel cap, network drops) and even app
//! restarts while the key persists.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

const TUNNEL_DEST: &str = "stable@proxy.tunnl.gg";
/// how long `share()` waits for the tunnel's URL to be assigned
const URL_WAIT: Duration = Duration::from_secs(15);
/// tunnl.gg closes tunnels idle for 2h — a periodic request through the
/// tunnel itself keeps it counted as live
const KEEPALIVE_EVERY: Duration = Duration::from_secs(20 * 60);

#[derive(Default)]
struct State {
    port: Option<u16>,
    /// https://<sub>.tunnl.gg — set once the banner is parsed
    base: Option<String>,
    last_err: Option<String>,
}

/// One listener + one tunnel shared by every shared file. `Arc`-held by
/// the host; dropping kills the ssh child and the accept loop.
pub struct ShareManager {
    /// token -> shared file path
    files: Arc<Mutex<HashMap<String, PathBuf>>>,
    state: Arc<(Mutex<State>, Condvar)>,
    stop: Arc<AtomicBool>,
    /// pid of the live ssh child (the monitor thread owns the Child)
    child_pid: AtomicU32,
    monitor_started: AtomicBool,
    keepalive_started: AtomicBool,
    /// ed25519 key = the stable URL's identity
    key: PathBuf,
}

impl ShareManager {
    pub fn new() -> Arc<Self> {
        let key = crate::config::Config::path()
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("share_key");
        Arc::new(Self {
            files: Arc::new(Mutex::new(HashMap::new())),
            state: Arc::new((Mutex::new(State::default()), Condvar::new())),
            stop: Arc::new(AtomicBool::new(false)),
            child_pid: AtomicU32::new(0),
            monitor_started: AtomicBool::new(false),
            keepalive_started: AtomicBool::new(false),
            key,
        })
    }

    /// Register `path` under a fresh token and return its public URL —
    /// starts the server/tunnel lazily and blocks until tunnl.gg assigns
    /// the URL (first share takes a couple of seconds).
    pub fn share(self: &Arc<Self>, path: &Path) -> Result<String> {
        let token = new_token()?;
        self.files
            .lock()
            .unwrap()
            .insert(token.clone(), path.to_path_buf());
        self.ensure_server()?;
        self.ensure_tunnel()?;
        let base = self.wait_base()?;
        Ok(format!("{base}/s/{token}.png"))
    }

    fn ensure_server(&self) -> Result<u16> {
        {
            let st = self.state.0.lock().unwrap();
            if let Some(p) = st.port {
                return Ok(p);
            }
        }
        let l = TcpListener::bind(("127.0.0.1", 0)).context("share listener")?;
        let port = l.local_addr()?.port();
        let (files, stop) = (self.files.clone(), self.stop.clone());
        std::thread::spawn(move || serve(l, files, stop));
        self.state.0.lock().unwrap().port = Some(port);
        Ok(port)
    }

    fn ensure_tunnel(self: &Arc<Self>) -> Result<()> {
        ensure_key(&self.key)?;
        if self
            .monitor_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let mgr = Arc::clone(self);
            std::thread::spawn(move || mgr.monitor());
        }
        Ok(())
    }

    /// ssh supervision: spawn, scan stdout for the assigned URL, respawn
    /// on death — the stable@ key makes every reincarnation reuse the
    /// same subdomain.
    fn monitor(self: &Arc<Self>) {
        let port = self.state.0.lock().unwrap().port.unwrap_or(0);
        while !self.stop.load(Ordering::SeqCst) {
            let mut child = match spawn_ssh(&self.key, port) {
                Ok(c) => c,
                Err(e) => {
                    self.state.0.lock().unwrap().last_err = Some(e.to_string());
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
            };
            self.child_pid.store(child.id(), Ordering::SeqCst);
            if let Some(out) = child.stdout.take() {
                for line in BufReader::new(out).lines().map_while(|l| l.ok()) {
                    if let Some(b) = extract_url(&line) {
                        let mut st = self.state.0.lock().unwrap();
                        if st.base.as_deref() != Some(b.as_str()) {
                            st.base = Some(b);
                            self.state.1.notify_all();
                            drop(st);
                            self.start_keepalive();
                        }
                    }
                }
            }
            let _ = child.wait();
            self.child_pid.store(0, Ordering::SeqCst);
            // the URL is gone until the next spawn prints it again
            self.state.0.lock().unwrap().base = None;
            std::thread::sleep(Duration::from_secs(2));
        }
    }

    /// A request through the public URL every 20 minutes defeats the
    /// provider's 2h idle timeout.
    fn start_keepalive(self: &Arc<Self>) {
        if self
            .keepalive_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let mgr = Arc::clone(self);
        std::thread::spawn(move || {
            while !mgr.stop.load(Ordering::SeqCst) {
                std::thread::sleep(KEEPALIVE_EVERY);
                let base = mgr.state.0.lock().unwrap().base.clone();
                if let Some(b) = base {
                    let _ = ureq::get(&format!("{b}/healthz")).call();
                }
            }
        });
    }

    fn wait_base(&self) -> Result<String> {
        let (lock, cv) = &*self.state;
        let mut st = lock.lock().unwrap();
        let deadline = std::time::Instant::now() + URL_WAIT;
        loop {
            if let Some(b) = &st.base {
                return Ok(b.clone());
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                let e = st
                    .last_err
                    .clone()
                    .unwrap_or_else(|| "tunnel did not come up".into());
                anyhow::bail!("share link timed out: {e}");
            }
            let (g, _) = cv.wait_timeout(st, left).unwrap();
            st = g;
        }
    }
}

impl Drop for ShareManager {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let pid = self.child_pid.load(Ordering::SeqCst);
        if pid != 0 {
            let _ = Command::new("kill").arg(pid.to_string()).status();
        }
    }
}

/// The app-owned ed25519 key the stable subdomain is derived from.
fn ensure_key(key: &Path) -> Result<()> {
    if key.exists() {
        return Ok(());
    }
    if let Some(d) = key.parent() {
        std::fs::create_dir_all(d)?;
    }
    let st = Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-C", "quickshort-share", "-f"])
        .arg(key)
        .stdout(Stdio::null())
        .status()
        .context("ssh-keygen")?;
    anyhow::ensure!(st.success(), "ssh-keygen failed");
    Ok(())
}

fn spawn_ssh(key: &Path, port: u16) -> Result<Child> {
    Command::new("ssh")
        // -tt: tunnl.gg needs the session channel + pty to print the URL
        // (ssh -N gets the connection killed). stdin stays piped-and-held
        // inside the Child — EOF on it would end the remote session and
        // drop the tunnel
        // BatchMode keeps it from ever prompting (no tty for the user)
        .args([
            "-tt",
            "-N",
            "-R",
            &format!("80:127.0.0.1:{port}"),
            TUNNEL_DEST,
            "-i",
        ])
        .arg(key)
        .args([
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "ServerAliveInterval=30",
            "-o",
            "ExitOnForwardFailure=yes",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn ssh tunnel")
}

/// First `https://…tunnl.gg` run in a line — the banner prints it next
/// to a QR code; block-art can't false-positive a scheme prefix.
fn extract_url(line: &str) -> Option<String> {
    let i = line.find("https://")?;
    let url: String = line[i..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/' | ':'))
        .collect();
    url.ends_with(".tunnl.gg").then_some(url)
}

/// 128-bit hex token — the URL path is the only auth a share link has.
fn new_token() -> Result<String> {
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

fn serve(
    listener: TcpListener,
    files: Arc<Mutex<HashMap<String, PathBuf>>>,
    stop: Arc<AtomicBool>,
) {
    let _ = listener.set_nonblocking(true);
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((s, _)) => handle_conn(s, &files),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(_) => break,
        }
    }
}

/// One request per connection: `GET|HEAD /s/<32-hex>.png` or 404.
fn handle_conn(s: TcpStream, files: &Mutex<HashMap<String, PathBuf>>) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let mut w = match s.try_clone() {
        Ok(w) => w,
        Err(_) => return,
    };
    let mut first = String::new();
    if BufReader::new(s).take(8192).read_line(&mut first).is_err() {
        return;
    }
    let mut parts = first.split_whitespace();
    let (method, path) = (parts.next(), parts.next());
    let head_only = method == Some("HEAD");
    let token = (method == Some("GET") || head_only)
        .then(|| {
            path.and_then(|p| p.strip_prefix("/s/"))
                .and_then(|t| t.strip_suffix(".png"))
                .filter(|t| t.len() == 32 && t.bytes().all(|b| b.is_ascii_hexdigit()))
        })
        .flatten();
    let file = token.and_then(|t| files.lock().unwrap().get(t).cloned());
    let Some(file) = file else {
        let _ = w
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    };
    match std::fs::read(&file) {
        Ok(body) => {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nCache-Control: public, max-age=86400\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = w.write_all(head.as_bytes());
            if !head_only {
                let _ = w.write_all(&body);
            }
        }
        Err(_) => {
            let _ = w.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn server_serves_only_token_paths() {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let dir = std::env::temp_dir().join(format!("qs-share-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        std::fs::write(&png, b"\x89PNG-test").unwrap();
        let files = Arc::new(Mutex::new(HashMap::from([(
            "0123456789abcdef0123456789abcdef".to_string(),
            png,
        )])));
        let stop = Arc::new(AtomicBool::new(false));
        let (f, st) = (files.clone(), stop.clone());
        let t = std::thread::spawn(move || serve(l, f, st));

        let get = |path: &str| -> (String, Vec<u8>) {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(format!("GET {path} HTTP/1.1\r\nConnection: close\r\n\r\n").as_bytes())
                .unwrap();
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).unwrap();
            // split head/body on raw bytes — PNG bodies aren't UTF-8
            let i = buf
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .expect("no header terminator");
            (
                String::from_utf8_lossy(&buf[..i]).into_owned(),
                buf[i + 4..].to_vec(),
            )
        };

        let (h, b) = get("/s/0123456789abcdef0123456789abcdef.png");
        assert!(h.contains("200 OK"), "{h}");
        assert_eq!(b, b"\x89PNG-test");

        // wrong token, bad path, directory traversal: all dead ends
        for p in [
            "/s/deadbeefdeadbeefdeadbeefdeadbeef.png",
            "/s/../config.toml",
            "/",
            "/s/0123456789abcdef0123456789abcdef",
        ] {
            assert!(get(p).0.contains("404"), "{p}");
        }

        stop.store(true, Ordering::SeqCst);
        let _ = t.join();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn url_extraction() {
        let line = "  🌐️ https://brave-fox-7c41e09b.tunnl.gg  ";
        assert_eq!(
            extract_url(line).as_deref(),
            Some("https://brave-fox-7c41e09b.tunnl.gg")
        );
        assert!(extract_url("no url here").is_none());
        assert!(extract_url("https://evil.example.com").is_none());
    }
}
