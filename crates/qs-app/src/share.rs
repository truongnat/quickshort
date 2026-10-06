//! Link sharing through the `quickshort-share` Cloudflare Worker.
//! The PNG is uploaded over HTTPS and served back from an unguessable
//! URL that expires after a week — no tunnel, no local server, and the
//! process that shared it does not need to stay alive.

use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;

const ENDPOINT: &str = "https://quickshort-share.truongnat.workers.dev";

/// Stateless — every call is a plain upload. `Arc`-shaped because the
/// host and one-shot CLI paths already hold it that way.
pub struct ShareManager;

impl ShareManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }

    /// Upload `path` and return its public URL. Blocks for the upload —
    /// the caller runs this on a worker thread.
    pub fn share(&self, path: &Path) -> Result<String> {
        let bytes = std::fs::read(path).context("read shared file")?;
        let endpoint = std::env::var("QS_SHARE_URL").unwrap_or_else(|_| ENDPOINT.into());
        let mut resp = ureq::post(format!("{endpoint}/u"))
            .content_type("image/png")
            .send(&bytes[..])
            .context("share upload failed")?;
        let v: serde_json::Value = resp.body_mut().read_json().context("bad share response")?;
        v["url"]
            .as_str()
            .map(str::to_owned)
            .context("share response has no url")
    }
}
