//! Link sharing through the `quickshort-share` Cloudflare Worker.
//! The image is encoded as WebP (q95 — visually lossless at ~2-3x
//! smaller than PNG), uploaded over HTTPS, and served back from an
//! unguessable URL that expires after a few days — no tunnel, no local
//! server, and the process that shared it does not need to stay alive.

use anyhow::{Context, Result};
use image::RgbaImage;
use std::sync::Arc;

const ENDPOINT: &str = "https://quickshort-share.tx001-dev.workers.dev";
/// Lossy WebP at this quality is visually indistinguishable from the PNG
/// source on screenshots while roughly halving upload + storage cost.
const WEBP_QUALITY: f32 = 95.0;

#[derive(Debug)]
pub struct SharedLink {
    pub url: String,
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
