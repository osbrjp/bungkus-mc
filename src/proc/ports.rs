//! Listening ports of tracked processes, for the quit dialog only
//! (ARCHITECTURE §3.3). Ports never decide what is stopped.

use std::collections::HashMap;
use std::process::Command;

/// Returns the TCP ports each of `pids` listens on, via one `lsof` exec
/// with a fixed argv; empty when `lsof` is missing or fails.
#[must_use]
pub(crate) fn listening(pids: &[i32]) -> HashMap<i32, Vec<u16>> {
    if pids.is_empty() {
        return HashMap::new();
    }
    let list: Vec<String> = pids.iter().map(ToString::to_string).collect();
    Command::new("lsof")
        .args([
            "-nP",
            "-iTCP",
            "-sTCP:LISTEN",
            "-a",
            "-p",
            &list.join(","),
            "-Fpn",
        ])
        .env("LC_ALL", "C")
        .output()
        .map(|out| parse(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
}

/// Parses `lsof -Fpn` output: `p<pid>` starts a process, `n<addr>:<port>`
/// is one listening socket. Unknown lines are skipped.
#[must_use]
pub(crate) fn parse(text: &str) -> HashMap<i32, Vec<u16>> {
    let mut out: HashMap<i32, Vec<u16>> = HashMap::new();
    let mut current = None;
    for line in text.lines() {
        if let Some(pid) = line.strip_prefix('p') {
            current = pid.parse().ok();
        } else if let (Some(addr), Some(pid)) = (line.strip_prefix('n'), current) {
            let port = addr.rsplit(':').next().and_then(|p| p.parse().ok());
            if let Some(port) = port {
                let ports = out.entry(pid).or_default();
                if !ports.contains(&port) {
                    ports.push(port);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lsof_field_output_and_dedupes() {
        let ports = parse(include_str!("testdata/lsof.txt"));
        assert_eq!(ports[&4470], [5173]);
        assert_eq!(ports[&4502], [5432]);
        assert!(
            parse("garbage\nn*:80\n").is_empty(),
            "a name before any pid is ignored"
        );
    }

    #[test]
    fn no_pids_means_no_exec() {
        assert!(listening(&[]).is_empty());
    }
}
