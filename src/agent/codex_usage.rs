//! The Codex usage reader: the one place mc reads an agent's session log
//! (ARCHITECTURE §6.3, SECURITY.md "Codex session log reader", approved).
//!
//! It reads only the `token_count` records of the rollout file Codex's own
//! `SessionStart` hook names, only under `CODEX_HOME`, tail-only and capped,
//! and keeps nothing but the numbers. Any error stops the reader; the card
//! then shows `-`. Nothing else in mc opens a transcript.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::time::Duration;

use serde::Deserialize;

use crate::agent::usage::{Usage, Window};

/// Most bytes read per poll, and the tail read on attach (256 KiB).
const CHUNK: u64 = 256 * 1024;

/// Poll interval (ARCHITECTURE §6.3).
const POLL: Duration = Duration::from_secs(1);

/// The byte pattern a line must contain before it is parsed at all.
const NEEDLE: &[u8] = b"\"token_count\"";

/// Why a rollout path is refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum PathError {
    /// The path or `CODEX_HOME` does not resolve.
    #[error("path does not resolve")]
    Unresolved,
    /// The resolved path is not inside `CODEX_HOME`.
    #[error("path is outside CODEX_HOME")]
    Outside,
    /// The file is not a `.jsonl` file.
    #[error("not a .jsonl file")]
    Extension,
    /// The opened file is not a regular file.
    #[error("not a regular file")]
    NotRegular,
}

/// Opens a rollout file after validating it (SECURITY.md): both paths are
/// canonicalised, the file must lie component-wise inside `codex_home`,
/// end in `.jsonl`, and the *opened* descriptor must be a regular file.
///
/// # Errors
///
/// A [`PathError`] naming the failed check.
pub(crate) fn open(path: &Path, codex_home: &Path) -> Result<File, PathError> {
    let resolved = path.canonicalize().map_err(|_| PathError::Unresolved)?;
    let home = codex_home
        .canonicalize()
        .map_err(|_| PathError::Unresolved)?;
    resolved
        .strip_prefix(&home)
        .map_err(|_| PathError::Outside)?;
    if resolved.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return Err(PathError::Extension);
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits().cast_signed())
        .open(&resolved)
        .map_err(|_| PathError::Unresolved)?;
    let meta = file.metadata().map_err(|_| PathError::Unresolved)?;
    if meta.is_file() {
        Ok(file)
    } else {
        Err(PathError::NotRegular)
    }
}

/// The tail position in the rollout file.
#[derive(Debug, Default)]
pub(crate) struct Tail {
    /// Next byte to read.
    offset: u64,
    /// A partial line carried to the next read.
    carry: Vec<u8>,
    /// Whether bytes up to the next newline must be dropped (after a skip).
    resync: bool,
}

impl Tail {
    /// Reads what was appended since the last call and returns the newest
    /// usage found, if any.
    ///
    /// A shrunk file restarts from 0; more than [`CHUNK`] new bytes skip
    /// ahead to the newest `CHUNK`, dropping the partial first line; a
    /// partial last line is carried, and a carry reaching `CHUNK` without a
    /// newline is dropped up to the next one.
    ///
    /// # Errors
    ///
    /// The I/O error of `metadata` or `read_at`; the reader then stops.
    pub(crate) fn poll(&mut self, file: &File) -> std::io::Result<Option<Usage>> {
        let len = file.metadata()?.len();
        if len < self.offset {
            *self = Self::default();
        }
        if len - self.offset > CHUNK {
            self.offset = len - CHUNK;
            self.carry.clear();
            self.resync = true;
        }
        let want = usize::try_from(len - self.offset).unwrap_or(0);
        let mut buf = vec![0_u8; want];
        let read = file.read_at(&mut buf, self.offset)?;
        buf.truncate(read);
        self.offset += read as u64;
        let mut data = std::mem::take(&mut self.carry);
        data.extend_from_slice(&buf);
        let mut lines: Vec<&[u8]> = data.split(|b| *b == b'\n').collect();
        let partial = lines.pop().unwrap_or_default();
        if self.resync && !lines.is_empty() {
            lines.remove(0);
            self.resync = false;
        }
        let newest = lines.iter().rev().find_map(|l| parse(l));
        if partial.len() < usize::try_from(CHUNK).unwrap_or(usize::MAX) {
            self.carry = partial.to_vec();
        } else {
            self.resync = true;
        }
        Ok(newest)
    }
}

/// The only record shape mc deserialises from a rollout (tolerant: unknown
/// fields are ignored, every field is optional).
#[derive(Debug, Deserialize)]
struct Record {
    #[serde(rename = "type")]
    kind: Option<String>,
    payload: Option<Payload>,
}

