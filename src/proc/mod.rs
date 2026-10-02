//! Processes the agents started (dev servers and friends), and stopping
//! them on quit or `x` (ARCHITECTURE §3.2–3.3, SECURITY.md "Process-tree
//! scan and cleanup").
//!
//! mc only ever tracks processes it *observed* as descendants of an agent
//! it launched (by `ppid`, in periodic snapshots), only of its own uid,
//! identified by pid **and** start time. Nothing is inferred from names or
//! ports, and argv of other processes is never read.

pub(crate) mod kill;
pub(crate) mod ports;
pub(crate) mod usage;

use std::collections::{HashMap, HashSet};
use std::process::Command;

use crate::ui::sanitise::sanitise;

/// Longest process name shown.
const NAME_MAX: usize = 40;

/// Basenames that start as `[keep]` (ARCHITECTURE §3.2).
const DEFAULT_KEEP: [&str; 11] = [
    "gpg-agent",
    "ssh-agent",
    "tmux",
    "screen",
    "watchman",
    "ollama",
    "colima",
    "docker",
    "code",
    "claude",
    "codex",
];

/// One process as seen in a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Proc {
    /// Process id.
    pub pid: i32,
    /// Parent process id.
    pub ppid: i32,
    /// Owner.
    pub uid: u32,
    /// Start time as the OS reports it (`lstart` text on macOS, clock
    /// ticks since boot on Linux); with `pid` it is the identity.
    pub start: String,
    /// Command path or name (`comm`); untrusted.
    pub comm: String,
}

impl Proc {
    /// Returns the sanitised basename of `comm`, for display.
    #[must_use]
    pub(crate) fn name(&self) -> String {
        let base = self.comm.rsplit('/').next().unwrap_or(&self.comm);
        sanitise(base, NAME_MAX)
    }

    /// Returns whether the process starts as `[keep]`: a desktop app
    /// (a bundle under an `Applications` folder; interpreters such as
    /// Homebrew's `Python.app` are not), a known long-lived tool, or a name
    /// in `cleanup.keep`.
    #[must_use]
    pub(crate) fn keep_by_default(&self, extra: &[String]) -> bool {
        let base = self.comm.rsplit('/').next().unwrap_or(&self.comm);
        let desktop_app =
            self.comm.contains(".app/Contents/") && self.comm.contains("Applications/");
        desktop_app || DEFAULT_KEEP.contains(&base) || extra.iter().any(|k| k == base)
    }
}

/// Parses `ps -axo pid=,ppid=,uid=,lstart=,comm=` output (run with
/// `LC_ALL=C`): three integers, exactly five `lstart` tokens, then `comm`
/// as the rest of the line (it may contain spaces). Lines that do not fit
/// are skipped, so a localised `lstart` yields nothing rather than wrong
/// entries.
#[must_use]
#[expect(
    clippy::similar_names,
    reason = "pid, ppid and uid are the ps column names"
)]
pub(crate) fn parse_ps(text: &str) -> Vec<Proc> {
    text.lines()
        .filter_map(|line| {
            let mut rest = line.trim_start();
            let mut next = || {
                let (word, tail) = rest.split_once(char::is_whitespace)?;
                rest = tail.trim_start();
                Some(word)
            };
            let pid = next()?.parse().ok()?;
            let ppid = next()?.parse().ok()?;
            let uid = next()?.parse().ok()?;
            let words: Vec<&str> = (0..5).map(|_| next()).collect::<Option<_>>()?;
            let (weekday, month, day, time, year) =
                (words[0], words[1], words[2], words[3], words[4]);
            let valid = weekday.len() == 3
                && weekday.chars().all(|c| c.is_ascii_alphabetic())
                && month.chars().all(|c| c.is_ascii_alphabetic())
                && day.parse::<u8>().is_ok()
                && time.split(':').count() == 3
                && year.parse::<u16>().is_ok();
            let comm = rest.trim_end();
            (valid && !comm.is_empty()).then(|| Proc {
                pid,
                ppid,
                uid,
                start: words.join(" "),
                comm: comm.to_owned(),
            })
        })
        .collect()
}

/// Parses one Linux `/proc/<pid>/stat` line into `(pid, comm, ppid,
/// starttime)`; `comm` is everything between the first `(` and the last
/// `)`, so names with spaces or parentheses parse.
#[must_use]
#[expect(
    clippy::similar_names,
    reason = "pid and ppid are the stat field names"
)]
pub(crate) fn parse_stat(line: &str) -> Option<(i32, String, i32, String)> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    let pid = line[..open].trim().parse().ok()?;
    let comm = line.get(open + 1..close)?.to_owned();
    let fields: Vec<&str> = line.get(close + 1..)?.split_whitespace().collect();
    // Fields after `comm`: state is field 3, ppid 4, starttime 22 (1-based).
    let ppid = fields.get(1)?.parse().ok()?;
    let start = (*fields.get(19)?).to_owned();
    Some((pid, comm, ppid, start))
}

/// Takes one process-table snapshot of this uid's processes.
///
/// macOS: one `ps` exec with a fixed argv and `LC_ALL=C`; Linux: a walk of
/// `/proc`. Failures yield an empty snapshot, which never widens what gets
/// stopped (entries are only ever pruned by it).
#[must_use]
pub(crate) fn snapshot(uid: u32) -> Vec<Proc> {
    let all = if cfg!(target_os = "linux") {
        linux_snapshot()
    } else {
        ps_snapshot()
    };
    all.into_iter().filter(|p| p.uid == uid).collect()
}

