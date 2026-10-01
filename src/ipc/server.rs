//! The per-process unix socket the hook subcommand writes to.
//!
//! Directory `clean(${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}})/bungkus-mc-<uid>/`,
//! created 0700 and checked before use (a real directory, ours, mode 0700,
//! not a symlink); socket `<pid>.sock`, set to 0600 right after `bind`. If
//! any check fails or the path is too long, mc runs without a socket and
//! every card is "output only" (SECURITY.md). Each connection carries one
//! line of at most 1 MiB within 2 s; it is handed to the UI thread as-is
//! and parsed there.

use std::fs::{DirBuilder, Permissions};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::mpsc::SyncSender;
use std::thread;
use std::time::Duration;

/// Most bytes read from one connection (1 MiB, SECURITY.md).
const LINE_MAX: u64 = 1024 * 1024;

/// Read deadline per connection (SECURITY.md).
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Longest socket path `sun_path` allows on macOS (ARCHITECTURE §3).
const PATH_MAX: usize = 104;

/// Why the socket could not be opened.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ServerError {
    /// The directory exists but is not a private directory of ours.
    #[error("{0} is not a private directory owned by you")]
    UnsafeDir(PathBuf),
    /// The socket path is longer than `sun_path` allows.
    #[error("socket path is too long")]
    PathTooLong,
    /// Creating the directory or binding failed.
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// A listening socket; the file is removed when this is dropped.
#[derive(Debug)]
pub(crate) struct Server {
    /// Where the socket is (the value of `BUNGKUS_MC_SOCK`).
    pub path: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        // reason: a stale socket file is harmless; the next run uses its own pid.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Returns the socket directory for `uid`, from `XDG_RUNTIME_DIR`, then
/// `TMPDIR`, then `/tmp`.
#[must_use]
pub(crate) fn socket_dir(var: impl Fn(&str) -> Option<String>, uid: u32) -> PathBuf {
    let base = var("XDG_RUNTIME_DIR")
        .or_else(|| var("TMPDIR"))
        .filter(|p| Path::new(p).is_absolute())
        .unwrap_or_else(|| "/tmp".to_owned());
    let clean: PathBuf = Path::new(&base).components().collect();
    clean.join(format!("bungkus-mc-{uid}"))
}

/// Opens the socket in `dir` and starts the listener thread, which sends
/// every received line to `events` as `wrap(bytes)`.
///
/// # Errors
///
/// * [`ServerError::UnsafeDir`] - `dir` is a symlink, not a directory, not
///   ours, or not mode 0700.
/// * [`ServerError::PathTooLong`] - the socket path exceeds 104 bytes.
/// * [`ServerError::Io`] - creating the directory or binding failed.
pub(crate) fn start<E: Send + 'static>(
    dir: &Path,
    uid: u32,
    events: SyncSender<E>,
    wrap: fn(Vec<u8>) -> E,
) -> Result<Server, ServerError> {
    let path = dir.join(format!("{}.sock", std::process::id()));
    if path.as_os_str().len() > PATH_MAX {
        return Err(ServerError::PathTooLong);
    }
    match DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    let private = meta.is_dir() && meta.uid() == uid && meta.mode() & 0o777 == 0o700;
    if !private {
        return Err(ServerError::UnsafeDir(dir.to_path_buf()));
    }
    // reason: a leftover file from a crashed run with the same pid.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, Permissions::from_mode(0o600))?;
    thread::spawn(move || {
        for stream in listener.incoming().filter_map(Result::ok) {
            // reason: macOS refuses the option (EINVAL) once the peer has
            // closed, which a hook usually has; reading then cannot block.
            let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
            let mut line = Vec::new();
            let read = BufReader::new(stream.take(LINE_MAX)).read_until(b'\n', &mut line);
            if read.is_ok() && !line.is_empty() && events.send(wrap(line)).is_err() {
                break;
            }
        }
    });
    Ok(Server { path })
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::sync::mpsc;

    use super::*;

    fn uid() -> u32 {
        rustix::process::getuid().as_raw()
    }

    fn temp(name: &str) -> PathBuf {
        let dir = PathBuf::from("/tmp").join(format!("mc-srv-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn round_trips_a_line_with_private_modes() {
        let dir = temp("ok");
        let (tx, rx) = mpsc::sync_channel(4);
        let server = start(&dir, uid(), tx, |b| b).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(&dir), mode(&server.path)), (0o700, 0o600));
        UnixStream::connect(&server.path)
            .unwrap()
            .write_all(b"{\"a\":1}\n")
            .unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            b"{\"a\":1}\n"
        );
        let path = server.path.clone();
        drop(server);
        assert!(!path.exists(), "socket removed on drop");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn caps_oversize_lines() {
        let dir = temp("big");
        let (tx, rx) = mpsc::sync_channel(4);
        let server = start(&dir, uid(), tx, |b| b).unwrap();
        let mut stream = UnixStream::connect(&server.path).unwrap();
        let _ = stream.write_all(&vec![b'x'; 2 * 1024 * 1024]);
        drop(stream);
        let got = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(u64::try_from(got.len()).unwrap(), LINE_MAX);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refuses_unsafe_directories_and_long_paths() {
        let open = temp("open");
        std::fs::create_dir_all(&open).unwrap();
        std::fs::set_permissions(&open, Permissions::from_mode(0o755)).unwrap();
        let (tx, _rx) = mpsc::sync_channel::<Vec<u8>>(1);
        assert!(matches!(
            start(&open, uid(), tx.clone(), |b| b),
            Err(ServerError::UnsafeDir(_))
        ));
        let link = temp("link");
        std::os::unix::fs::symlink(&open, &link).unwrap();
        assert!(matches!(
            start(&link, uid(), tx.clone(), |b| b),
            Err(ServerError::UnsafeDir(_))
        ));
        let long = PathBuf::from("/tmp").join("d".repeat(120));
        assert!(matches!(
            start(&long, uid(), tx, |b| b),
            Err(ServerError::PathTooLong)
        ));
        std::fs::remove_file(&link).unwrap();
        std::fs::remove_dir_all(&open).unwrap();
    }

    #[test]
    fn picks_the_socket_dir_from_the_environment() {
        let var = |pairs: &'static [(&str, &str)]| {
            move |n: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == n)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        let cases: [(&'static [(&str, &str)], &str); 4] = [
            (
                &[("XDG_RUNTIME_DIR", "/run/user/501"), ("TMPDIR", "/t")],
                "/run/user/501/bungkus-mc-7",
            ),
            (
                &[("TMPDIR", "/var/folders/x/T/")],
                "/var/folders/x/T/bungkus-mc-7",
            ),
            (&[("TMPDIR", "relative")], "/tmp/bungkus-mc-7"),
            (&[], "/tmp/bungkus-mc-7"),
        ];
        for (env, want) in cases {
            assert_eq!(socket_dir(var(env), 7), PathBuf::from(want), "{env:?}");
        }
    }
}
