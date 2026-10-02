//! Memory and CPU time of every process, plus the device's memory size
//! and temperature, for the activity overlay (ARCHITECTURE §3.7).
//!
//! Read-only and display-only: a sample never decides what is tracked or
//! stopped, and argv is never read. Nothing here runs unless the overlay
//! is open.

use std::process::{Command, Stdio};
use std::time::Instant;

/// One process's resource use in a [`Sample`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProcUse {
    /// Process id.
    pub pid: i32,
    /// Parent process id.
    pub ppid: i32,
    /// Resident memory in KiB.
    pub rss_kb: u64,
    /// CPU time used since the process started, in centiseconds.
    pub cpu_cs: u64,
}

/// One reading of the process table and the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sample {
    /// When the reading was taken.
    pub at: Instant,
    /// Every process that could be read.
    pub procs: Vec<ProcUse>,
    /// How many CPU cores the device offers mc.
    pub cores: usize,
    /// The device's memory in KiB; `None` when it could not be read.
    pub mem_total_kb: Option<u64>,
    /// The hottest thermal zone in °C. Linux only: macOS gives the
    /// temperature to root (`powermetrics`) or through `IOKit` FFI, and mc
    /// uses neither (no `unsafe`, SECURITY.md).
    pub temp_c: Option<u64>,
}

/// Parses `ps`'s `cputime` column (`M:SS.cc`, minutes unbounded) into
/// centiseconds.
#[must_use]
fn parse_cputime(text: &str) -> Option<u64> {
    let (whole, frac) = text.split_once('.')?;
    let mut secs = 0u64;
    for part in whole.split(':') {
        secs = secs.checked_mul(60)?.checked_add(part.parse().ok()?)?;
    }
    let cs: u64 = (frac.len() == 2).then(|| frac.parse().ok())??;
    secs.checked_mul(100)?.checked_add(cs)
}

/// Parses `ps -axo pid=,ppid=,rss=,cputime=` output (run with `LC_ALL=C`);
/// lines that do not fit are skipped.
#[must_use]
pub(crate) fn parse_ps(text: &str) -> Vec<ProcUse> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            Some(ProcUse {
                pid: words.next()?.parse().ok()?,
                ppid: words.next()?.parse().ok()?,
                rss_kb: words.next()?.parse().ok()?,
                cpu_cs: parse_cputime(words.next()?)?,
            })
        })
        .collect()
}

/// Parses one Linux `/proc/<pid>/stat` line.
///
/// # Arguments
///
/// * `line`    - The stat line; `comm` may hold spaces and parentheses.
/// * `page_kb` - The page size in KiB (`rss` is counted in pages).
/// * `ticks`   - Clock ticks per second (`utime`/`stime` are in ticks).
#[must_use]
fn parse_stat(line: &str, page_kb: u64, ticks: u64) -> Option<ProcUse> {
    let pid = line[..line.find('(')?].trim().parse().ok()?;
    let fields: Vec<&str> = line
        .get(line.rfind(')')? + 1..)?
        .split_whitespace()
        .collect();
    // Fields after `comm`: ppid is field 4, utime 14, stime 15, rss 24 (1-based).
    let field = |n: usize| fields.get(n - 3)?.parse::<u64>().ok();
    Some(ProcUse {
        pid,
        ppid: fields.get(1)?.parse().ok()?,
        rss_kb: field(24)?.checked_mul(page_kb)?,
        cpu_cs: (field(14)? + field(15)?).checked_mul(100)? / ticks.max(1),
    })
}

/// Parses the `MemTotal` line of Linux's `/proc/meminfo` into KiB.
#[must_use]
fn parse_meminfo(text: &str) -> Option<u64> {
    let line = text.lines().find_map(|l| l.strip_prefix("MemTotal:"))?;
    line.split_whitespace().next()?.parse().ok()
}

/// Returns the hottest `/sys/class/thermal/thermal_zone*/temp` in °C.
fn linux_temp() -> Option<u64> {
    std::fs::read_dir("/sys/class/thermal")
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("thermal_zone"))
        .filter_map(|e| std::fs::read_to_string(e.path().join("temp")).ok())
        .filter_map(|milli| milli.trim().parse::<u64>().ok())
        .max()
        .map(|milli| milli / 1000)
}

