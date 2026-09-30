//! `proto`: M0 spike of mcc's embedded terminal pane (see `spikes/SPEC.md`).
//!
//! With no flags it runs the interactive TUI ([`ui`]); with `--headless` it
//! runs one harness case ([`headless`]).

mod headless;
mod keys;
mod session;
mod ui;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::session::Size;

/// Parses the command line and runs the chosen mode.
///
/// # Errors
///
/// Fails on bad arguments or when the chosen mode fails.
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return ui::run(&std::env::current_dir().context("current dir")?);
    }
    let mut size = Size { cols: 80, rows: 24 };
    let (mut case, mut out, mut headless) = (None, None, false);
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut value = || it.next().with_context(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--headless" => headless = true,
            "--cols" => size.cols = value()?.parse().context("--cols")?,
            "--rows" => size.rows = value()?.parse().context("--rows")?,
            "--case" => case = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            other => bail!("unknown argument {other}"),
        }
    }
    if !headless {
        bail!("usage: proto [--headless --cols C --rows R --case FILE --out DIR]");
    }
    headless::run(
        size,
        &case.context("--case is required")?,
        &out.context("--out is required")?,
    )
}
