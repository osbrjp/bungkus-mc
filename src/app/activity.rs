//! The activity overlay's figures (`A`): how much memory and CPU mc, each
//! running session's process tree and mc's other children use, next to
//! the device's memory and temperature (DESIGN §5.5, ARCHITECTURE §3.7).
//!
//! Samples are taken only while the overlay is open, every
//! [`ACTIVITY_EVERY`], off the UI thread. They are for display: they never
//! change what is tracked or stopped.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::app::model::{Cmd, Model, Overlay};
use crate::app::sessions::Card;
use crate::proc::usage::Sample;

/// How often the open overlay takes a sample; the process scan's interval
/// (ARCHITECTURE §3.3).
pub(crate) const ACTIVITY_EVERY: Duration = Duration::from_secs(2);

/// One line of the overlay: a group of processes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    /// What the group is: `mc`, a session, or mc's other children.
    pub label: String,
    /// How many processes it holds.
    pub procs: usize,
    /// Their resident memory in KiB.
    pub rss_kb: u64,
    /// Their CPU use between the last two samples, in tenths of a percent
    /// of one core; `None` until there are two samples.
    pub cpu_tenths: Option<u64>,
}

/// What the overlay shows, from the latest sample.
#[derive(Debug, Default)]
pub(crate) struct Activity {
    /// `mc` first, then the running sessions, then the other children.
    pub rows: Vec<Row>,
    /// How many CPU cores the device has.
    pub cores: usize,
    /// The device's memory in KiB, when known.
    pub mem_total_kb: Option<u64>,
    /// The device's temperature in °C, when it can be read.
    pub temp_c: Option<u64>,
    /// When the previous sample was taken, and each pid's CPU time in it.
    prev: Option<(Instant, HashMap<i32, u64>)>,
    /// When the next sample is due while the overlay is open.
    next: Option<Instant>,
}

/// Returns `roots` and everything below them by `ppid`, minus `claimed`,
/// and adds the result to `claimed` so no process is counted twice.
fn subtree(
    roots: impl IntoIterator<Item = i32>,
    children: &HashMap<i32, Vec<i32>>,
    claimed: &mut HashSet<i32>,
) -> Vec<i32> {
    let mut stack: Vec<i32> = roots.into_iter().collect();
    let mut out = Vec::new();
    while let Some(pid) = stack.pop() {
        if claimed.insert(pid) {
            out.push(pid);
            stack.extend(children.get(&pid).into_iter().flatten());
        }
    }
    out
}

impl Model {
    /// Opens the activity overlay on fresh figures and asks for a first
    /// sample.
    pub(crate) fn open_activity(&mut self) -> Cmd {
        self.activity = Activity::default();
        self.overlay = Some(Overlay::Activity);
        self.activity.next = Some(self.now + ACTIVITY_EVERY);
        Cmd::Sample
    }

    /// Returns when the open overlay samples again, if it is open.
    #[must_use]
    pub(crate) fn activity_deadline(&self) -> Option<Instant> {
        matches!(self.overlay, Some(Overlay::Activity))
            .then_some(self.activity.next)
            .flatten()
    }

    /// Returns the next sample request once it is due.
    pub(crate) fn activity_due(&mut self) -> Option<Cmd> {
        let at = self.activity_deadline()?;
        (self.now >= at).then(|| {
            self.activity.next = Some(self.now + ACTIVITY_EVERY);
            Cmd::Sample
        })
    }

