//! Self-update against GitHub Releases — unsigned build.
//!
//! Check hits `releases/latest`; a newer semver downloads the asset for this
//! platform and swaps it over the running binary:
//!
//! - macOS: a zipped `.app` swapped over the bundle, then relaunched via
//!   `open`. Downloads go through `ureq`/plain HTTP so no quarantine xattr is
//!   set, and releases are signed with a stable self-signed identity
//!   (`packaging/macos/gen_signing_cert.sh`) so TCC grants survive the swap.
//! - Linux: the `.deb` asset is unpacked with `ar`/`tar` (or `dpkg-deb`) and
//!   the binary atomically renamed over `current_exe` — `pkexec install`
//!   when the install dir isn't writable. Running as an AppImage replaces
//!   `$APPIMAGE` instead.
//! - Windows: the NSIS `*-Setup.exe` is launched and the app exits; the
//!   installer performs the upgrade.
//!
//! All functions block — callers run them on a worker thread.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

const REPO: &str = "truongnat/quickshort";
const CURRENT: &str = env!("CARGO_PKG_VERSION");

pub struct Release {
    pub version: String,
    /// asset name → download url
    assets: Vec<(String, String)>,
}

impl Release {
    fn asset(&self, pred: impl Fn(&str) -> bool) -> Option<&(String, String)> {
        self.assets.iter().find(|(n, _)| pred(n))
    }
}

/// `Some` when the latest GitHub release is newer than this binary.
pub fn check() -> Result<Option<Release>> {
    let resp = ureq::get(&format!(
        "https://api.github.com/repos/{REPO}/releases/latest"
    ))
    .header("User-Agent", "quickshort")
    .header("Accept", "application/vnd.github+json")
    .call();
    let body: Value = match resp {
        Ok(mut r) => r
            .body_mut()
            .read_json()
            .context("release reply is not JSON")?,
        // 404 = no release published yet — that's "up to date", not an error
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(e).context("release check failed"),
    };

    let tag = body
        .get("tag_name")
        .and_then(|v| v.as_str())
        .context("release reply: no tag_name")?;
    let ver = tag.trim_start_matches('v');
    if !is_newer(ver, CURRENT) {
        return Ok(None);
    }

    let assets: Vec<(String, String)> = body
        .get("assets")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    let n = x.get("name")?.as_str()?.to_string();
                    let u = x.get("browser_download_url")?.as_str()?.to_string();
                    Some((n, u))
                })
                .collect()
        })
        .context("release reply: no assets")?;

    // bail early when this platform's asset is missing — an update we can't
    // install must not be offered
    #[cfg(target_os = "macos")]
    let ok = assets.iter().any(|(n, _)| n.ends_with("-macos.zip"));
    #[cfg(target_os = "linux")]
    let ok = assets
        .iter()
        .any(|(n, _)| n == "quickshort_amd64.deb" || n.ends_with(".AppImage"));
    #[cfg(target_os = "windows")]
    let ok = assets
        .iter()
        .any(|(n, _)| n.ends_with("Setup.exe") || n.ends_with("setup.exe"));
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let ok = false;
    if !ok {
        bail!("release {ver} has no asset for this platform");
    }

    Ok(Some(Release {
        version: ver.to_string(),
        assets,
    }))
}

fn download(url: &str, dest: &Path) -> Result<()> {
    let mut resp = ureq::get(url)
        .header("User-Agent", "quickshort")
        .call()
        .context("download failed")?;
    let mut f = std::fs::File::create(dest)?;
    std::io::copy(
        &mut std::io::Read::take(resp.body_mut().as_reader(), 64 * 1024 * 1024),
        &mut f,
    )
    .context("download write failed")?;
    Ok(())
}

/// Sha256 the release claims for `asset_name`, pulled from its `.sha256`
/// asset (`linux.sha256` is a multi-line file covering every linux asset).
fn expected_sha(rel: &Release, asset_name: &str) -> Result<Option<String>> {
    let sha_name = if cfg!(target_os = "linux") {
        "linux.sha256".to_string()
    } else {
        format!("{asset_name}.sha256")
    };
    let Some((_, url)) = rel.asset(|n| n == sha_name) else {
        return Ok(None);
    };
    let body: String = ureq::get(url)
        .call()
        .and_then(|mut r| r.body_mut().read_to_string())
        .context("sha256 asset fetch failed")?;
    Ok(body
        .lines()
        .find(|l| l.contains(asset_name))
        .and_then(|l| l.split_whitespace().next())
        .map(str::to_string))
}

fn verify(rel: &Release, file: &Path, asset_name: &str) -> Result<()> {
    if let Some(want) = expected_sha(rel, asset_name)? {
        let got = sha256(file)?;
        if !want.is_empty() && want != got {
            bail!("checksum mismatch: got {got}, want {want}");
        }
    }
    Ok(())
}