/// Runs `ps` for [`snapshot`].
fn ps_snapshot() -> Vec<Proc> {
    Command::new("ps")
        .args(["-axo", "pid=,ppid=,uid=,lstart=,comm="])
        .env("LC_ALL", "C")
        .output()
        .map(|out| parse_ps(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
}

/// Walks `/proc` for [`snapshot`].
#[expect(
    clippy::similar_names,
    reason = "pid, ppid and uid are the ps column names"
)]
fn linux_snapshot() -> Vec<Proc> {
    use std::os::unix::fs::MetadataExt;
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let uid = std::fs::metadata(&path).ok()?.uid();
            let stat = std::fs::read_to_string(path.join("stat")).ok()?;
            let (pid, comm, ppid, start) = parse_stat(&stat)?;
            Some(Proc {
                pid,
                ppid,
                uid,
                start,
                comm,
            })
        })
        .collect()
}

/// The processes observed under one agent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Descendants {
    /// Every tracked process, by identity.
    pub procs: Vec<Proc>,
}

impl Descendants {
    /// Folds in a snapshot: adds every process reachable from `root` by
    /// `ppid` (the agent itself excluded) and prunes tracked entries whose
    /// pid and start time are no longer in the snapshot. A process seen
    /// once stays tracked after it reparents to init.
    pub(crate) fn update(&mut self, root: i32, snapshot: &[Proc]) {
        let alive: HashSet<(i32, &str)> =
            snapshot.iter().map(|p| (p.pid, p.start.as_str())).collect();
        self.procs
            .retain(|p| alive.contains(&(p.pid, p.start.as_str())));
        let mut children: HashMap<i32, Vec<&Proc>> = HashMap::new();
        for p in snapshot {
            children.entry(p.ppid).or_default().push(p);
        }
        let mut stack = vec![root];
        while let Some(pid) = stack.pop() {
            for child in children.get(&pid).into_iter().flatten() {
                stack.push(child.pid);
                if !self
                    .procs
                    .iter()
                    .any(|p| p.pid == child.pid && p.start == child.start)
                {
                    self.procs.push((*child).clone());
                }
            }
        }
        self.procs.sort_by_key(|p| p.pid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = include_str!("testdata/ps-macos.txt");

    #[test]
    fn parses_ps_with_spaces_in_comm() {
        let procs = parse_ps(PS);
        assert_eq!(procs.len(), 11);
        let esbuild = procs.iter().find(|p| p.pid == 4480).unwrap();
        assert_eq!(esbuild.comm, "/opt/My Tools/esbuild helper");
        assert_eq!(esbuild.start, "Wed Sep 30 13:43:00 2026");
        assert_eq!(esbuild.name(), "esbuild helper");
    }

    #[test]
    fn a_localised_ps_parses_to_nothing() {
        assert!(parse_ps(include_str!("testdata/ps-macos-ja.txt")).is_empty());
        assert!(parse_ps("garbage\n  12 x 3 Wed Sep 30 1:2:3 2026 a\n").is_empty());
    }

    #[test]
    fn parses_linux_stat_lines() {
        let line =
            "4471 (vite (dev) x) S 4470 4471 4400 0 -1 4194304 1 0 0 0 0 0 0 0 20 0 1 0 987654 0 0";
        assert_eq!(
            parse_stat(line),
            Some((4471, "vite (dev) x".into(), 4470, "987654".into()))
        );
        assert_eq!(parse_stat("broken"), None);
    }

    #[test]
    fn tracks_descendants_across_snapshots() {
        let first = parse_ps(PS);
        let mut d = Descendants::default();
        d.update(4400, &first);
        let pids: Vec<i32> = d.procs.iter().map(|p| p.pid).collect();
        assert_eq!(pids, [4460, 4470, 4471, 4480, 4490, 4500, 4502]);
        let mut second: Vec<Proc> = first
            .iter()
            .filter(|p| p.pid != 4400 && p.pid != 4460)
            .cloned()
            .collect();
        for p in &mut second {
            if p.pid == 4470 {
                p.ppid = 1;
            }
        }
        d.update(4400, &second);
        assert!(
            d.procs.iter().any(|p| p.pid == 4471),
            "reparented child stays tracked"
        );
        assert!(
            !d.procs.iter().any(|p| p.pid == 4460),
            "gone process is pruned"
        );
        let mut third = second.clone();
        for p in third.iter_mut().filter(|p| p.pid == 4502) {
            p.start = "Thu Oct  1 09:00:00 2026".into();
            p.ppid = 1;
        }
        d.update(4400, &third);
        assert!(
            !d.procs.iter().any(|p| p.pid == 4502),
            "a reused pid is a different process"
        );
    }

    #[test]
    fn default_keep_rule() {
        let procs = parse_ps(PS);
        let keep = |pid| {
            procs
                .iter()
                .find(|p| p.pid == pid)
                .unwrap()
                .keep_by_default(&["postgres".into()])
        };
        assert!(keep(611), "ssh-agent");
        assert!(keep(4500), "an app bundle");
        assert!(keep(4502), "cleanup.keep");
        assert!(!keep(4471), "vite");
        let python = Proc {
            pid: 9,
            ppid: 1,
            uid: 501,
            start: "s".into(),
            comm: "/opt/homebrew/Cellar/python@3.14/3.14.4/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python".into(),
        };
        assert!(
            !python.keep_by_default(&[]),
            "an interpreter bundle is not a desktop app"
        );
    }
}
