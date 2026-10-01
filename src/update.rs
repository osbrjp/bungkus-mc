//! Release check, `bungkus-mc update` and `bungkus-mc uninstall`.
//!
//! As osbrjp/bungkus-cli `pkg/update.go`: the latest tag comes from the
//! public GitHub API and an update re-runs the install script from the
//! repository's `main`, which verifies the binary against the release's
//! `checksums.txt`. Both go through `curl` with a fixed argv, the tool the
//! installer needs anyway, so mc carries no HTTP client (ARCHITECTURE §11,
//! SECURITY.md "Update check / self-update").

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

/// The latest-release endpoint of the public GitHub API.
const LATEST_API: &str = "https://api.github.com/repos/osbrjp/bungkus-mc/releases/latest";

/// The install script, as the README's one-line install uses it.
const INSTALL_URL: &str = "https://raw.githubusercontent.com/osbrjp/bungkus-mc/main/install.sh";

/// Most bytes read from the release API (64 KiB).
const API_MAX: u64 = 64 * 1024;

/// How long the release lookup may take (SECURITY.md: 3 s).
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);

/// How long a cached answer is trusted, and how often a running mc looks
/// again: one check an hour.
pub(crate) const CHECK_TTL: Duration = Duration::from_hours(1);

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

/// Asks the GitHub API for the latest release with `curl` and returns its
/// tag, or `None` when curl is missing, offline, slow, rate-limited or the
/// answer holds no version.
fn latest_release() -> Option<String> {
    use std::io::Read;
    let mut child = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "3",
            "-H",
            "Accept: application/vnd.github+json",
        ])
        .arg(LATEST_API)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    while child.try_wait().ok()?.is_none() {
        if started.elapsed() > LOOKUP_TIMEOUT + Duration::from_secs(1) {
            // reason: a hung lookup just means no update notice this time.
            let _ = child.kill();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut body = Vec::new();
    child
        .stdout
        .take()?
        .take(API_MAX)
        .read_to_end(&mut body)
        .ok()?;
    tag_of(&body)
}

/// Returns the `tag_name` of a GitHub release JSON when it is a version.
#[must_use]
fn tag_of(body: &[u8]) -> Option<String> {
    let release: serde_json::Value = serde_json::from_slice(body).ok()?;
    let tag = release.get("tag_name")?.as_str()?.trim().to_owned();
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

/// Why an update could not be installed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum UpdateError {
    /// No release came back: none published yet, offline, or `curl` is
    /// missing.
    #[error("no release found (none published yet, offline, or curl missing)")]
    NoRelease,
    /// Downloading the install script failed.
    #[error("could not download install.sh for {0}")]
    Download(String),
    /// The installer exited non-zero (in mc: it may have needed sudo).
    #[error("the installer failed")]
    Installer,
    /// A temp folder or process could not be created.
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// Downloads the install script with `curl` and runs it with `bash` for
/// release `latest` (the one documented shell use for updates,
/// SECURITY.md); it verifies the binary against `checksums.txt` itself.
/// The running binary's real path goes along as `BUNGKUS_CURRENT_BIN`, so
/// the copy that runs is the one updated (ARCHITECTURE §11). `quiet` sends
/// the installer's output nowhere (inside the TUI) and gives it no
/// terminal, so a sudo prompt fails instead of hanging.
///
/// # Errors
///
/// [`UpdateError::Download`], [`UpdateError::Installer`] or an I/O error.
pub(crate) fn install(latest: &str, quiet: bool) -> Result<(), UpdateError> {
    let dir = std::env::temp_dir().join(format!("bungkus-mc-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let script = dir.join("install.sh");
    let io = || {
        if quiet {
            Stdio::null()
        } else {
            Stdio::inherit()
        }
    };
    let got = Command::new("curl")
        .args(["-fsSL", "--max-time", "30", "-o"])
        .arg(&script)
        .arg(INSTALL_URL)
        .stdin(Stdio::null())
        .stdout(io())
        .stderr(io())
        .status()?;
    if !got.success() {
        return Err(UpdateError::Download(latest.to_owned()));
    }
    let mut installer = Command::new("bash");
    installer
        .arg(&script)
        .env("BUNGKUS_MC_VERSION", latest)
        .stdin(io())
        .stdout(io())
        .stderr(io());
    if let Ok(exe) = std::env::current_exe().and_then(std::fs::canonicalize) {
        installer.env("BUNGKUS_CURRENT_BIN", exe);
    }
    let status = installer.status()?;
    // reason: a leftover temp dir is harmless.
    let _ = std::fs::remove_dir_all(&dir);
    if status.success() {
        Ok(())
    } else {
        Err(UpdateError::Installer)
    }
}

/// Checks for a newer release and installs it quietly (the TUI's `U`).
///
/// # Returns
///
/// The installed tag, or `None` when this version is the latest.
///
/// # Errors
///
/// As [`install`], plus [`UpdateError::NoRelease`].
pub(crate) fn check_and_install() -> Result<Option<String>, UpdateError> {
    let latest = latest_release().ok_or(UpdateError::NoRelease)?;
    if !is_newer(env!("CARGO_PKG_VERSION"), &latest) {
        return Ok(None);
    }
    install(&latest, true)?;
    Ok(Some(latest))
}

/// Runs `bungkus-mc update [--check]`: `--check` prints whether a newer
/// release exists; otherwise its installer runs in this terminal.
///
/// # Errors
///
/// A message when `gh` is missing or not logged in, or the installer fails.
pub(crate) fn run(check_only: bool) -> anyhow::Result<String> {
    let current = env!("CARGO_PKG_VERSION");
    let latest = latest_release().ok_or(UpdateError::NoRelease)?;
    if !is_newer(current, &latest) {
        return Ok(format!("bungkus-mc {current} is up to date"));
    }
    if check_only {
        return Ok(format!(
            "bungkus-mc {latest} is available (you have {current}); run: bungkus-mc update"
        ));
    }
    install(&latest, false)?;
    Ok(format!("updated bungkus-mc to {latest}"))
}

/// Runs `bungkus-mc uninstall [--purge] [--yes]`: removes this binary and
/// the `bkmc` link next to it when it points here; `--purge` also removes
/// mc's config, state (sessions, limits, debug log) and update cache. It
/// lists what goes and asks first unless `--yes`. Agents' own files are
/// never touched.
///
/// # Errors
///
/// A message when the binary cannot be found or a removal fails (a binary
/// in a root-owned folder needs `sudo rm`).
pub(crate) fn uninstall(purge: bool, yes: bool) -> anyhow::Result<String> {
    use anyhow::Context;
    use std::io::{BufRead, Write};
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .context("finding the installed binary")?;
    let mut targets = vec![exe.clone()];
    if let Some(dir) = exe.parent() {
        let short = dir.join("bkmc");
        if std::fs::read_link(&short)
            .ok()
            .map(|t| if t.is_absolute() { t } else { dir.join(t) })
            .and_then(|t| t.canonicalize().ok())
            .is_some_and(|t| t == exe)
        {
            targets.push(short);
        }
    }
    if purge {
        let var = |n: &str| std::env::var(n).ok();
        let dirs = [
            crate::store::config_file(var).and_then(|p| p.parent().map(Path::to_path_buf)),
            crate::store::state::state_file(var).and_then(|p| p.parent().map(Path::to_path_buf)),
            cache_path(var).and_then(|p| p.parent().map(Path::to_path_buf)),
        ];
        targets.extend(dirs.into_iter().flatten().filter(|p| p.exists()));
    }
    let mut out = std::io::stdout();
    writeln!(out, "bungkus-mc uninstall will remove:")?;
    for t in &targets {
        writeln!(out, "  {}", t.display())?;
    }
    if !yes {
        write!(out, "Remove them? [y/N] ")?;
        out.flush()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            return Ok("nothing removed".into());
        }
    }
    for t in &targets {
        let removed = if t.is_dir() && !t.is_symlink() {
            std::fs::remove_dir_all(t)
        } else {
            std::fs::remove_file(t)
        };
        removed
            .with_context(|| format!("removing {} (try: sudo rm {})", t.display(), t.display()))?;
    }
    Ok(if purge {
        "bungkus-mc removed, with its config and state".into()
    } else {
        "bungkus-mc removed (config and sessions kept; --purge removes them)".into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_tag_of_a_release_and_ignores_junk() {
        assert_eq!(
            tag_of(br#"{"tag_name":"v0.1.0-beta.1","name":"x"}"#).as_deref(),
            Some("v0.1.0-beta.1")
        );
        assert_eq!(tag_of(br#"{"message":"Not Found"}"#), None);
        assert_eq!(tag_of(br#"{"tag_name":"latest"}"#), None);
        assert_eq!(tag_of(b"<html>"), None);
    }

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
    fn caches_the_answer_for_an_hour_and_ignores_junk() {
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