/// `payload` of a `token_count` record.
#[derive(Debug, Deserialize)]
struct Payload {
    #[serde(rename = "type")]
    kind: Option<String>,
    info: Option<Info>,
    rate_limits: Option<RateLimits>,
}

/// `payload.info`.
#[derive(Debug, Deserialize)]
struct Info {
    total_token_usage: Option<Tokens>,
    last_token_usage: Option<Tokens>,
    model_context_window: Option<u64>,
}

/// A token breakdown.
#[expect(
    clippy::struct_field_names,
    reason = "mirrors Codex's JSON field names"
)]
#[derive(Debug, Deserialize)]
struct Tokens {
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

/// `payload.rate_limits`.
#[derive(Debug, Deserialize)]
struct RateLimits {
    primary: Option<Limit>,
    secondary: Option<Limit>,
}

/// One rate-limit window.
#[derive(Debug, Deserialize)]
struct Limit {
    used_percent: Option<f64>,
    window_minutes: Option<u64>,
    resets_at: Option<u64>,
}

/// Parses one line into usage when it is a `token_count` record; every
/// other line (and malformed JSON) is `None`. Lines without the needle are
/// never deserialised.
fn parse(line: &[u8]) -> Option<Usage> {
    if !line.windows(NEEDLE.len()).any(|w| w == NEEDLE) {
        return None;
    }
    let record: Record = serde_json::from_slice(line).ok()?;
    let payload = record
        .payload
        .filter(|p| p.kind.as_deref() == Some("token_count"))?;
    if record.kind.as_deref() != Some("event_msg") {
        return None;
    }
    let info = payload.info;
    let total = info.as_ref().and_then(|i| i.total_token_usage.as_ref());
    let size = info.as_ref().and_then(|i| i.model_context_window);
    let last = info
        .as_ref()
        .and_then(|i| i.last_token_usage.as_ref())
        .and_then(|t| t.total_tokens);
    let ctx_pct = match (last, size) {
        (Some(used), Some(size)) if size > 0 => u32::try_from(used.saturating_mul(1000) / size)
            .ok()
            .map(|p| f64::from(p) / 10.0),
        _ => None,
    };
    let window = |l: Option<Limit>| {
        let l = l?;
        let label = match l.window_minutes? {
            300 => "5h".to_owned(),
            10_080 => "7d".to_owned(),
            m => format!("{}h", m / 60),
        };
        Some(Window {
            label,
            used_pct: l.used_percent?,
            resets_at: l.resets_at,
        })
    };
    let limits = payload
        .rate_limits
        .map(|r| {
            [window(r.primary), window(r.secondary)]
                .into_iter()
                .flatten()
                .collect()
        })
        .unwrap_or_default();
    Some(Usage {
        session_name: None,
        input: total.and_then(|t| t.input_tokens),
        output: total.and_then(|t| t.output_tokens),
        cache_read: total.and_then(|t| t.cached_input_tokens),
        cache_write: None,
        cost_usd: None,
        ctx_pct,
        ctx_size: size,
        limits,
        added_dirs: Vec::new(),
    })
}

/// Returns `CODEX_HOME`, else `~/.codex`.
#[must_use]
pub(crate) fn codex_home(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    var("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| var("HOME").map(|h| PathBuf::from(h).join(".codex")))
}

/// Tails `file` every second, sending each new usage as `wrap(usage)`,
/// until `stop` fires or is dropped, the channel closes, or a read fails.
pub(crate) fn watch<E: Send + 'static>(
    file: File,
    stop: Receiver<()>,
    events: SyncSender<E>,
    wrap: impl Fn(Usage) -> E + Send + 'static,
) {
    std::thread::spawn(move || {
        let mut tail = Tail::default();
        loop {
            match tail.poll(&file) {
                Ok(Some(usage)) => {
                    if events.send(wrap(usage)).is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                Err(_) => return,
            }
            match stop.recv_timeout(POLL) {
                Err(RecvTimeoutError::Timeout) => {}
                Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    const ROLLOUT: &str = include_str!("testdata/codex/rollout.jsonl");

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mc-cx-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("home/sessions")).unwrap();
        dir
    }

    #[test]
    fn reads_only_event_msg_token_count_records() {
        let usages: Vec<Usage> = ROLLOUT
            .lines()
            .filter_map(|l| parse(l.as_bytes()))
            .collect();
        assert_eq!(
            usages.len(),
            4,
            "three records plus the null-info one; decoys skipped"
        );
        let last = usages.last().unwrap();
        assert_eq!(
            (last.input, last.output, last.cache_read),
            (Some(52_945), Some(105), Some(47_744))
        );
        assert_eq!(last.ctx_size, Some(258_400));
        assert_eq!(last.ctx_pct, Some(6.8));
        assert_eq!(last.cost_usd, None, "Codex has no USD cost");
        let labels: Vec<&str> = last.limits.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["5h", "7d"]);
        assert!(
            usages.iter().all(|u| u.input != Some(999_999_999)),
            "token_usage_record decoy"
        );
        let null = usages.iter().find(|u| u.input.is_none()).unwrap();
        assert_eq!(
            (null.ctx_pct, null.limits.len()),
            (None, 0),
            "null info and rate_limits"
        );
    }

    #[test]
    fn refuses_paths_outside_codex_home_and_special_files() {
        let dir = temp("paths");
        let home = dir.join("home");
        let good = home.join("sessions/rollout-1.jsonl");
        std::fs::write(&good, ROLLOUT).unwrap();
        assert!(open(&good, &home).is_ok());
        let evil_home = dir.join("home-evil");
        std::fs::create_dir_all(&evil_home).unwrap();
        std::fs::write(evil_home.join("x.jsonl"), "").unwrap();
        let cases = [
            (evil_home.join("x.jsonl"), PathError::Outside),
            (
                home.join("sessions/../../home-evil/x.jsonl"),
                PathError::Outside,
            ),
            (home.join("sessions/rollout-1.txt"), PathError::Unresolved),
        ];
        for (path, want) in cases {
            assert_eq!(open(&path, &home).unwrap_err(), want, "{}", path.display());
        }
        std::fs::write(home.join("sessions/notes.txt"), "").unwrap();
        assert_eq!(
            open(&home.join("sessions/notes.txt"), &home).unwrap_err(),
            PathError::Extension
        );
        std::os::unix::fs::symlink(evil_home.join("x.jsonl"), home.join("sessions/link.jsonl"))
            .unwrap();
        assert_eq!(
            open(&home.join("sessions/link.jsonl"), &home).unwrap_err(),
            PathError::Outside
        );
        std::fs::create_dir_all(home.join("sessions/dir.jsonl")).unwrap();
        assert_eq!(
            open(&home.join("sessions/dir.jsonl"), &home).unwrap_err(),
            PathError::NotRegular
        );
        let fifo = home.join("sessions/fifo.jsonl");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(made.success());
        assert_eq!(open(&fifo, &home).unwrap_err(), PathError::NotRegular);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tails_appends_carries_partial_lines_and_restarts_on_shrink() {
        let dir = temp("tail");
        let path = dir.join("home/sessions/r.jsonl");
        let record = ROLLOUT
            .lines()
            .rfind(|l| parse(l.as_bytes()).is_some())
            .unwrap();
        std::fs::write(&path, "").unwrap();
        let file = open(&path, &dir.join("home")).unwrap();
        let mut tail = Tail::default();
        assert_eq!(tail.poll(&file).unwrap(), None);
        let (head, rest) = record.split_at(40);
        let mut w = OpenOptions::new().append(true).open(&path).unwrap();
        w.write_all(head.as_bytes()).unwrap();
        assert_eq!(tail.poll(&file).unwrap(), None, "partial line carried");
        w.write_all(format!("{rest}\n").as_bytes()).unwrap();
        assert!(tail.poll(&file).unwrap().is_some(), "completed line parsed");
        w.write_all(b"{\"type\":\"filler\"}\n").unwrap();
        assert_eq!(tail.poll(&file).unwrap(), None);
        std::fs::write(&path, format!("{record}\n")).unwrap();
        let shrunk = open(&path, &dir.join("home")).unwrap();
        assert!(
            tail.poll(&shrunk).unwrap().is_some(),
            "shrink restarts from 0"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_burst_over_the_cap_skips_ahead_and_drops_the_partial_first_line() {
        let dir = temp("burst");
        let path = dir.join("home/sessions/r.jsonl");
        let record = ROLLOUT
            .lines()
            .rfind(|l| parse(l.as_bytes()).is_some())
            .unwrap();
        let filler = format!("{{\"type\":\"x\",\"pad\":\"{}\"}}\n", "y".repeat(1000));
        let mut body = filler.repeat(400);
        body.push_str(record);
        body.push('\n');
        std::fs::write(&path, &body).unwrap();
        let file = open(&path, &dir.join("home")).unwrap();
        let mut tail = Tail::default();
        let usage = tail.poll(&file).unwrap();
        assert!(
            usage.is_some(),
            "the newest record inside the last 256 KiB is found"
        );
        assert_eq!(tail.offset, body.len() as u64);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
