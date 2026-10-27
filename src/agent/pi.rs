//! Pi's per-launch extension and session-id rules.
//!
//! mc writes the bundled extension into its verified private runtime
//! directory and passes it with `pi --extension`. The extension only emits
//! documented lifecycle metadata through `bungkus-mc pi-hook`; this module
//! never reads Pi session files.

use std::path::{Path, PathBuf};

/// The bundled extension loaded into every hooked Pi session.
const EXTENSION: &[u8] = include_bytes!("pi-extension.js");

/// The runtime filename shared by mc instances; its content is identical.
const EXTENSION_FILE: &str = "pi-bungkus-mc.js";

/// Writes the bundled Pi extension into the verified private runtime directory.
///
/// The file contains no executable path or secret. Each Pi child receives the
/// current executable separately through `BUNGKUS_MC_EXE`.
///
/// # Arguments
///
/// * `dir` - The private directory already verified by the socket server.
///
/// # Returns
///
/// The absolute path passed to Pi's `--extension` option.
///
/// # Errors
///
/// Returns the filesystem error from the atomic 0600 write.
pub(crate) fn install_extension(dir: &Path) -> std::io::Result<PathBuf> {
    let path = dir.join(EXTENSION_FILE);
    if std::fs::read(&path).is_ok_and(|current| current == EXTENSION) {
        return Ok(path);
    }
    crate::store::write_atomic(&path, EXTENSION)?;
    Ok(path)
}

/// Returns whether `id` is safe to pass to Pi's `--session` option.
///
/// Pi documents ids as letters, numbers, `.`, `_`, and `-`, with an
/// alphanumeric first and last character. mc also applies its socket id cap.
#[must_use]
pub(crate) fn valid_session_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    let edge = |byte: u8| byte.is_ascii_alphanumeric();
    !bytes.is_empty()
        && bytes.len() <= 64
        && edge(bytes[0])
        && edge(bytes[bytes.len() - 1])
        && bytes
            .iter()
            .all(|byte| edge(*byte) || matches!(*byte, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn installs_a_private_stable_extension() {
        let dir = std::env::temp_dir().join(format!("mc-pi-extension-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = install_extension(&dir).unwrap();
        let second = install_extension(&dir).unwrap();
        assert_eq!(first, second);
        assert_eq!(std::fs::read(&first).unwrap(), EXTENSION);
        assert_eq!(
            std::fs::metadata(&first).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn validates_only_documented_bounded_session_ids() {
        for valid in ["a", "abc-123", "one.two_three", &"a".repeat(64)] {
            assert!(valid_session_id(valid), "{valid:?}");
        }
        for invalid in ["", "-abc", "abc-", "a/b", "a b", "é", &"a".repeat(65)] {
            assert!(!valid_session_id(invalid), "{invalid:?}");
        }
    }
}
