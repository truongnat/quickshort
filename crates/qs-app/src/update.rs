//! Self-update against GitHub Releases — unsigned build.
//!
//! Check hits `releases/latest`; a newer semver downloads the
//! `*-macos.zip` asset (a zipped `.app`), swaps it over the running bundle,
//! and relaunches. Downloads go through `ureq`/plain HTTP so no quarantine
//! xattr is set, and releases are signed with a stable self-signed identity
//! (`packaging/macos/gen_signing_cert.sh`) so TCC grants survive the swap.
//!
//! All functions block — callers run them on a worker thread.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::PathBuf;

const REPO: &str = "truongnat/quickshort";
const CURRENT: &str = env!("CARGO_PKG_VERSION");

pub struct Release {
    pub version: String,
    zip_url: String,
    sha_url: Option<String>,
}

/// `Some` when the latest GitHub release is newer than this binary.
pub fn check() -> Result<Option<Release>> {
    let body: Value = ureq::get(&format!(
        "https://api.github.com/repos/{REPO}/releases/latest"
    ))
    .header("User-Agent", "quickshort")
    .header("Accept", "application/vnd.github+json")
    .call()
    .context("release check failed")?
    .body_mut()
    .read_json()
    .context("release reply is not JSON")?;

    let tag = body
        .get("tag_name")
        .and_then(|v| v.as_str())
        .context("release reply: no tag_name")?;
    let ver = tag.trim_start_matches('v');
    if !is_newer(ver, CURRENT) {
        return Ok(None);
    }

    let assets = body
        .get("assets")
        .and_then(|v| v.as_array())
        .context("release reply: no assets")?;
    let url_of = |name: &dyn Fn(&str) -> bool| {
        assets.iter().find_map(|a| {
            let n = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
            name(n)
                .then(|| a.get("browser_download_url").and_then(|v| v.as_str()))
                .flatten()
                .map(str::to_string)
        })
    };
    let zip_url =
        url_of(&|n| n.ends_with("-macos.zip")).context("release has no *-macos.zip asset")?;
    let sha_url = url_of(&|n| n.ends_with("-macos.zip.sha256"));
    Ok(Some(Release {
        version: ver.to_string(),
        zip_url,
        sha_url,
    }))
}

/// Download the release, swap it over the running `.app`, relaunch.
/// Returns the new app path; the caller should exit its event loop.
#[cfg(target_os = "macos")]
pub fn install(rel: &Release) -> Result<PathBuf> {
    let app = current_bundle().context("not running from a .app bundle")?;
    let tmp = std::env::temp_dir().join("quickshort-update");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;

    let zip = tmp.join("update.zip");
    let mut resp = ureq::get(&rel.zip_url)
        .header("User-Agent", "quickshort")
        .call()
        .context("download failed")?;
    let mut f = std::fs::File::create(&zip)?;
    std::io::copy(
        &mut std::io::Read::take(resp.body_mut().as_reader(), 64 * 1024 * 1024),
        &mut f,
    )
    .context("download write failed")?;

    if let Some(sha_url) = &rel.sha_url {
        let want: String = ureq::get(sha_url)
            .call()
            .and_then(|mut r| r.body_mut().read_to_string())
            .context("sha256 asset fetch failed")?;
        let want = want.split_whitespace().next().unwrap_or("").to_string();
        let got = shasum(&zip)?;
        if !want.is_empty() && want != got {
            bail!("checksum mismatch: got {got}, want {want}");
        }
    }

    if !std::process::Command::new("/usr/bin/ditto")
        .args(["-xk"])
        .arg(&zip)
        .arg(&tmp)
        .status()
        .context("unzip failed")?
        .success()
    {
        bail!("unzip failed");
    }

    let new_app = tmp.join("Quickshort.app");
    if !new_app.join("Contents/MacOS/quickshort").exists() {
        bail!("update archive is missing Quickshort.app");
    }

    // swap: move the old bundle aside, drop the new one in place
    let backup = app.with_extension(format!("old-{}", unix_ts()?));
    std::fs::rename(&app, &backup).context("can't move the old app aside")?;
    match std::fs::rename(&new_app, &app) {
        Ok(()) => {
            let _ = std::fs::remove_dir_all(&backup);
        }
        Err(e) => {
            let _ = std::fs::rename(&backup, &app); // roll back
            return Err(e).context("can't install the new app");
        }
    }
    Ok(app)
}

/// Relaunch the freshly installed bundle; the current process must exit.
#[cfg(target_os = "macos")]
pub fn relaunch(app: &std::path::Path) -> Result<()> {
    std::process::Command::new("open")
        .arg("-n")
        .arg(app)
        .spawn()
        .context("relaunch failed")?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn install(_rel: &Release) -> Result<PathBuf> {
    bail!("auto-update is macOS-only for now — grab the release from GitHub")
}

/// `/Applications/Quickshort.app` (or wherever the bundle lives), walking up
/// from the executable. `None` when running an unbundled binary (dev builds).
#[cfg(target_os = "macos")]
fn current_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(|p| p.to_path_buf())
}

#[cfg(target_os = "macos")]
fn shasum(p: &std::path::Path) -> Result<String> {
    let out = std::process::Command::new("/usr/bin/shasum")
        .arg("-a")
        .arg("256")
        .arg(p)
        .output()?;
    String::from_utf8(out.stdout)?
        .split_whitespace()
        .next()
        .map(str::to_string)
        .context("shasum gave no output")
}

fn unix_ts() -> Result<u64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs())
}

/// Loose semver compare: "1.10.0" > "1.9.9"; non-numeric suffixes ignored.
fn is_newer(a: &str, b: &str) -> bool {
    let parse = |v: &str| -> [u64; 3] {
        let mut out = [0u64; 3];
        for (i, seg) in v.split('.').take(3).enumerate() {
            let digits: String = seg.chars().take_while(|c| c.is_ascii_digit()).collect();
            out[i] = digits.parse().unwrap_or(0);
        }
        out
    };
    parse(a) > parse(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_compare() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(is_newer("1.0.0-beta", "0.9.9"));
    }

    /// Live check — `cargo test -p qs-app -- --ignored`
    #[test]
    #[ignore]
    fn live_release_check() {
        let r = check().unwrap();
        eprintln!("latest release: {r:?}", r = r.map(|r| r.version));
    }
}
