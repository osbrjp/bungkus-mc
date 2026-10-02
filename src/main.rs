//! `bungkus-mc`: mission control for AI coding agents in the terminal.
//!
//! This file parses the command line, reads the config, and hands off to
//! the TUI in [`app`]. It is the only place besides subcommand entry points
//! that uses `anyhow`.

mod agent;
mod app;
mod external;
mod ipc;
mod proc;
mod store;
mod term;
mod ui;
mod update;
mod workspace;

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::agent::{Kind, find_on_path};
use crate::app::model::Model;
use crate::store::config::{self, Config, Settings, tilde};
use crate::ui::icons::{self, IconChoice};
use crate::ui::theme::{Profile, Theme, ThemeName};

/// Usage text printed by `--help`.
const HELP: &str = "\
bungkus-mc - mission control for AI coding agents

Usage: bungkus-mc [options] [WORKSPACE]
       bungkus-mc hook        (run by agent hooks; silent)
       bungkus-mc statusline  (Claude's status line inside mc)
       bungkus-mc update [--check]
       bungkus-mc uninstall [--purge] [--yes]

  WORKSPACE        folder whose child folders are projects
                   (default: the workspace in config.json; first run asks)

Options:
  -q, --quick      open a quick session popup at once
  -p, --project NAME
                   start on this project; when the workspace has no such
                   project, the workspace switcher opens to find it
  --icons SET      state glyphs: auto (default: nerd when a Nerd Font is
                   installed, else ascii), ascii, unicode, nerd
  --debug          write a debug log (~/.local/state/bungkus/mc/mc.log)
  -h, --help       print this help
  -V, --version    print the version

Keys: ? in the app shows them all; , opens settings; q quits.
Config: ~/.config/bungkus/mc/config.json
";

/// What the command line gives the TUI.
#[derive(Debug, Default, PartialEq, Eq)]
struct TuiArgs {
    /// Workspace folder as typed; the saved one when absent.
    workspace: Option<String>,
    /// `--icons`.
    icons: Option<IconChoice>,
    /// `--debug`: write the debug log.
    debug: bool,
    /// `-q`: open a quick session popup at start.
    quick: bool,
    /// `-p`: name of the project to start on.
    project: Option<String>,
}

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// Run the TUI.
    Tui(TuiArgs),
    /// Print [`HELP`].
    Help,
    /// Print the version.
    Version,
    /// The silent hook subcommand agents run.
    Hook,
    /// The status-line wrapper Claude runs.
    StatusLine,
    /// Update to the latest release (or only check, with `--check`).
    Uninstall {
        /// Also remove config, state and the update cache.
        purge: bool,
        /// Do not ask.
        yes: bool,
    },
    Update {
        /// Only report whether a newer release exists.
        check: bool,
    },
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
    let mut tui = TuiArgs::default();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('h') | Long("help") => return Ok(Command::Help),
            Short('V') | Long("version") => return Ok(Command::Version),
            Value(value) if tui.workspace.is_none() && value == "hook" => return Ok(Command::Hook),
            Value(value) if tui.workspace.is_none() && value == "statusline" => {
                return Ok(Command::StatusLine);
            }
            Value(value) if tui.workspace.is_none() && value == "uninstall" => {
                let (mut purge, mut yes) = (false, false);
                while let Some(arg) = parser.next()? {
                    match arg {
                        Long("purge") => purge = true,
                        Long("yes") | Short('y') => yes = true,
                        other => return Err(other.unexpected()),
                    }
                }
                return Ok(Command::Uninstall { purge, yes });
            }
            Value(value) if tui.workspace.is_none() && value == "update" => {
                let check = match parser.next()? {
                    None => false,
                    Some(Long("check")) => true,
                    Some(other) => return Err(other.unexpected()),
                };
                return Ok(Command::Update { check });
            }
            Short('q') | Long("quick") => tui.quick = true,
            Short('p') | Long("project") => tui.project = Some(parser.value()?.string()?),
            Long("debug") => tui.debug = true,
            Long("icons") => {
                let value = parser.value()?.string()?;
                let Some(choice) = IconChoice::parse(&value) else {
                    return Err(lexopt::Error::from(format!(
                        "--icons: {value}? (auto, ascii, unicode, nerd)"
                    )));
                };
                tui.icons = Some(choice);
            }
            Value(value) if tui.workspace.is_none() => tui.workspace = Some(value.string()?),
            _ => return Err(arg.unexpected()),
        }
    }
    Ok(Command::Tui(tui))
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
    let Some(Command::Tui(args)) = run_subcommand(command, &mut stdout)? else {
        return Ok(());
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
    let fallback = default_workspace(home.as_deref(), &cwd);
    let utf8 = utf8_locale(var);
    // Over SSH the fonts on this host are not the ones the terminal draws with.
    let icons = args.icons.unwrap_or(config.icons).resolve(|| {
        utf8 && var("SSH_CONNECTION").is_none()
            && icons::nerd_font_installed(&icons::font_dirs(home.as_deref()))
    });
    let theme = Theme::new(ThemeName::Dark, Profile::detect(var), config.background).with_view(
        icons,
        utf8,
        config.motion,
    );
    let mut model = Model::new(theme, home.clone(), found, fallback);
    model.message = message;
    model.keep.clone_from(&config.cleanup.keep);
    model.widths = config.panes;
    model.workspaces = config
        .workspaces
        .iter()
        .filter_map(|w| config::expand(w, home.as_deref()))
        .collect();
    model.debug_keys = var("BUNGKUS_MC_DEBUG_KEYS").is_some();
    model.kitty = var("KITTY_WINDOW_ID").is_some();
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

    model.want_project.clone_from(&args.project);
    let workspace = args
        .workspace
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
            if let Some(name) = args.project
                && model.selected_project().is_none_or(|p| p.name != name)
            {
                model.message = Some(format!("No project {name} here; pick its workspace."));
                model.want_project = Some(name);
                model.open_switcher();
            }
            None
        }
        None => Some(tilde(&model.fallback_workspace, home.as_deref())),
    };
    let state_path = store::state::state_file(var);
    if let (true, Some(path)) = (args.debug, &state_path) {
        store::debug::open(&path.with_file_name("mc.log")).context("opening the debug log")?;
        debug_log!("bungkus-mc {} started", env!("CARGO_PKG_VERSION"));
    }
    if let Some(path) = &state_path {
        restore_state(&mut model, path);
    }
    let env = app::Env {
        config_path,
        cwd,
        wizard_prefill,
        quick: args.quick,
        config,
        state_path,
    };
    let exe = std::env::current_exe().ok();
    if app::run(model, &env).context("running the TUI")? {
        restart(exe)?;
    }
    Ok(())
}