    /// Folds `sample` into the overlay's rows.
    ///
    /// A session's group is its agent, every descendant mc tracks for it
    /// (so ones that reparented away still count) and whatever runs below
    /// those; the last row is the rest of mc's own children (the terminal
    /// pane's shells, the editor popup, a status line command).
    ///
    /// # Arguments
    ///
    /// * `me`     - mc's own pid.
    /// * `sample` - The reading to show.
    pub(crate) fn set_activity(&mut self, me: i32, sample: &Sample) {
        let by_pid: HashMap<i32, _> = sample.procs.iter().map(|p| (p.pid, p)).collect();
        let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
        for p in &sample.procs {
            children.entry(p.ppid).or_default().push(p.pid);
        }
        let prev = self.activity.prev.take();
        let elapsed_ms = prev
            .as_ref()
            .map(|(at, _)| sample.at.saturating_duration_since(*at).as_millis())
            .and_then(|ms| u64::try_from(ms).ok())
            .filter(|ms| *ms > 0);
        let row = |label: String, pids: Vec<i32>| {
            let live = || pids.iter().filter_map(|pid| by_pid.get(pid));
            let used = |(_, cpu): &(Instant, HashMap<i32, u64>)| -> u64 {
                live()
                    .filter_map(|p| Some(p.cpu_cs.saturating_sub(*cpu.get(&p.pid)?)))
                    .sum()
            };
            Row {
                label,
                procs: live().count(),
                rss_kb: live().map(|p| p.rss_kb).sum(),
                // Centiseconds of CPU per millisecond of wall time, as ‰ of a core.
                cpu_tenths: prev
                    .as_ref()
                    .zip(elapsed_ms)
                    .map(|(prev, ms)| used(prev) * 10_000 / ms),
            }
        };
        let mut claimed = HashSet::from([me]);
        let mut rows = vec![row("mc".into(), vec![me])];
        for card in self.cards.iter().filter(|c| c.running()) {
            let Some(pid) = card.pid else { continue };
            let roots = std::iter::once(pid).chain(card.descendants.procs.iter().map(|p| p.pid));
            let pids = subtree(roots, &children, &mut claimed);
            rows.push(row(label(card), pids));
        }
        claimed.remove(&me);
        let mut rest = subtree([me], &children, &mut claimed);
        rest.retain(|pid| *pid != me);
        if !rest.is_empty() {
            rows.push(row("terminals & other".into(), rest));
        }
        self.activity.rows = rows;
        self.activity.cores = sample.cores;
        self.activity.mem_total_kb = sample.mem_total_kb;
        self.activity.temp_c = sample.temp_c;
        self.activity.prev = Some((
            sample.at,
            sample.procs.iter().map(|p| (p.pid, p.cpu_cs)).collect(),
        ));
    }
}

/// Returns mc's own pid; 0 (no process, so an empty `mc` row) if it does
/// not fit, which no supported platform allows.
#[must_use]
pub(crate) fn own_pid() -> i32 {
    i32::try_from(std::process::id()).unwrap_or(0)
}

/// Returns a session's row label: its `#a3f1` id and its name.
fn label(card: &Card) -> String {
    format!("{} {}", card.id.short(), card.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::tests::{sample, with_session};
    use crate::proc::usage::ProcUse;

    fn reading(at: Instant, procs: &[(i32, i32, u64, u64)]) -> Sample {
        Sample {
            at,
            procs: procs
                .iter()
                .map(|&(pid, ppid, rss_kb, cpu_cs)| ProcUse {
                    pid,
                    ppid,
                    rss_kb,
                    cpu_cs,
                })
                .collect(),
            cores: 8,
            mem_total_kb: Some(16_000_000),
            temp_c: None,
        }
    }

    #[test]
    fn groups_processes_by_session_and_measures_cpu_between_samples() {
        let mut m = sample(&["a"]);
        let (id, _writes) = with_session(&mut m, "fix login");
        m.cards[0].pid = Some(200);
        // 100 = mc, 200 = agent, 201 = its child, 300 = mc's shell, 301 its
        // child, 900 = a tracked descendant that reparented to init, 7 = unrelated.
        m.cards[0].descendants.procs.push(crate::proc::Proc {
            pid: 900,
            ppid: 1,
            uid: 501,
            start: "s".into(),
            comm: "vite".into(),
        });
        let table = |cpu: u64| {
            [
                (100, 50, 30_000, cpu),
                (200, 100, 400_000, cpu * 2),
                (201, 200, 100_000, 0),
                (300, 100, 5_000, 0),
                (301, 300, 1_000, 0),
                (900, 1, 50_000, cpu),
                (7, 1, 999_999, cpu * 9),
            ]
        };
        let start = m.now;
        m.set_activity(100, &reading(start, &table(10)));
        let rows = m.activity.rows.clone();
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        let session = format!("{} fix login", id.short());
        assert_eq!(labels, ["mc", session.as_str(), "terminals & other"]);
        assert_eq!(
            rows.iter().map(|r| (r.procs, r.rss_kb)).collect::<Vec<_>>(),
            [(1, 30_000), (3, 550_000), (2, 6_000)]
        );
        assert!(rows.iter().all(|r| r.cpu_tenths.is_none()), "one sample");

        // Two seconds on: mc used 0.1 s (5 %), the session 0.2 + 0.1 s (15 %).
        m.set_activity(100, &reading(start + Duration::from_secs(2), &table(20)));
        let cpu: Vec<_> = m.activity.rows.iter().map(|r| r.cpu_tenths).collect();
        assert_eq!(cpu, [Some(50), Some(150), Some(0)]);
    }

    #[test]
    fn samples_only_while_the_overlay_is_open() {
        let mut m = sample(&["a"]);
        assert_eq!(m.activity_deadline(), None);
        assert_eq!(m.open_activity(), Cmd::Sample);
        assert_eq!(m.activity_due(), None, "not due yet");
        m.now += ACTIVITY_EVERY;
        assert_eq!(m.activity_due(), Some(Cmd::Sample));
        m.overlay = None;
        m.now += ACTIVITY_EVERY;
        assert_eq!(m.activity_due(), None);
    }
}
