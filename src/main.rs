//! `bungkus-mc`: mission control for AI coding agents in the terminal.
//!
//! This file parses the command line and hands off to the TUI in [`app`].
//! It is the only place besides subcommand entry points that uses `anyhow`.

mod app;
mod ui;

use std::io::{IsTerminal, Write};

use anyhow::{Context, Result, bail};

use crate::ui::theme::{Background, Profile, Theme};

/// Usage text printed by `--help`.
const HELP: &str = "\
bungkus-mc - mission control for AI coding agents

Usage: bungkus-mc [options]

Options:
  -h, --help       print this help
  -V, --version    print the version

Keys: q or ctrl-c quits.
";

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// Run the TUI.
    Tui,
    /// Print [`HELP`].
    Help,
    /// Print the version.
    Version,
}

/// Parses command-line arguments (without the program name).
///
/// # Arguments
///
/// * `args` - The arguments after the program name.
///
/// # Errors
///
/// Returns a [`lexopt::Error`] for any option or positional argument that
/// is not listed in [`HELP`].
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Command, lexopt::Error> {
    use lexopt::prelude::*;

    let mut parser = lexopt::Parser::from_args(args);
    if let Some(arg) = parser.next()? {
        return match arg {
            Short('h') | Long("help") => Ok(Command::Help),
            Short('V') | Long("version") => Ok(Command::Version),
            _ => Err(arg.unexpected()),
        };
    }
    Ok(Command::Tui)
}

/// Parses the command line and runs the chosen command.
///
/// # Errors
///
/// Fails on an unknown argument, when stdout is not a terminal, or when
/// the terminal cannot be driven.
fn main() -> Result<()> {
    let command = parse_args(std::env::args().skip(1)).context("reading arguments")?;
    let mut stdout = std::io::stdout();
    match command {
        Command::Help => stdout.write_all(HELP.as_bytes()).context("printing help")?,
        Command::Version => writeln!(stdout, "bungkus-mc {}", env!("CARGO_PKG_VERSION"))
            .context("printing version")?,
        Command::Tui => {
            if !stdout.is_terminal() {
                bail!("bungkus-mc needs a terminal on stdout");
            }
            let profile = Profile::detect(|name| std::env::var(name).ok());
            app::run(Theme::new(profile, Background::Paint)).context("running the TUI")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_flag_to_its_command() {
        let cases: &[(&[&str], Command)] = &[
            (&[], Command::Tui),
            (&["-h"], Command::Help),
            (&["--help"], Command::Help),
            (&["-V"], Command::Version),
            (&["--version"], Command::Version),
        ];
        for (args, want) in cases {
            let got = parse_args(args.iter().map(ToString::to_string)).unwrap();
            assert_eq!(&got, want, "args {args:?}");
        }
    }

    #[test]
    fn rejects_unknown_arguments() {
        for args in [&["--nope"][..], &["-x"], &["some/dir"]] {
            assert!(
                parse_args(args.iter().map(ToString::to_string)).is_err(),
                "{args:?}"
            );
        }
    }
}
