//! `ams update` — self-update from GitHub releases, plus the passive daily
//! check: any ams invocation spawns a detached `ams update --quiet` in the
//! background when the last check is older than 24 h (stamp file in
//! ~/.cache/ams). Opt out with AMS_NO_SELF_UPDATE=1.
//!
//! Network and hashing go through `curl` and `sha256sum`/`shasum` — the same
//! tools install.sh already requires; no extra crates. The binary is replaced
//! atomically: copy next to the target, then rename over it (safe for a
//! running executable on Linux/macOS). Windows: not supported, use the zip.

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const REPO: &str = "StMalina/ams";
const CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

fn cache_dir() -> Option<PathBuf> {
    if let Ok(x) = std::env::var("XDG_CACHE_HOME") {
        if !x.is_empty() {
            return Some(PathBuf::from(x).join("ams"));
        }
    }
    std::env::var("HOME")
        .ok()
        .map(|h| Path::new(&h).join(".cache/ams"))
}

/// Fire-and-forget daily check. Never fails, never blocks, never prints.
pub fn maybe_background_check() {
    if std::env::var("AMS_NO_SELF_UPDATE").ok().as_deref() == Some("1") {
        return;
    }
    if cfg!(windows) {
        return;
    }
    let Some(dir) = cache_dir() else { return };
    let stamp = dir.join("update-check");
    if let Ok(meta) = fs::metadata(&stamp) {
        if let Ok(modified) = meta.modified() {
            if modified.elapsed().map(|e| e < CHECK_INTERVAL).unwrap_or(true) {
                return;
            }
        }
    }
    // Touch the stamp first: even if the spawn fails we won't retry until
    // tomorrow — a broken updater must not tax every invocation.
    if fs::create_dir_all(&dir).is_err() || fs::write(&stamp, b"").is_err() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let _ = Command::new(exe)
        .args(["update", "--quiet"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn curl(args: &[&str]) -> Result<String> {
    let out = Command::new("curl")
        .args(args)
        .stderr(Stdio::null())
        .output()
        .context("running curl (is it installed?)")?;
    if !out.status.success() {
        bail!("curl {} failed", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn latest_tag() -> Result<String> {
    // 302 redirect of /releases/latest — no API rate limit.
    let effective = curl(&[
        "-fsSLI",
        "-o",
        "/dev/null",
        "-w",
        "%{url_effective}",
        &format!("https://github.com/{REPO}/releases/latest"),
    ])?;
    if let Some(tag) = effective.trim().rsplit("/tag/").next() {
        if tag.starts_with('v') && !tag.contains('/') {
            return Ok(tag.to_string());
        }
    }
    // Fallback: REST API.
    let json = curl(&[
        "-fsSL",
        &format!("https://api.github.com/repos/{REPO}/releases/latest"),
    ])?;
    json.split('"')
        .skip_while(|s| *s != "tag_name")
        .nth(2)
        .filter(|t| t.starts_with('v'))
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("could not resolve the latest release tag"))
}

/// "1.2.3" -> (1, 2, 3); unparsable parts are 0, pre-release suffixes ignored.
fn semver(v: &str) -> (u64, u64, u64) {
    let mut parts = v
        .split(['.', '-', '+'])
        .map(|p| p.parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

fn sha256_of(path: &Path) -> Result<String> {
    for tool in [&["sha256sum"][..], &["shasum", "-a", "256"][..]] {
        let out = Command::new(tool[0])
            .args(&tool[1..])
            .arg(path)
            .stderr(Stdio::null())
            .output();
        if let Ok(out) = out {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                if let Some(hash) = text.split_whitespace().next() {
                    return Ok(hash.to_lowercase());
                }
            }
        }
    }
    bail!("neither sha256sum nor shasum is available")
}

pub fn run(quiet: bool) -> Result<()> {
    if cfg!(windows) {
        bail!("self-update is not supported on Windows — download the zip from https://github.com/{REPO}/releases");
    }
    let current = env!("CARGO_PKG_VERSION");
    let tag = latest_tag()?;
    let latest = tag.trim_start_matches('v');
    // Strict "newer than": a locally built binary ahead of the latest release
    // must never be downgraded.
    if semver(latest) <= semver(current) {
        if !quiet {
            println!("ams {current} is up to date (latest release: {latest})");
        }
        return Ok(());
    }

    let arch = match std::env::consts::ARCH {
        a @ ("x86_64" | "aarch64") => a,
        other => bail!("unsupported architecture {other}"),
    };
    let os = match std::env::consts::OS {
        "linux" => "unknown-linux-musl",
        "macos" => "apple-darwin",
        other => bail!("unsupported OS {other}"),
    };
    let asset = format!("ams-{tag}-{arch}-{os}.tar.gz");
    let url = format!("https://github.com/{REPO}/releases/download/{tag}/{asset}");

    let tmp = std::env::temp_dir().join(format!("ams-update-{}", std::process::id()));
    fs::create_dir_all(&tmp)?;
    // Best-effort cleanup on every exit path below.
    let _guard = scopeguard(tmp.clone());

    let archive = tmp.join(&asset);
    curl(&["-fsSL", &url, "-o", archive.to_str().context("bad tmp path")?])?;
    let sha_file = tmp.join(format!("{asset}.sha256"));
    curl(&[
        "-fsSL",
        &format!("{url}.sha256"),
        "-o",
        sha_file.to_str().context("bad tmp path")?,
    ])?;
    let expected = fs::read_to_string(&sha_file)?
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_lowercase();
    let actual = sha256_of(&archive)?;
    if expected.is_empty() || expected != actual {
        bail!("checksum mismatch for {asset} — update aborted");
    }

    let status = Command::new("tar")
        .args(["-xzf", archive.to_str().unwrap(), "-C", tmp.to_str().unwrap()])
        .status()
        .context("running tar")?;
    if !status.success() {
        bail!("failed to extract {asset}");
    }
    let new_bin = tmp.join("ams");
    if !new_bin.is_file() {
        bail!("archive did not contain the ams binary");
    }

    let exe = fs::canonicalize(std::env::current_exe()?)?;
    // Stage in the same directory so the final rename is atomic (and works
    // while this very binary is executing).
    let staged = exe.with_file_name(".ams-update-staged");
    fs::copy(&new_bin, &staged).with_context(|| {
        format!(
            "cannot write to {} — re-run the installer instead",
            exe.parent().unwrap_or(Path::new("/")).display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
    }
    fs::rename(&staged, &exe)?;
    if !quiet {
        println!("ams updated: {current} -> {}", tag.trim_start_matches('v'));
    }
    Ok(())
}

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn scopeguard(p: PathBuf) -> Cleanup {
    Cleanup(p)
}
