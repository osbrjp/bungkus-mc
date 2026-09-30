//! `bungkus-mc`: mission control for AI coding agents in the terminal.
//!
//! This file parses the command line, reads the config, and hands off to
//! the TUI in [`app`]. It is the only place besides subcommand entry points
//! that uses `anyhow`.

mod agent;
mod app;
mod ipc;
mod proc;
mod store;
mod term;
mod ui;
mod workspace;

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::agent::{Kind, find_on_path};
use crate::app::model::Model;
use crate::store::config::{self, Config, Settings, tilde};
use crate::ui::theme::{Profile, Theme, ThemeName};

/// Usage text printed by `--help`.
const HELP: &str = "\
bungkus-mc - mission control for AI coding agents

Usage: bungkus-mc [options] [WORKSPACE]
       bungkus-mc hook        (run by agent hooks; silent)
       bungkus-mc statusline  (Claude's status line inside mc)

  WORKSPACE        folder whose child folders are projects
                   (default: the workspace in config.json; first run asks)

Options:
  -h, --help       print this help
  -V, --version    print the version

Keys: ? in the app shows them all; , opens settings; q quits.
Config: ~/.config/bungkus/mc/config.json
";

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// Run the TUI, optionally on a workspace given on the command line.
    Tui(Option<String>),
    /// Print [`HELP`].
    Help,
    /// Print the version.
    Version,
    /// The silent hook subcommand agents run.
    Hook,
    /// The status-line wrapper Claude runs.
    StatusLine,
}

/// Parses command-line arguments (without the program name).
///
/// # Arguments
///
/// * `args` - The arguments after the program name.
///
/// # Errors
///
/// Returns a [`lexopt::Error`] for an unknown option or a second
/// positional argument.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Command, lexopt::Error> {
    use lexopt::prelude::*;

    let mut parser = lexopt::Parser::from_args(args);
    let mut workspace = None;
    while let Some(arg) = parser.next()? {
        match arg {
            Short('h') | Long("help") => return Ok(Command::Help),
            Short('V') | Long("version") => return Ok(Command::Version),
            Value(value) if workspace.is_none() && value == "hook" => return Ok(Command::Hook),
            Value(value) if workspace.is_none() && value == "statusline" => {
                return Ok(Command::StatusLine);
            }
            Value(value) if workspace.is_none() => workspace = Some(value.string()?),
            _ => return Err(arg.unexpected()),
        }
    }
    Ok(Command::Tui(workspace))
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
    let workspace_arg = match command {
        Command::Help => return stdout.write_all(HELP.as_bytes()).context("printing help"),
        Command::Version => {
            return writeln!(stdout, "bungkus-mc {}", env!("CARGO_PKG_VERSION"))
                .context("printing version");
        }
        Command::Hook => {
            ipc::hook::run();
            return Ok(());
        }
        Command::StatusLine => std::process::exit(ipc::statusline::run()),
        Command::Tui(workspace) => workspace,
    };
    if !stdout.is_terminal() {
        bail!("bungkus-mc needs a terminal on stdout");
    }
    let var = |name: &str| std::env::var(name).ok();
    let home = var("HOME").map(PathBuf::from);
    let cwd = std::env::current_dir().context("reading the current directory")?;
    let config_path = store::config_file(var);
    let (config, message) = match config_path.as_deref().map(config::load) {
        Some(Ok(config)) => (config, None),
        Some(Err(e)) => (Config::default(), Some(format!("{e} — using defaults."))),
        None => (Config::default(), None),
    };
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let found = Kind::ALL
        .map(|kind| find_on_path(kind.command(), &path_var).map(|p| tilde(&p, home.as_deref())));
    let git_parent = app::form::git_parent(&cwd);
    let fallback = git_parent.clone().unwrap_or_else(|| cwd.clone());
    let theme = Theme::new(ThemeName::Dark, Profile::detect(var), config.background);
    let mut model = Model::new(theme, home.clone(), found, fallback);
    model.message = message;
    model.keep.clone_from(&config.cleanup.keep);
    if let Some(text) = &config.interact_exit {
        match term::keys::Chord::parse(text) {
            Some(chord) => model.exit_chord = chord,
            None => {
                model.message = Some(format!(
                    "interactExit {text:?} is not a ctrl chord; using ctrl-\\."
                ));
            }
        }
    }

    let workspace = workspace_arg
        .map(|arg| app::absolute(&arg, &cwd, home.as_deref()))
        .or_else(|| {
            config
                .workspace
                .as_deref()
                .and_then(|w| config::expand(w, home.as_deref()))
        });
    let wizard_prefill = match workspace {
        Some(workspace) => {
            apply(&mut model, &config, workspace, &cwd);
            None
        }
        None => Some(
            git_parent
                .map(|p| tilde(&p, home.as_deref()))
                .unwrap_or_default(),
        ),
    };
    let state_path = store::state::state_file(var);
    if let Some(path) = &state_path {
        let now = std::time::Instant::now();
        let unix_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        model.cards = store::state::load(path)
            .iter()
            .filter_map(|r| app::sessions::Card::from_record(r, now, unix_now))
            .collect();
    }
    let env = app::Env {
        config_path,
        cwd,
        wizard_prefill,
        config,
        state_path,
    };
    app::run(model, &env).context("running the TUI")
}

/// Applies the config's settings on `workspace` and scans it.
fn apply(model: &mut Model, config: &Config, workspace: PathBuf, cwd: &Path) {
    let scan = workspace::scan(&workspace);
    let settings = Settings {
        workspace,
        theme: config.theme,
        default_agent: config.default_agent,
    };
    model.apply(settings, scan, cwd);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_flag_to_its_command() {
        let cases: &[(&[&str], Command)] = &[
            (&[], Command::Tui(None)),
            (&["~/Works"], Command::Tui(Some("~/Works".into()))),
            (&["-h"], Command::Help),
            (&["--help"], Command::Help),
            (&["-V"], Command::Version),
            (&["hook"], Command::Hook),
            (&["statusline"], Command::StatusLine),
            (&["--version"], Command::Version),
        ];
        for (args, want) in cases {
            let got = parse_args(args.iter().map(ToString::to_string)).unwrap();
            assert_eq!(&got, want, "args {args:?}");
        }
    }

    #[test]
    fn rejects_unknown_arguments() {
        for args in [&["--nope"][..], &["-x"], &["a", "b"]] {
            assert!(
                parse_args(args.iter().map(ToString::to_string)).is_err(),
                "{args:?}"
            );
        }
    }
}
