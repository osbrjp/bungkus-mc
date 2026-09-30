//! Agent sessions running outside bungkus-mc, shown read-only
//! (PROPOSAL §6 item 23, ARCHITECTURE §3.4).
//!
//! Claude Code lists its own live sessions with `claude agents --json`
//! (pid, cwd, name, `busy`/`idle`); Codex has no such list, so a running
//! `codex` process of this user and its working folder stand in for one.
//! mc never types into these sessions, never signals them and never reads
//! their transcripts: it only shows that they exist and what they report.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::agent::Kind;
use crate::app::sessions::State;
use crate::proc::Proc;
use crate::ui::sanitise::sanitise;

/// How long `claude agents --json` may take.
const LIST_TIMEOUT: Duration = Duration::from_secs(3);

/// Most output read from the listing command (1 MiB).
const OUTPUT_MAX: u64 = 1024 * 1024;

/// Longest session name shown.
const NAME_MAX: usize = 80;

/// One session found outside mc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct External {
    /// Which agent.
    pub kind: Kind,
    /// Its process id.
    pub pid: i32,
    /// The folder it runs in.
    pub cwd: PathBuf,
    /// Its name (Claude) or `codex`, sanitised.
    pub name: String,
    /// Claude's own status word (`busy`, `idle`, …); `None` for Codex.
    pub status: Option<String>,
    /// The agent's session id, when it reports one.
    pub session_id: Option<String>,
    /// Start time in unix milliseconds, when reported.
    pub started_ms: Option<u64>,
}

impl External {
    /// Returns the card state for this session: Claude's `busy` is working,
    /// `idle` is your turn, a status about waiting or permission needs you;
    /// Codex reports nothing, so it shows as working.
    #[must_use]
    pub(crate) fn state(&self) -> State {
        match self.status.as_deref() {
            Some("busy") | None => State::Working,
            Some(s)
                if ["wait", "input", "perm", "block", "attention"]
                    .iter()
                    .any(|w| s.contains(w)) =>
            {
                State::NeedsYou
            }
            Some(_) => State::YourTurn,
        }
    }
}

/// One row of `claude agents --json` (every field optional).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeRow {
    pid: Option<i32>,
    cwd: Option<PathBuf>,
    session_id: Option<String>,
    name: Option<String>,
    status: Option<String>,
    started_at: Option<u64>,
}

/// Parses `claude agents --json`; a malformed row is skipped and a
/// malformed listing yields nothing.
#[must_use]
pub(crate) fn parse_claude(bytes: &[u8]) -> Vec<External> {
    let rows: Vec<serde_json::Value> = serde_json::from_slice(bytes).unwrap_or_default();
    rows.into_iter()
        .filter_map(|v| serde_json::from_value::<ClaudeRow>(v).ok())
        .filter_map(|r| {
            Some(External {
                kind: Kind::Claude,
                pid: r.pid?,
                cwd: r.cwd?,
                name: sanitise(r.name.as_deref().unwrap_or("untitled"), NAME_MAX),
                status: r.status.map(|s| sanitise(&s, 20)),
                session_id: r.session_id.map(|s| sanitise(&s, 64)),
                started_ms: r.started_at,
            })
        })
        .collect()
}

/// Runs `claude agents --json` (fixed argv, no stdin, capped output,
/// [`LIST_TIMEOUT`]); any failure is an empty list.
fn claude_sessions(program: &Path) -> Vec<External> {
    let Ok(mut child) = Command::new(program)
        .args(["agents", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    let started = Instant::now();
    let mut out = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            // reason: a partial read just parses to nothing.
            let _ = stdout.take(OUTPUT_MAX).read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
        match rx.recv_timeout(LIST_TIMEOUT.saturating_sub(started.elapsed())) {
            Ok(buf) => out = buf,
            Err(_) => {
                // reason: a hung listing is killed; the next scan tries again.
                let _ = child.kill();
            }
        }
    }
    // reason: reaps the child; its exit status does not matter.
    let _ = child.wait();
    parse_claude(&out)
}

/// Picks the interactive Codex processes from a snapshot: `comm` named
/// `codex`, not the app-server daemon, and not a child of another `codex`
/// (the npm wrapper starts the native binary).
#[must_use]
pub(crate) fn codex_processes(snapshot: &[Proc]) -> Vec<&Proc> {
    let is_codex =
        |p: &Proc| p.comm.rsplit('/').next() == Some("codex") && !p.comm.contains("app-server");
    let codex_pids: Vec<i32> = snapshot
        .iter()
        .filter(|p| is_codex(p))
        .map(|p| p.pid)
        .collect();
    snapshot
        .iter()
        .filter(|p| is_codex(p) && !codex_pids.contains(&p.ppid))
        .collect()
}

/// Parses `lsof -a -d cwd -p … -Fpn`: `p<pid>` then `n<path>`.
#[must_use]
pub(crate) fn parse_cwd(text: &str) -> HashMap<i32, PathBuf> {
    let mut out = HashMap::new();
    let mut pid = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().ok();
        } else if let (Some(path), Some(p)) = (line.strip_prefix('n'), pid) {
            out.insert(p, PathBuf::from(path));
        }
    }
    out
}

