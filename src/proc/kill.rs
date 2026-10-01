//! Signalling tracked descendants, only after re-checking their identity
//! (SECURITY.md "Process-tree scan and cleanup").
//!
//! Before every signal the process's start time is read again and must
//! equal the tracked one, so a reused pid is never signalled. The
//! re-check reads the same source as the snapshot (`ps` on macOS, `/proc`
//! on Linux).

use std::process::Command;

use rustix::process::{Pid, Signal, kill_process};

use crate::proc::{Proc, parse_ps, parse_stat};

/// Why a process was not signalled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum KillError {
    /// It is gone, or its pid now belongs to another process.
    #[error("no longer running")]
    Gone,
    /// The OS refused (`EPERM`).
    #[error("could not stop")]
    Denied,
}

/// Returns the current start time of `pid`, or `None` when it is gone.
fn start_time(pid: i32) -> Option<String> {
    if cfg!(target_os = "linux") {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        parse_stat(&stat).map(|(_, _, _, start)| start)
    } else {
        let out = Command::new("ps")
            .args([
                "-o",
                "pid=,ppid=,uid=,lstart=,comm=",
                "-p",
                &pid.to_string(),
            ])
            .env("LC_ALL", "C")
            .output()
            .ok()?;
        parse_ps(&String::from_utf8_lossy(&out.stdout))
            .into_iter()
            .next()
            .map(|p| p.start)
    }
}

/// Sends `signal` to `proc` when it is still the same process.
///
/// # Errors
///
/// * [`KillError::Gone`] - it exited or its pid was reused.
/// * [`KillError::Denied`] - the OS refused.
pub(crate) fn signal(proc: &Proc, signal: Signal) -> Result<(), KillError> {
    signal_checked(proc, signal, start_time)
}

/// [`signal`] with the start-time lookup injected, for tests.
fn signal_checked(
    proc: &Proc,
    signal: Signal,
    lookup: impl Fn(i32) -> Option<String>,
) -> Result<(), KillError> {
    if lookup(proc.pid).as_deref() != Some(proc.start.as_str()) {
        return Err(KillError::Gone);
    }
    let pid = Pid::from_raw(proc.pid).ok_or(KillError::Gone)?;
    kill_process(pid, signal).map_err(|e| {
        if e == rustix::io::Errno::PERM {
            KillError::Denied
        } else {
            KillError::Gone
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_a_changed_start_time() {
        let proc = Proc {
            pid: 1,
            ppid: 0,
            uid: 0,
            start: "old".into(),
            comm: "x".into(),
        };
        let got = signal_checked(&proc, Signal::TERM, |_| Some("new".into()));
        assert_eq!(got, Err(KillError::Gone), "never signals a reused pid");
        assert_eq!(
            signal_checked(&proc, Signal::TERM, |_| None),
            Err(KillError::Gone)
        );
    }

    #[test]
    fn signals_a_live_child_it_started() {
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = i32::try_from(child.id()).unwrap();
        let start = start_time(pid).expect("the child is visible");
        let proc = Proc {
            pid,
            ppid: 0,
            uid: 0,
            start,
            comm: "sleep".into(),
        };
        assert_eq!(signal(&proc, Signal::TERM), Ok(()));
        let status = child.wait().unwrap();
        assert!(!status.success());
        assert_eq!(signal(&proc, Signal::TERM), Err(KillError::Gone));
    }
}