/// Runs every command but the TUI and returns `None`; the TUI command is
/// handed back for `main` to run.
///
/// # Errors
///
/// What the subcommand reports, or a failed write to stdout.
fn run_subcommand(command: Command, stdout: &mut std::io::Stdout) -> Result<Option<Command>> {
    match command {
        Command::Help => stdout.write_all(HELP.as_bytes()).context("printing help")?,
        Command::Version => {
            writeln!(stdout, "bungkus-mc {}", env!("CARGO_PKG_VERSION"))
                .context("printing version")?;
        }
        Command::Hook => ipc::hook::run(),
        Command::StatusLine => std::process::exit(ipc::statusline::run()),
        Command::Uninstall { purge, yes } => {
            let text = update::uninstall(purge, yes).context("uninstall")?;
            writeln!(stdout, "{text}").context("printing")?;
        }
        Command::Update { check } => {
            let text = update::run(check).context("update")?;
            writeln!(stdout, "{text}").context("printing")?;
        }
        tui @ Command::Tui(_) => return Ok(Some(tui)),
    }
    Ok(None)
}

/// Starts the updated binary in place of this process (`U`), with the same
/// arguments, once the terminal is restored. `exe` is the path taken at
/// start, as Linux reports a replaced binary as deleted afterwards.
///
/// # Errors
///
/// Only when the new binary cannot be executed.
fn restart(exe: Option<PathBuf>) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let exe = exe.context("finding mc's own path to restart")?;
    let error = std::process::Command::new(&exe)
        .args(std::env::args_os().skip(1))
        .exec();
    Err(error).context("restarting the updated bungkus-mc")
}

/// Restores the remembered sessions and plan limits from the state folder
/// (`sessions.json`, `limits.json`).
fn restore_state(model: &mut app::model::Model, path: &std::path::Path) {
    let now = std::time::Instant::now();
    let unix_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let stored = store::state::load_limits(&store::state::limits_file(path));
    for (v, (windows, at)) in stored.vendors.into_iter().enumerate() {
        if !windows.is_empty() {
            model.limits[v] = windows;
            model.limits_at[v] =
                now.checked_sub(std::time::Duration::from_secs(unix_now.saturating_sub(at)));
        }
    }
    model.cards = store::state::load(path)
        .iter()
        .filter_map(|r| app::sessions::Card::from_record(r, now, unix_now))
        .collect();
    model.known = model.cards.iter().map(|c| c.id).collect();
}

