//! Release check and `bungkus-mc update`.
//!
//! Port of osbrjp/bungkus-cli `pkg/update.go` @68f6c2e (v1.7.0), with one
//! change: the repo is private, so the latest tag comes from the user's
//! authenticated `gh` CLI (`gh release view`) instead of an anonymous API
//! call, and the installer is fetched with `gh release download` at that
//! tag. mc never reads or stores a GitHub token (ARCHITECTURE §11,
//! SECURITY.md "Update check / self-update").

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

/// The repository releases are published to.
const REPO: &str = "osbrjp/bungkus-mc";

/// How long the release lookup may take (SECURITY.md: 3 s).
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);

/// How long a cached answer is trusted: one check a day.
const CHECK_TTL: Duration = Duration::from_hours(24);

/// Returns whether `latest` is a newer release than `current`; an
/// unparseable version on either side is never newer.
#[must_use]
pub(crate) fn is_newer(current: &str, latest: &str) -> bool {
    let parse = |v: &str| semver::Version::parse(v.trim().trim_start_matches('v')).ok();
    match (parse(current), parse(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

/// Runs `gh release view` and returns the latest tag, or `None` when `gh`
/// is missing, not logged in, slow or answers something that is not a
/// version.
fn latest_release() -> Option<String> {
    let mut child = Command::new("gh")
        .args([
            "release", "view", "--repo", REPO, "--json", "tagName", "--jq", ".tagName",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    while child.try_wait().ok()?.is_none() {
        if started.elapsed() > LOOKUP_TIMEOUT {
            // reason: a hung gh just means no update notice this time.
            let _ = child.kill();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().ok()?;
    let tag = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    is_newer("0.0.0", &tag).then_some(tag)
}

/// Returns the latest tag, asking `fetch` only when the cache at `path` is
/// older than [`CHECK_TTL`]; the fetched tag is written back (0600). A
/// cached value that is not a version is ignored.
fn cached_latest(
    path: &Path,
    now: SystemTime,
    fetch: impl FnOnce() -> Option<String>,
) -> Option<String> {
    let fresh = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .is_ok_and(|at| now.duration_since(at).unwrap_or(Duration::ZERO) < CHECK_TTL);
    if fresh
        && let Ok(tag) = std::fs::read_to_string(path)
        && is_newer("0.0.0", tag.trim())
    {
        return Some(tag.trim().to_owned());
    }
    let tag = fetch()?;
    // reason: an unwritable cache only costs another lookup tomorrow.
    let _ = crate::store::write_atomic(path, tag.as_bytes());
    Some(tag)
}

/// Returns where the daily check caches its answer: the user cache dir
/// (`~/Library/Caches` on macOS, `$XDG_CACHE_HOME` or `~/.cache` elsewhere).
#[must_use]
pub(crate) fn cache_path(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let home = var("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home?.join("Library/Caches")
    } else {
        var("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or(home.map(|h| h.join(".cache")))?
    };
    Some(base.join("bungkus-mc/latest-release"))
}

/// Returns the newer release tag, or `None` when up to date, the check is
/// disabled (`BUNGKUS_NO_UPDATE_CHECK`), or it could not run.
#[must_use]
pub(crate) fn available(var: impl Fn(&str) -> Option<String>) -> Option<String> {
    if var("BUNGKUS_NO_UPDATE_CHECK").is_some() {
        return None;
    }
    let path = cache_path(&var)?;
    let latest = cached_latest(&path, SystemTime::now(), latest_release)?;
    is_newer(env!("CARGO_PKG_VERSION"), &latest).then_some(latest)
}

/// Runs `bungkus-mc update [--check]`.
///
/// `--check` prints whether a newer release exists. Otherwise the
/// installer attached to the latest release is downloaded with `gh` and
/// run with `bash` (the one documented shell use for updates, SECURITY.md);
/// it verifies the binary against `checksums.txt` itself.
///
/// # Errors
///
/// A message when `gh` is missing or not logged in, or the installer fails.
pub(crate) fn run(check_only: bool) -> anyhow::Result<String> {
    use anyhow::{Context, bail};
    let current = env!("CARGO_PKG_VERSION");
    let latest = latest_release().context("could not ask GitHub for the latest release; is gh installed and logged in (gh auth login)?")?;
    if !is_newer(current, &latest) {
        return Ok(format!("bungkus-mc {current} is up to date"));
    }
    if check_only {
        return Ok(format!(
            "bungkus-mc {latest} is available (you have {current}); run: bungkus-mc update"
        ));
    }
    let dir = std::env::temp_dir().join(format!("bungkus-mc-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir).context("creating a temp dir")?;
    let got = Command::new("gh")
        .args([
            "release",
            "download",
            &latest,
            "--repo",
            REPO,
            "--pattern",
            "install.sh",
            "--dir",
        ])
        .arg(&dir)
        .status()
        .context("running gh release download")?;
    if !got.success() {
        bail!("could not download install.sh for {latest}");
    }
    let status = Command::new("bash")
        .arg(dir.join("install.sh"))
        .env("BUNGKUS_MC_VERSION", &latest)
        .status()
        .context("running the installer")?;
    // reason: a leftover temp dir is harmless.
    let _ = std::fs::remove_dir_all(&dir);
    if !status.success() {
        bail!("the installer failed");
    }
    Ok(format!("updated bungkus-mc to {latest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_like_semver() {
        let cases = [
            ("0.1.0", "v0.2.0", true),
            ("0.1.0", "0.1.0", false),
            ("0.2.0", "v0.1.9", false),
            ("0.1.0", "v0.1.1-rc.1", true),
            ("dev", "v9.9.9", false),
            ("0.1.0", "2026.09.week5.release1", false),
            ("0.1.0", "", false),
        ];
        for (current, latest, want) in cases {
            assert_eq!(is_newer(current, latest), want, "{current} -> {latest}");
        }
    }

    #[test]
    fn caches_the_answer_for_a_day_and_ignores_junk() {
        let dir = std::env::temp_dir().join(format!("mc-upd-{}", std::process::id()));
        let path = dir.join("latest-release");
        let now = SystemTime::now();
        assert_eq!(
            cached_latest(&path, now, || Some("v0.2.0".into())).as_deref(),
            Some("v0.2.0")
        );
        assert_eq!(
            cached_latest(&path, now, || panic!("cached")).as_deref(),
            Some("v0.2.0")
        );
        let later = now + CHECK_TTL + Duration::from_secs(1);
        assert_eq!(
            cached_latest(&path, later, || Some("v0.3.0".into())).as_deref(),
            Some("v0.3.0")
        );
        std::fs::write(&path, "\u{1b}]0;evil\u{7}").unwrap();
        assert_eq!(
            cached_latest(&path, now, || None),
            None,
            "junk in the cache is never shown"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_check_can_be_disabled() {
        let var = |n: &str| (n == "BUNGKUS_NO_UPDATE_CHECK").then(|| "1".to_owned());
        assert_eq!(available(var), None);
    }
}
