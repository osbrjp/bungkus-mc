//! The debug log (`--debug`): timestamped lines appended to `mc.log` next
//! to `sessions.json`, mode 0600 (ARCHITECTURE §7, `TECH_STACK.md` "no
//! logging crate").
//!
//! It records what mc decides (launches, exits, hook event names, stops,
//! moves, failures) and never what the user types, a prompt, an API key or
//! agent output. Without `--debug` the file is never opened and
//! [`debug_log!`](crate::debug_log) costs one uncontended lock.

use std::fmt::Arguments;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::sync::Mutex;

/// The open log, when `--debug` is on.
static LOG: Mutex<Option<File>> = Mutex::new(None);

/// Opens (appending) the debug log at `path`, creating it 0600 and its
/// folder 0700.
///
/// # Errors
///
/// The I/O error of creating the folder or opening the file.
pub(crate) fn open(path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    if let Ok(mut log) = LOG.lock() {
        *log = Some(file);
    }
    Ok(())
}

/// Appends one line, `<unix seconds.millis> <module>: <message>`, when the
/// log is open; a failed write is dropped (the log must never break mc).
pub(crate) fn write(module: &str, message: Arguments<'_>) {
    let Ok(mut log) = LOG.lock() else {
        return;
    };
    let Some(file) = log.as_mut() else {
        return;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    // reason: a debug line that cannot be written is simply lost.
    let _ = writeln!(
        file,
        "{}.{:03} {module}: {message}",
        now.as_secs(),
        now.subsec_millis()
    );
}

/// Writes a line to the debug log (`--debug`), formatted like `format!`.
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        $crate::store::debug::write(module_path!(), format_args!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn writes_timestamped_lines_to_a_private_file() {
        let dir = std::env::temp_dir().join(format!("mc-debug-{}", std::process::id()));
        let path = dir.join("mc.log");
        open(&path).unwrap();
        crate::debug_log!("launch {} in {}", "claude", "/w/app");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("bungkus_mc::store::debug::tests: launch claude in /w/app"),
            "{text}"
        );
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        if let Ok(mut log) = LOG.lock() {
            *log = None;
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