/// Returns the running Codex sessions with their working folders.
fn codex_sessions(snapshot: &[Proc]) -> Vec<External> {
    let procs = codex_processes(snapshot);
    if procs.is_empty() {
        return Vec::new();
    }
    let pids: Vec<String> = procs.iter().map(|p| p.pid.to_string()).collect();
    let cwds = Command::new("lsof")
        .args(["-a", "-d", "cwd", "-p", &pids.join(","), "-Fpn"])
        .env("LC_ALL", "C")
        .output()
        .map(|o| parse_cwd(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default();
    procs
        .into_iter()
        .filter_map(|p| {
            Some(External {
                kind: Kind::Codex,
                pid: p.pid,
                cwd: cwds.get(&p.pid)?.clone(),
                name: "codex".to_owned(),
                status: None,
                session_id: None,
                started_ms: None,
            })
        })
        .collect()
}

/// Lists every agent session of this user that is running now, from both
/// agents; the caller drops the ones mc started itself.
///
/// # Arguments
///
/// * `claude` - The resolved `claude` executable, if installed.
/// * `uid`    - This user's id.
#[must_use]
pub(crate) fn scan(claude: Option<&Path>, uid: u32) -> Vec<External> {
    let mut all = claude.map(claude_sessions).unwrap_or_default();
    all.extend(codex_sessions(&crate::proc::snapshot(uid)));
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_claude_listing_and_skips_broken_rows() {
        let json = br#"[
            {"pid": 98780, "cwd": "/w/nrha", "kind": "interactive", "startedAt": 1790744521640,
             "sessionId": "a8e34add-0962-4129-bd62-670f74813675", "name": "nrha-c8", "status": "idle"},
            {"pid": 13255, "cwd": "/w/mc", "name": "\u001b]0;evil\u0007mc-7c", "status": "busy"},
            {"cwd": "/w/no-pid"},
            {"pid": "x"}
        ]"#;
        let list = parse_claude(json);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "nrha-c8");
        assert_eq!(list[0].state(), State::YourTurn);
        assert_eq!(list[1].name, "mc-7c", "names are sanitised");
        assert_eq!(list[1].state(), State::Working);
        assert!(parse_claude(b"not json").is_empty());
    }

    #[test]
    fn maps_status_words_to_states() {
        let ext = |status: Option<&str>| External {
            kind: Kind::Claude,
            pid: 1,
            cwd: "/".into(),
            name: "n".into(),
            status: status.map(Into::into),
            session_id: None,
            started_ms: None,
        };
        assert_eq!(ext(Some("waiting_for_input")).state(), State::NeedsYou);
        assert_eq!(ext(Some("idle")).state(), State::YourTurn);
        assert_eq!(ext(None).state(), State::Working, "Codex reports no status");
    }

    #[test]
    fn picks_top_level_interactive_codex_processes() {
        let p = |pid, ppid, comm: &str| Proc {
            pid,
            ppid,
            uid: 501,
            start: "s".into(),
            comm: comm.into(),
        };
        let snapshot = [
            p(
                10,
                1,
                "/Users/me/.codex/packages/app-server-daemon/releases/0.159.2/bin/codex",
            ),
            p(20, 5, "/opt/homebrew/bin/codex"),
            p(
                21,
                20,
                "/opt/homebrew/lib/node_modules/@openai/codex/bin/codex",
            ),
            p(30, 5, "/usr/bin/vim"),
        ];
        let pids: Vec<i32> = codex_processes(&snapshot).iter().map(|p| p.pid).collect();
        assert_eq!(pids, [20]);
    }

    #[test]
    fn parses_lsof_cwd_output() {
        let cwds = parse_cwd("p20\nfcwd\nn/Users/me/Works/app\np31\nfcwd\nn/tmp\n");
        assert_eq!(cwds[&20], PathBuf::from("/Users/me/Works/app"));
        assert_eq!(cwds.len(), 2);
    }
}
