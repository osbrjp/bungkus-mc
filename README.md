# bungkus-mc

<p align="center"><img src="docs/assets/mascot.gif" width="160" alt="the bungkus mascot: a green banana-leaf packet with tan paper corners, hopping"></p>

bungkus-mc ("mission control") is a terminal panel for running AI coding
agents — Claude Code and Codex CLI — across the projects in a workspace.
Projects on the left, sessions with their subagents and usage (tokens,
cost, context %) in the middle, the selected agent's live interactive UI
on the right, your plans' 5-hour / weekly limits in the status bar. One
small Rust binary, no daemon.

It is a sibling of [bungkus-cli](https://github.com/osbrjp/bungkus-cli)
and shares its release pipeline and the "Daun Pisang" design language.

**Status:** milestones M1–M8 are built (v0.1.0 candidate). Model routing
(M9) is not.

## Install

The repo is private, so install through the GitHub CLI with an account
that has access (`gh auth login` first):

```bash
gh release download --repo osbrjp/bungkus-mc --pattern install.sh -O - | bash
bungkus-mc            # or the short command: bkmc
bungkus-mc update     # later: install the newest release (--check only reports)
```

The installer verifies the binary against the release's `checksums.txt`,
installs to `/usr/local/bin` (`BUNGKUS_INSTALL_DIR` to change), and adds
`bkmc` only when no command of that name exists. mc checks for a newer
release once a day through `gh`; `BUNGKUS_NO_UPDATE_CHECK=1` turns that off.

Needs `claude` and/or `codex` on `PATH`. Any monospace font works.

## First run

A short wizard asks for the **workspace** (a folder whose child folders
are projects: they contain `CLAUDE.md`, `AGENTS.md` or `.git`), the
**default agent**, and the **theme** (auto, dark "Daun Teduh", light
"Santan"; the screen previews it). `esc` on the first step skips with
defaults. Change these later with `,`.

`bungkus-mc ~/Works` opens a workspace directly.

## Keys

| Keys | What |
|------|------|
| `n` | new session in the selected project (agent, model, name, optional prompt) |
| `enter` · `l` · `→` · `tab` | talk to the selected session (**INTERACT**: every key goes to the agent) |
| `ctrl-\` | leave INTERACT |
| `!` · `ctrl-]` | jump to the next session that needs you, in any project |
| `j` `k` · `↓` `↑` · `gg` `G` | move |
| `x` | stop a session (lists what it started too) |
| `r` · `d` | resume · forget a finished session |
| `z` | zoom the output pane |
| `/` | filter projects |
| `,` · `w` | settings · workspace |
| `?` | all keys |
| `q` | quit (stops sessions and what they started, after a confirm) |

INTERACT is signalled four ways at once: double border, `INTERACT` in the
title, the reverse-video mode word and the hint. The mouse wheel scrolls
mc's scrollback; to select text, hold `shift` (kitty, Ghostty, iTerm2,
WezTerm, Alacritty) or `option` (Terminal.app) while dragging.

**If `ctrl-\` is taken** (tmux with vim-tmux-navigator, VS Code / Cursor,
or a JIS or German keyboard), set another chord, for example
`"interactExit": "ctrl-^"`.

## Config

`~/.config/bungkus/mc/config.json` (all keys optional; the settings screen
writes `workspace`, `defaultAgent` and `theme` and keeps everything else):

```json
{
  "workspace": "/Users/me/Works",
  "defaultAgent": "claude",
  "theme": "auto",
  "background": "paint",
  "icons": "ascii",
  "motion": true,
  "mouse": true,
  "notify": "bell",
  "interactExit": "ctrl-\\",
  "agents": {
    "claude": { "command": "claude", "args": [] },
    "codex":  { "command": "codex",  "args": [] }
  },
  "cleanup": { "keep": ["postgres"] }
}
```

- `background: "terminal"` keeps your terminal's own background (mc paints
  its green only on TrueColor terminals anyway).
- `icons`: `ascii` (default), `unicode`, `nerd` (also `--icons`).
- `notify`: `bell` (default), `desktop` (plus OSC 9/99/777), `off`.
- `cleanup.keep`: process names the quit dialog starts as `[keep]`.

Sessions are remembered in `~/.local/state/bungkus/mc/sessions.json`.

## How it knows what the agents do

- **Structure** (working / your turn / needs you, subagents) comes from the
  agents' own hooks, injected per launch (`claude --settings`,
  `codex -c hooks.*`). Nothing is written to your agent config, so there is
  nothing to remove; your own hooks keep running.
- **Codex** asks once to trust mc's hooks ("Hooks need review" → "Trust all
  and continue"). The trust is remembered while mc stays at the same path.
- **Claude usage** comes from its status line; your own status line keeps
  rendering (mc runs it for you).
- **Codex usage** is read from `token_count` records in Codex's own session
  log, and nothing else.
- Sessions started outside mc (another terminal tab, an IDE) are listed
  read-only under their project with their state and pid, from
  `claude agents --json` and running `codex` processes.

## Quitting and cleanup

Quitting (or `x`) stops the agents and the processes they started, such as
dev servers. The dialog lists every process with its ports first and lets
you keep any of them (`space`). Desktop apps, agents, multiplexers,
`ssh-agent`, Docker and `cleanup.keep` names start as `[keep]`. mc only
ever signals processes it saw start under its own agents, identified by pid
and start time, never by port.

## Limits (v0.1)

- macOS and Linux; no Windows, no remote agents.
- Sessions do not survive mc; resume them with `r` instead.
- On a non-UTF-8 locale, borders and the mascot fall back to ASCII, but
  separators such as `·` and `→` stay as they are.
- Two mc instances on one workspace share `sessions.json` (last writer wins).

## Build

```bash
cargo build --release                 # target/release/bungkus-mc (Rust 1.97, pinned)
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings \
  && cargo test --all-features && RUSTDOCFLAGS="-D warnings" cargo doc --no-deps \
  && cargo deny check                 # what CI runs
```

Releases: merge the release PR (`main` → `release`, opened automatically)
after bumping `version` in `Cargo.toml`; the release workflow builds
darwin/linux × arm64/amd64 and publishes `v<version>`.

## Documents

| Doc | What |
|-----|------|
| [docs/PROPOSAL.md](docs/PROPOSAL.md) | Goals, milestones, decisions, risks |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Process model, hooks and status line, emulator, storage |
| [docs/DESIGN.md](docs/DESIGN.md) | Daun Pisang: palette, glyphs, mascot, mockups, keys |
| [docs/TECH_STACK.md](docs/TECH_STACK.md) | Crates and rejected alternatives, CI, release |
| [docs/CODING_RULES.md](docs/CODING_RULES.md) | Behaviour rules, tests, review checklist |
| [docs/SECURITY.md](docs/SECURITY.md) | Threat model and rules |

## Licence

Proprietary. Copyright (c) 2026 OSBR. All rights reserved. See `LICENSE`.
Third-party licences ship as `THIRD-PARTY-NOTICES` with each release.