/// Download the release, swap it over the running `.app`, relaunch.
/// Returns the new app path; the caller should exit its event loop.
#[cfg(target_os = "macos")]
pub fn install(rel: &Release) -> Result<PathBuf> {
    let app = current_bundle().context("not running from a .app bundle")?;
    let tmp = std::env::temp_dir().join("quickshort-update");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;

    let (name, url) = rel
        .asset(|n| n.ends_with("-macos.zip"))
        .context("release has no *-macos.zip asset")?;
    let zip = tmp.join(name);
    download(url, &zip)?;
    verify(rel, &zip, name)?;

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

/// Download the `.deb`, pull out `usr/bin/quickshort`, swap it over the
/// running binary. AppImage runs replace `$APPIMAGE` instead.
#[cfg(target_os = "linux")]
pub fn install(rel: &Release) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("can't locate current exe")?;
    let tmp = std::env::temp_dir().join("quickshort-update");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;

    // AppImage: the whole app is one file — download and replace it
    if let Ok(appimage) = std::env::var("APPIMAGE") {
        let (name, url) = rel
            .asset(|n| n.ends_with(".AppImage"))
            .context("release has no AppImage asset")?;
        let f = tmp.join(name);
        download(url, &f)?;
        verify(rel, &f, name)?;
        set_exec(&f)?;
        // stage next to the target so rename stays on one filesystem
        let staged = PathBuf::from(&appimage).with_extension("new");
        std::fs::copy(&f, &staged).context("can't stage AppImage")?;
        std::fs::rename(&staged, &appimage).context("can't replace AppImage")?;
        return Ok(PathBuf::from(appimage));
    }

    let (name, url) = rel
        .asset(|n| n == "quickshort_amd64.deb")
        .context("release has no linux .deb asset")?;
    let deb = tmp.join(name);
    download(url, &deb)?;
    verify(rel, &deb, name)?;

    // extract data.tar payload — dpkg-deb when present, ar+tar otherwise
    let ok = std::process::Command::new("dpkg-deb")
        .args(["-x"])
        .arg(&deb)
        .arg(&tmp)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        std::process::Command::new("ar")
            .arg("x")
            .arg(&deb)
            .current_dir(&tmp)
            .status()
            .context("deb unpack failed (ar)")?;
        let data = std::fs::read_dir(&tmp)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("data.tar"))
            })
            .context("deb has no data.tar")?;
        if !std::process::Command::new("tar")
            .arg("xf")
            .arg(&data)
            .current_dir(&tmp)
            .status()
            .context("deb unpack failed (tar)")?
            .success()
        {
            bail!("deb unpack failed (tar)");
        }
    }
    let new_bin = tmp.join("usr/bin/quickshort");
    if !new_bin.exists() {
        bail!("update archive is missing usr/bin/quickshort");
    }
    set_exec(&new_bin)?;

    // swap atomically when the install dir is writable; elevated copy
    // (pkexec → polkit prompt) when it isn't, e.g. /usr/bin from a .deb
    let staged = exe.with_extension("new");
    let swapped = std::fs::copy(&new_bin, &staged)
        .and_then(|_| std::fs::rename(&staged, &exe))
        .is_ok();
    if !swapped {
        let s = std::process::Command::new("pkexec")
            .args(["install", "-m", "755"])
            .arg(&new_bin)
            .arg(&exe)
            .status()
            .context("can't replace binary — run the update manually")?;
        if !s.success() {
            bail!("pkexec install failed ({s}) — run the update manually");
        }
    }
    Ok(exe)
}

/// Download the NSIS installer and run it; the caller then exits and the
/// installer performs the upgrade.
#[cfg(target_os = "windows")]
pub fn install(rel: &Release) -> Result<PathBuf> {
    let (name, url) = rel
        .asset(|n| n.ends_with("Setup.exe") || n.ends_with("setup.exe"))
        .context("release has no windows installer asset")?;
    let setup = std::env::temp_dir().join(name);
    download(url, &setup)?;
    verify(rel, &setup, name)?;
    std::process::Command::new(&setup)
        .spawn()
        .context("can't launch installer")?;
    Ok(std::env::current_exe()?)
}

/// Relaunch the freshly installed app; the current process must exit.
/// Windows relaunches itself from the NSIS installer — nothing to do.
#[cfg(target_os = "macos")]
pub fn relaunch(app: &Path) -> Result<()> {
    std::process::Command::new("open")
        .arg("-n")
        .arg(app)
        .spawn()
        .context("relaunch failed")?;
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn relaunch(exe: &Path) -> Result<()> {
    std::process::Command::new(exe)
        .spawn()
        .context("relaunch failed")?;
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn relaunch(_exe: &Path) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn set_exec(p: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).context("chmod failed")
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
fn sha256(p: &Path) -> Result<String> {
    let out = std::process::Command::new("/usr/bin/shasum")
        .arg("-a")
        .arg("256")
        .arg(p)
        .output()?;
    first_word(&out.stdout)
}

#[cfg(target_os = "linux")]
fn sha256(p: &Path) -> Result<String> {
    let out = std::process::Command::new("sha256sum").arg(p).output()?;
    first_word(&out.stdout)
}

#[cfg(target_os = "windows")]
fn sha256(p: &Path) -> Result<String> {
    // certutil prints "<hex>" on its second output line
    let out = std::process::Command::new("certutil")
        .args(["-hashfile"])
        .arg(p)
        .arg("SHA256")
        .output()?;
    let text = String::from_utf8(out.stdout)?;
    text.lines()
        .nth(1)
        .map(|l| l.replace(' ', "").to_lowercase())
        .context("certutil gave no hash")
}

#[cfg(unix)]
fn first_word(stdout: &[u8]) -> Result<String> {
    String::from_utf8(stdout.to_vec())?
        .split_whitespace()
        .next()
        .map(str::to_string)
        .context("sha256 gave no output")
}

#[cfg(target_os = "macos")]
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
