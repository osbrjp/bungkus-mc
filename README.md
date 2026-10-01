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

**Status:** beta (`0.1.0-beta.1`): milestones M1–M8 are built; model
routing (M9) is not.

## Install

The repo is private, so install through the GitHub CLI with an account
that has access (`gh auth login` first):

```bash
gh release download --repo osbrjp/bungkus-mc --pattern install.sh -O - | bash
bungkus-mc            # or the short command: bkmc
bungkus-mc update     # later: install the newest release (--check only reports)
```

Once published, also:

```bash
brew install osbrjp/tap/bungkus-mc
npm install -g @osbrjp/bungkus-mc
```

The installer verifies the binary against the release's `checksums.txt`,
installs to `~/.local/bin` without sudo (`BUNGKUS_INSTALL_DIR` to
change; it prints the line to put that folder on `PATH` if needed), updates
an existing install where it is, and adds `bkmc` only when no command of
that name exists. mc checks for a newer
release once a day through `gh`; `BUNGKUS_NO_UPDATE_CHECK=1` turns that off.

Needs `claude` and/or `codex` on `PATH`. Any monospace font works.

## First run

A short wizard asks for the **workspace** (a folder whose child folders
are projects: they contain `CLAUDE.md`, `AGENTS.md` or `.git`), the
**default agent**, and the **theme** (auto, dark "Daun Teduh", light
"Santan"; the screen previews it). `esc` on the first step skips with
defaults. Change these later with `,`.

A project that is a linked git worktree of another project in the
workspace (its `.git` is a file pointing into that repo) is listed right
below it as a branch: `├ i746` under `nrha-timii`.

`bungkus-mc ~/Works` opens a workspace directly.

## Keys

| Keys | What |
|------|------|
| `n` | new session in the selected project (agent, model, name, optional prompt) |
| `enter` · `l` · `→` · `tab` | talk to the selected session (**INTERACT**: every key goes to the agent) |
| `ctrl-\` | leave INTERACT |
| `ctrl-h` · `ctrl-l` | pane left · right, also from inside INTERACT (`R` redraws) |
| `cmd`/`alt`/`ctrl` + `1` `2` `3` | projects · sessions · output pane, also from inside INTERACT (`cmd` needs the terminal mapping below) |
| `!` · `ctrl-]` | jump to the next session that needs you, in any project |
| `j` `k` · `↓` `↑` · `gg` `G` | move |
| `x` | stop a session (lists what it started too) |
| `N` | quick session at the workspace root, in a popup. Every key goes to the agent; `ctrl-m` opens the move dialog (in terminals with the kitty keyboard protocol: Ghostty, kitty, WezTerm, iTerm2; elsewhere ctrl-m is enter) and `ctrl-\` opens the popup menu: `h` hide · `m` move. The move dialog takes a name: it lists the 3 most recent projects, then the 3 best matches as you type, with a last row to create one (`↑`/`↓` or `ctrl-j`/`ctrl-k` pick); a name that matches no project creates it (folder + `git init`). The conversation comes along. A hidden one waits under the `quick` row, number `0` at the top of the list (`enter` reopens it, or resumes it once it has ended) |
| `r` | resume the selected finished session; anywhere else, open the agent's own list of this project's past sessions (`claude --resume` / `codex resume`) to pick any of them |
| `d` | forget a finished session |
| `z` | zoom the output pane |
| `a` | new project in the workspace: a name, then fresh (`.git`) or with agent files (`.git` + `AGENTS.md` + a `CLAUDE.md` that imports it with `@AGENTS.md`) |
| `dd` · `V` then `d` · `u` | move the project (or a `V` line selection of projects) to the Trash after a confirm · undo it |
| `/` | search projects (`↑` `↓` pick, `enter` open, `esc` clear) |
| `1`–`9` | jump to project N; type the next digit quickly for two digits (`1` `6` → 16) |
| `,` · `w` | settings · workspace |
| `ctrl-j` `ctrl-k` · `ctrl-n` `ctrl-p` | down · up in every dialog, list and the `/` search (same as `↓` `↑`) |
| `?` · `space` | key menu: every key for the pane; press one to run it |
| `q` | quit (stops sessions and what they started, after a confirm) |

INTERACT is signalled four ways at once: double border, `INTERACT` in the
title, the reverse-video mode word and the hint. The mouse wheel scrolls
mc's scrollback; to select text, hold `shift` (kitty, Ghostty, iTerm2,
WezTerm, Alacritty) or `option` (Terminal.app) while dragging.

**`cmd` + 1 2 3 on macOS.** Terminals keep `cmd`-digits for their own tabs,
so map them to the sequences mc reads (this gives up the terminal's
`cmd`-1..3 tab switching):

- Ghostty (`~/.config/ghostty/config`):
  ```
  keybind = cmd+1=csi:49;9u
  keybind = cmd+2=csi:50;9u
  keybind = cmd+3=csi:51;9u
  ```
- kitty (`kitty.conf`): `map cmd+1 send_text all \x1b[49;9u` (and `50`, `51`
  for 2 and 3).
- WezTerm: `{ key = '1', mods = 'CMD', action = wezterm.action.SendString '\x1b[49;9u' }`
  (and 2, 3).
- iTerm2: Settings → Profiles → Keys → Key Mappings → `+`, shortcut `⌘1`,
  action "Send Escape Sequence", `[49;9u` (and `[50;9u`, `[51;9u`).

`alt` and `ctrl` + 1 2 3 do the same where the terminal passes them on.

**If a chord does nothing**, your terminal probably keeps it (IDE terminals
bind many `ctrl`/`cmd` chords; macOS terminals keep `cmd`-digits for tabs).
Run mc with `BUNGKUS_MC_DEBUG_KEYS=1` and the hint line shows every key mc
receives; a chord that never shows up there never reached mc.

**If `ctrl-\` is taken** (tmux with vim-tmux-navigator, VS Code / Cursor,
or a JIS or German keyboard), set another chord, for example
`"interactExit": "ctrl-^"`.

## Config

`~/.config/bungkus/mc/config.json` (all keys optional; the settings screen
writes `workspace`, `defaultAgent` and `theme`, a border drag writes
`panes`, and everything else is kept):

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
  "cleanup": { "keep": ["postgres"] },
  "panes": { "projects": 22, "sessions": 38 }
}
```

- `background: "terminal"` keeps your terminal's own background (mc paints
  its green only on TrueColor terminals anyway).
- `icons`: `ascii` (default), `unicode`, `nerd` (also `--icons`).
- `notify`: `bell` (default), `desktop` (plus OSC 9/99/777), `off`.
- `cleanup.keep`: process names the quit dialog starts as `[keep]`.
- `panes`: pane widths; drag a pane's right border with the mouse and mc
  saves them here (projects 16–40, sessions 28–72, output keeps 40).

Sessions are remembered in `~/.local/state/bungkus/mc/sessions.json`.
`bungkus-mc --debug` also writes a debug log next to it (`mc.log`, 0600):
launches, exits, hook event names, stops and moves — never what you type,
prompts or agent output.

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
  under their project (or under `elsewhere` at the end of the list when
  they run in no project folder) with their state and pid, from `claude agents
  --json` and running `codex` processes. `enter` on a Claude one takes it
  over: quit it in its own terminal (`/exit`) and mc resumes it with its
  history (`claude --resume`) in the output pane. `x` stops one after a
  confirm.

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
