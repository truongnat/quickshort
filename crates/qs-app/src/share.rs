//! Link sharing through the `quickshort-share` Cloudflare Worker.
//! The image is encoded as WebP (q95 — visually lossless at ~2-3x
//! smaller than PNG), uploaded over HTTPS, and served back from an
//! unguessable URL that expires after a few days — no tunnel, no local
//! server, and the process that shared it does not need to stay alive.
//!
//! Uploads also return a `delete_key`: a one-time 128-bit token the
//! worker only stores as SHA-256. Passing it to `revoke` deletes the
use anyhow::{Context, Result};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const ENDPOINT: &str = "https://share.2tsoftware.app";
/// Lossy WebP at this quality is visually indistinguishable from the PNG
/// source on screenshots while roughly halving upload + storage cost.
const WEBP_QUALITY: f32 = 95.0;

#[derive(Debug)]
pub struct SharedLink {
    pub url: String,
    /// one-time token to delete the image early — keep it, lose it and
    /// the image stays up until expiry
    pub delete_key: String,
    /// seconds until the worker drops the image (KV TTL)
    pub expires_s: u64,
}

/// Stateless — every call is a plain upload. `Arc`-shaped because the
/// host and one-shot CLI paths already hold it that way.
pub struct ShareManager;

impl ShareManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }

    /// Upload `img` and return its public URL. Blocks for the encode +
    /// upload — the caller runs this on a worker thread.
    pub fn share(&self, img: &RgbaImage) -> Result<SharedLink> {
        let endpoint = std::env::var("QS_SHARE_URL").unwrap_or_else(|_| ENDPOINT.into());
        let webp = webp::Encoder::from_rgba(img.as_raw(), img.width(), img.height())
            .encode(WEBP_QUALITY)
            .to_vec();
        match upload(&endpoint, &webp, "image/webp") {
            Ok(link) => Ok(link),
            // a worker running the old png-only build rejects webp with
            // 415 — fall back to PNG so share keeps working until the
            // worker is redeployed
            Err(e) if is_unsupported_media(&e) => {
                let mut png = std::io::Cursor::new(Vec::new());
                image::DynamicImage::ImageRgba8(img.clone())
                    .write_to(&mut png, image::ImageFormat::Png)
                    .context("png encode failed")?;
                upload(&endpoint, &png.into_inner(), "image/png")
            }
            Err(e) => Err(e),
        }
    }

    /// Delete a previously shared image. `url` is the share link and
    /// `delete_key` the token returned at upload. Idempotent — deleting
    /// an already-expired link is a silent no-op.
    pub fn revoke(&self, url: &str, delete_key: &str) -> Result<()> {
        let endpoint = std::env::var("QS_SHARE_URL").unwrap_or_else(|_| ENDPOINT.into());
        // strip the endpoint origin so an override still lands on /s/…
        let path = url
            .strip_prefix(ENDPOINT)
            .or_else(|| url.strip_prefix("https://quickshort-share.tx001-dev.workers.dev"))
            .unwrap_or(url);
        match ureq::delete(format!("{endpoint}{path}"))
            .header("x-delete-key", delete_key)
            .call()
        {
            Ok(_) => Ok(()),
            // 404 = already gone (expired or revoked) — success
            Err(ureq::Error::StatusCode(404)) => Ok(()),
            Err(e) => Err(e).context("share revoke failed"),
        }
    }
}

fn upload(endpoint: &str, body: &[u8], content_type: &str) -> Result<SharedLink> {
    let mut resp = ureq::post(format!("{endpoint}/u"))
        .content_type(content_type)
        .send(body)
        .context("share upload failed")?;
    let v: serde_json::Value = resp.body_mut().read_json().context("bad share response")?;
    Ok(SharedLink {
        url: v["url"]
            .as_str()
            .map(str::to_owned)
            .context("share response has no url")?,
        // empty on old workers that predate delete support — revoke()
        // against them returns 404 which is treated as gone
        delete_key: v["delete_key"].as_str().unwrap_or_default().to_owned(),
        expires_s: v["expires_in"].as_u64().unwrap_or(0),
    })
}

fn is_unsupported_media(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        matches!(
            c.downcast_ref::<ureq::Error>(),
            Some(ureq::Error::StatusCode(415))
        )
    })
}

/* ==================== share ledger (revoke across restarts) ========== */

/// One live share link — the delete key only exists while it is.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShareEntry {
    /// local file this link was made from — rehydrates the badge
    pub path: PathBuf,
    pub url: String,
    /// token for DELETE /s/<id> — empty for links shared before the
    /// worker learned revoke; those can only expire
    #[serde(default)]
    pub delete_key: String,
    /// unix timestamp the worker stops serving at
    #[serde(default)]
    pub expires_at: u64,
}

/// `shares.json` next to `config.toml` — the set of links this install
/// can still revoke. Entries age out on their own expiry or when the
/// backing file is deleted from disk.
pub struct ShareStore {
    entries: Vec<ShareEntry>,
    dirty: bool,
}

impl ShareStore {
    fn path() -> PathBuf {
        crate::config::Config::path()
            .parent()
            .map(|d| d.join("shares.json"))
            .unwrap_or_else(|| PathBuf::from("shares.json"))
    }

    pub fn load() -> Self {
        let entries = std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            entries,
            dirty: false,
        }
    }

    /// Entry for an on-disk shot, if a live link exists for it.
    pub fn get(&self, path: &Path) -> Option<&ShareEntry> {
        self.entries
            .iter()
            .find(|e| e.path == path && e.expires_at > unix_now())
    }

    pub fn put(&mut self, e: ShareEntry) {
        // one live link per file — re-sharing replaces the stale record
        self.entries.retain(|x| x.path != e.path);
        self.entries.push(e);
        self.dirty = true;
    }

    /// Drop the record for `url` (revoked, expired, or file gone).
    pub fn remove_url(&mut self, url: &str) {
        let before = self.entries.len();
        self.entries.retain(|e| e.url != url);
        self.dirty |= self.entries.len() != before;
    }

    /// Flush if dirty; dead entries (expired or missing file) are
    /// pruned on the way out so the file self-cleans.
    pub fn save(&mut self) {
        let now = unix_now();
        let before = self.entries.len();
        self.entries
            .retain(|e| e.expires_at > now && e.path.exists());
        self.dirty |= self.entries.len() != before;
        if !self.dirty {
            return;
        }
        let p = Self::path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(s) = serde_json::to_string_pretty(&self.entries) {
            let _ = std::fs::write(&p, s);
        }
        self.dirty = false;
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Build a ledger entry from a fresh upload.
pub fn entry(path: PathBuf, link: &SharedLink) -> ShareEntry {
    ShareEntry {
        path,
        url: link.url.clone(),
        delete_key: link.delete_key.clone(),
        expires_at: unix_now() + link.expires_s,
    }
}
