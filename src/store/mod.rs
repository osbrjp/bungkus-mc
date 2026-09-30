//! mc's own files: where they live and how they are read and written.
//!
//! Every path mc writes is under the XDG directories resolved here
//! (ARCHITECTURE §7). Directories are created 0700, files 0600, and every
//! write is atomic (temp file + `rename`). Environment variables that name
//! these directories are read here and nowhere else.

pub(crate) mod config;
pub(crate) mod state;

use std::fs::{DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// Returns `$XDG_CONFIG_HOME/bungkus/mc/config.json`, or the same under
/// `~/.config`; `None` when neither variable is set.
///
/// # Arguments
///
/// * `var` - Looks up an environment variable by name.
#[must_use]
pub(crate) fn config_file(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    xdg_dir(&var, "XDG_CONFIG_HOME", ".config").map(|d| d.join("bungkus/mc/config.json"))
}

/// Resolves an XDG base directory: the variable when it holds an absolute
/// path (the XDG spec ignores relative ones), else `$HOME/<fallback>`.
fn xdg_dir(var: &impl Fn(&str) -> Option<String>, name: &str, fallback: &str) -> Option<PathBuf> {
    var(name)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| var("HOME").map(|home| Path::new(&home).join(fallback)))
}

/// Writes `bytes` to `path` atomically with mode 0600, creating the parent
/// directory with mode 0700 when missing.
///
/// # Errors
///
/// Returns the I/O error if the directory, the temp file or the rename
/// fails; the original file is then untouched.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        // reason: best-effort cleanup; the write error is what gets reported.
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn resolves_the_config_file_from_xdg_or_home() {
        type Case<'a> = (&'a [(&'a str, &'a str)], Option<&'a str>);
        let cases: &[Case] = &[
            (
                &[("XDG_CONFIG_HOME", "/x"), ("HOME", "/h")],
                Some("/x/bungkus/mc/config.json"),
            ),
            (
                &[("XDG_CONFIG_HOME", "rel"), ("HOME", "/h")],
                Some("/h/.config/bungkus/mc/config.json"),
            ),
            (&[("HOME", "/h")], Some("/h/.config/bungkus/mc/config.json")),
            (&[], None),
        ];
        for (env, want) in cases {
            let got = config_file(|n| env.iter().find(|(k, _)| *k == n).map(|(_, v)| (*v).into()));
            assert_eq!(got, want.map(PathBuf::from), "{env:?}");
        }
    }

    #[test]
    fn writes_atomically_with_private_modes() {
        let root = std::env::temp_dir().join(format!("mc-store-{}", std::process::id()));
        let path = root.join("a/b/file.json");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1,
            "no temp left"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