/// Runs `cmd` with fixed `args` under `LC_ALL=C` and returns its pid and
/// its stdout.
fn output(cmd: &str, args: &[&str]) -> Option<(u32, String)> {
    let child = Command::new(cmd)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let pid = child.id();
    let out = child.wait_with_output().ok()?;
    Some((pid, String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// Takes one [`Sample`]. Blocking (one `/proc` walk, or two execs on
/// macOS), so it runs off the UI thread; whatever cannot be read is left
/// out.
#[must_use]
pub(crate) fn sample() -> Sample {
    let at = Instant::now();
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    if cfg!(target_os = "linux") {
        let page_kb = u64::try_from(rustix::param::page_size() / 1024).unwrap_or(4);
        let ticks = rustix::param::clock_ticks_per_second();
        let procs = std::fs::read_dir("/proc")
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            // `self` and `thread-self` would list mc and this thread again.
            .filter(|e| e.file_name().to_string_lossy().parse::<u32>().is_ok())
            .filter_map(|e| std::fs::read_to_string(e.path().join("stat")).ok())
            .filter_map(|stat| parse_stat(&stat, page_kb, ticks))
            .collect();
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok();
        Sample {
            at,
            cores,
            procs,
            mem_total_kb: meminfo.as_deref().and_then(parse_meminfo),
            temp_c: linux_temp(),
        }
    } else {
        // `ps` lists itself as a child of mc; it is gone once it has answered.
        let procs = output("ps", &["-axo", "pid=,ppid=,rss=,cputime="]).map(|(ps, text)| {
            let mut procs = parse_ps(&text);
            procs.retain(|p| u32::try_from(p.pid) != Ok(ps));
            procs
        });
        let bytes = output("sysctl", &["-n", "hw.memsize"]);
        Sample {
            at,
            cores,
            procs: procs.unwrap_or_default(),
            mem_total_kb: bytes
                .and_then(|(_, b)| b.trim().parse::<u64>().ok())
                .map(|b| b / 1024),
            temp_c: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ps_usage_and_skips_what_does_not_fit() {
        let text = "    1     0  20688 173:19.61\n39541 39000   4016   0:00.05\nps: oops\n 7 1 x 0:00.00\n";
        assert_eq!(
            parse_ps(text),
            [
                ProcUse {
                    pid: 1,
                    ppid: 0,
                    rss_kb: 20688,
                    cpu_cs: 173 * 6000 + 1961
                },
                ProcUse {
                    pid: 39541,
                    ppid: 39000,
                    rss_kb: 4016,
                    cpu_cs: 5
                },
            ]
        );
    }

    #[test]
    fn parses_cputime_shapes() {
        let cases = [
            ("0:00.05", Some(5)),
            ("1:02:03.04", Some(372_304)),
            ("12", None),
            ("1:xx.00", None),
            ("0:00.5", None),
        ];
        for (text, want) in cases {
            assert_eq!(parse_cputime(text), want, "{text}");
        }
    }

    #[test]
    fn parses_linux_stat_usage() {
        let line = "4471 (vite (dev) x) S 4470 4471 4400 0 -1 4194304 1 0 0 0 150 50 0 0 20 0 1 0 987654 1000000 2500 0";
        assert_eq!(
            parse_stat(line, 4, 100),
            Some(ProcUse {
                pid: 4471,
                ppid: 4470,
                rss_kb: 10_000,
                cpu_cs: 200
            })
        );
        assert_eq!(parse_stat("broken", 4, 100), None);
    }

    #[test]
    fn a_live_sample_holds_this_process() {
        let me = i32::try_from(std::process::id()).unwrap();
        let sample = sample();
        let own = sample.procs.iter().find(|p| p.pid == me).unwrap();
        assert!(own.rss_kb > 0);
        assert!(sample.mem_total_kb.is_some_and(|kb| kb > own.rss_kb));
    }

    #[test]
    fn parses_meminfo_total() {
        let text = "MemTotal:       16384000 kB\nMemFree:  1 kB\n";
        assert_eq!(parse_meminfo(text), Some(16_384_000));
        assert_eq!(parse_meminfo("nothing"), None);
    }
}
