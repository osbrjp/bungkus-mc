//! Headless mode: runs one harness case (SPEC.md §2) with no UI.
//!
//! Reads a case file, drives a [`Session`] through its steps and writes
//! screen dumps and `timing.json` into the output directory. Never touches
//! the host terminal.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use alacritty_terminal::{
    Term,
    grid::Dimensions,
    index::{Column, Line, Point},
    term::{TermMode, cell::Flags},
};
use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::{
    keys,
    session::{Session, Size},
};

/// Poll interval while waiting for the child to exit.
const POLL: Duration = Duration::from_millis(10);

/// How long to wait for the pump to reach EOF after the child exits.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// A harness case file.
#[derive(Debug, Deserialize)]
struct Case {
    cmd: Vec<String>,
    cwd: Option<PathBuf>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    steps: Vec<Step>,
}

/// One case step; the JSON key names the variant.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Step {
    Wait(u64),
    Bytes(String),
    Key(String),
    Paste(String),
    Resize([u16; 2]),
    Dump(String),
    Waitexit(u64),
}

/// Runs one case file and writes its outputs.
///
/// # Arguments
///
/// * `size` - Initial PTY and emulator size.
/// * `case` - Path to the case JSON.
/// * `out`  - Output directory; created if missing.
///
/// # Errors
///
/// Fails on an unreadable or invalid case, an unknown key name, a spawn
/// failure, a PTY write or resize failure, or an unwritable output file.
pub fn run(size: Size, case: &Path, out: &Path) -> Result<()> {
    let text = fs::read_to_string(case).with_context(|| format!("reading {}", case.display()))?;
    let case: Case = serde_json::from_str(&text).context("parsing case")?;
    fs::create_dir_all(out).context("creating output dir")?;
    let cwd = match case.cwd {
        Some(dir) => dir,
        None => std::env::current_dir().context("current dir")?,
    };
    let env: Vec<_> = case.env.into_iter().collect();
    let mut session = Session::spawn(&case.cmd, &cwd, &env, size)?;

    for step in &case.steps {
        match step {
            Step::Wait(ms) => thread::sleep(Duration::from_millis(*ms)),
            Step::Bytes(s) => session.write(s.as_bytes())?,
            Step::Key(name) => {
                let key = keys::parse(name).with_context(|| format!("unknown key {name}"))?;
                session.write(&keys::encode(&key, session.mode()))?;
            }
            Step::Paste(s) => session.write(&keys::paste(s, session.mode()))?,
            Step::Resize([cols, rows]) => session.resize(Size {
                cols: *cols,
                rows: *rows,
            })?,
            Step::Dump(label) => dump(&session.term(), out, label)?,
            Step::Waitexit(ms) => wait_exit(&mut session, Duration::from_millis(*ms)),
        }
    }
    let wall = session.started.elapsed();
    session.kill();

    let drain_deadline = Instant::now() + DRAIN_TIMEOUT;
    let drained = loop {
        let at = *session
            .shared
            .drained_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if at.is_some() || Instant::now() > drain_deadline {
            break at;
        }
        thread::sleep(POLL);
    };
    let timing = serde_json::json!({
        "wall_ms": wall.as_millis(),
        "drained_ms": drained.map(|t| t.duration_since(session.started).as_millis()),
    });
    fs::write(out.join("timing.json"), timing.to_string()).context("writing timing.json")
}

/// Waits up to `timeout` for the child to exit, then kills it.
fn wait_exit(session: &mut Session, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !session.exited() {
        if Instant::now() > deadline {
            session.kill();
            return;
        }
        thread::sleep(POLL);
    }
}

/// Writes `<label>.txt`, `<label>.cursor` and `<label>.meta.json`.
///
/// The text is exactly one line per screen row, right-trimmed, with wide
/// characters emitted once (their spacer cell is skipped).
///
/// # Errors
///
/// Fails if the label is not a plain file name or a file cannot be written.
fn dump<T>(term: &Term<T>, out: &Path, label: &str) -> Result<()> {
    if label.is_empty() || label.contains(['/', '\\']) || label.starts_with('.') {
        bail!("invalid dump label {label:?}");
    }
    let grid = term.grid();
    let mut text = String::new();
    for row in 0..grid.screen_lines() {
        let mut line = String::new();
        for col in 0..grid.columns() {
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            // reason: screen_lines is a u16 from Size, so it fits in i32.
            let cell = &grid[Point::new(Line(row as i32), Column(col))];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            line.push(cell.c);
            line.extend(cell.zerowidth().into_iter().flatten());
        }
        text.push_str(line.trim_end());
        text.push('\n');
    }
    let cursor = grid.cursor.point;
    let meta = serde_json::json!({ "alt_screen": term.mode().contains(TermMode::ALT_SCREEN) });
    fs::write(out.join(format!("{label}.txt")), text)?;
    fs::write(
        out.join(format!("{label}.cursor")),
        format!("{} {}\n", cursor.line.0, cursor.column.0),
    )?;
    fs::write(out.join(format!("{label}.meta.json")), meta.to_string())?;
    Ok(())
}