/// Returns whether the locale is UTF-8 (DESIGN §3): the first set of
/// `LC_ALL`, `LC_CTYPE`, `LANG` names UTF-8, and `TERM` is not `linux` or
/// `dumb`.
fn utf8_locale(var: impl Fn(&str) -> Option<String>) -> bool {
    let term = var("TERM").unwrap_or_default();
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|n| var(n).filter(|v| !v.is_empty()));
    let utf8 = locale.is_some_and(|l| {
        let l = l.to_ascii_lowercase();
        l.contains("utf-8") || l.contains("utf8")
    });
    utf8 && term != "linux" && term != "dumb"
}

/// Returns the wizard's default workspace: `~/Documents` when it exists,
/// else the parent of the git repository mc was started in, else the
/// current folder.
fn default_workspace(home: Option<&Path>, cwd: &Path) -> PathBuf {
    home.map(|h| h.join("Documents"))
        .filter(|d| d.is_dir())
        .or_else(|| app::form::git_parent(cwd))
        .unwrap_or_else(|| cwd.to_path_buf())
}

/// Applies the config's settings on `workspace` and scans it.
fn apply(model: &mut Model, config: &Config, workspace: PathBuf, cwd: &Path) {
    let scan = workspace::scan(&workspace);
    let settings = Settings {
        workspace,
        theme: config.theme,
        default_agent: config.default_agent,
    };
    model.remember_workspace(&settings.workspace);
    model.apply(settings, scan, cwd);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_flag_to_its_command() {
        let tui = Command::Tui;
        let cases: &[(&[&str], Command)] = &[
            (&[], tui(TuiArgs::default())),
            (
                &["~/Works"],
                tui(TuiArgs {
                    workspace: Some("~/Works".into()),
                    ..TuiArgs::default()
                }),
            ),
            (
                &["--debug"],
                tui(TuiArgs {
                    debug: true,
                    ..TuiArgs::default()
                }),
            ),
            (
                &["-q"],
                tui(TuiArgs {
                    quick: true,
                    ..TuiArgs::default()
                }),
            ),
            (
                &["-q", "~/Works", "--debug"],
                tui(TuiArgs {
                    workspace: Some("~/Works".into()),
                    debug: true,
                    quick: true,
                    ..TuiArgs::default()
                }),
            ),
            (
                &["-p", "kedai-web", "~/Works"],
                tui(TuiArgs {
                    workspace: Some("~/Works".into()),
                    project: Some("kedai-web".into()),
                    ..TuiArgs::default()
                }),
            ),
            (
                &["--project", "kedai-web", "--quick"],
                tui(TuiArgs {
                    quick: true,
                    project: Some("kedai-web".into()),
                    ..TuiArgs::default()
                }),
            ),
            (&["-h"], Command::Help),
            (&["--help"], Command::Help),
            (&["-V"], Command::Version),
            (&["hook"], Command::Hook),
            (
                &["--icons", "unicode"],
                tui(TuiArgs {
                    icons: Some(IconChoice::Unicode),
                    ..TuiArgs::default()
                }),
            ),
            (&["statusline"], Command::StatusLine),
            (&["update"], Command::Update { check: false }),
            (&["update", "--check"], Command::Update { check: true }),
            (
                &["uninstall"],
                Command::Uninstall {
                    purge: false,
                    yes: false,
                },
            ),
            (
                &["uninstall", "--purge", "-y"],
                Command::Uninstall {
                    purge: true,
                    yes: true,
                },
            ),
            (&["--version"], Command::Version),
        ];
        for (args, want) in cases {
            let got = parse_args(args.iter().map(ToString::to_string)).unwrap();
            assert_eq!(&got, want, "args {args:?}");
        }
    }

    #[test]
    fn rejects_unknown_arguments() {
        for args in [
            &["--nope"][..],
            &["-x"],
            &["a", "b"],
            &["--icons", "emoji"],
            &["-p"],
        ] {
            assert!(
                parse_args(args.iter().map(ToString::to_string)).is_err(),
                "{args:?}"
            );
        }
    }

    #[test]
    fn detects_a_utf8_locale() {
        let env = |pairs: &'static [(&str, &str)]| {
            move |n: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == n)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert!(utf8_locale(env(&[("LANG", "en_US.UTF-8")])));
        assert!(utf8_locale(env(&[("LC_ALL", "ja_JP.utf8"), ("LANG", "C")])));
        assert!(
            !utf8_locale(env(&[("LC_ALL", "C"), ("LANG", "en_US.UTF-8")])),
            "LC_ALL wins"
        );
        assert!(!utf8_locale(env(&[
            ("LANG", "en_US.UTF-8"),
            ("TERM", "linux")
        ])));
        assert!(!utf8_locale(env(&[])));
    }
}
