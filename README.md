# bungkus-mc

<p align="center"><img src="docs/assets/mascot.gif" width="160" alt="the bungkus mascot: a green banana-leaf packet with tan paper corners, hopping"></p>

<p align="center">
  <b>Mission control for AI coding agents in the terminal.</b><br>
  Claude Code and Codex CLI, across every project in a workspace, on one screen.
</p>

<p align="center">
  <img src="docs/assets/promo.gif" width="720" alt="A 45-second tour of bungkus-mc: projects, agent sessions with usage, and the selected agent's live screen on one screen"><br>
  <a href="docs/assets/promo.mp4">Download the tour with sound (mp4)</a>
</p>

---

| Pane | Shows |
|------|-------|
| **Left** | the projects in your workspace |
| **Middle** | the selected project's sessions, with their subagents and usage (tokens, cost, context %) |
| **Right** | the selected agent's live interactive UI |
| **Status bar** | your plans' 5-hour / weekly limits |

One small Rust binary, no daemon. A sibling of
[bungkus-cli](https://github.com/osbrjp/bungkus-cli): same release
pipeline, same "Daun Pisang" design language.

> **Status:** beta (`0.1.0-beta.8`). Milestones M1–M8 are built; model
> routing (M9) is not.

**Contents:** [Install](#install) · [First run](#first-run) ·
[Keys](#keys) · [Terminal setup](#terminal-setup) · [Config](#config) ·
[How it works](#how-it-knows-what-the-agents-do) ·
[Quitting](#quitting-and-cleanup) · [Limits](#limits-v01) ·
[Build](#build) · [Documents](#documents)

---

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/osbrjp/bungkus-mc/main/install.sh | bash
```

**Needs** `claude` and/or `codex` on `PATH`. Any monospace font works.

### Let Claude set it up

[`skills/bungkus-mc-setup`](skills/bungkus-mc-setup/SKILL.md) is a Claude
Code skill that installs and configures mc with you: it checks your
machine, runs the installer, helps with `config.json`, the icon set and a
workspace's `.bungkus-mc/` folder, and asks before it writes any file. Copy
the folder to `~/.claude/skills/bungkus-mc-setup`, then ask Claude Code to
"set up bungkus-mc".

### Commands

| Command | What |
|---------|------|
| `bungkus-mc` | open mc (short command: `bkmc`) |
| `bungkus-mc WORKSPACE` | open a workspace directly, e.g. `bungkus-mc ~/Works` |
| `bungkus-mc -q [WORKSPACE]` | open mc with a quick session popup (default: your last workspace) |
| `bungkus-mc -p PROJECT [WORKSPACE]` | open mc on a project; asks for the workspace when the project is not in the last one |
| `bungkus-mc update` | install the newest release (`--check` only reports). Inside mc: press `U` |
| `bungkus-mc uninstall` | remove mc (`--purge` also removes config and sessions) |
| `bungkus-mc --debug` | also write a debug log (see [Config](#config)) |

### What the installer does

- Verifies the binary against the release's `checksums.txt`.
- Installs to `~/.local/bin` without sudo, and prints the line to put that
  folder on `PATH` if needed.
- Updates an existing install where it is.
- Adds `bkmc` only when no command of that name exists.

Another folder (the variable goes on `bash`, which runs the script):

```bash
curl -fsSL …/install.sh | BUNGKUS_INSTALL_DIR=/opt/bin bash
```

### Updates

mc checks for a newer release once an hour, also while it runs; a newer
one stays in the header (`v0.1.0 → v0.2.0 · U updates`) until you update.
`BUNGKUS_NO_UPDATE_CHECK=1` turns that off.

### Uninstall

`bungkus-mc uninstall` lists what it removes and asks first (`--yes` skips
the question).

| Flag | Removes |
|------|---------|
| (none) | the binary and its `bkmc` link |
| `--purge` | also `~/.config/bungkus/mc`, `~/.local/state/bungkus/mc` and the update cache |

Your agents' own files are never touched.

---

## First run

A short wizard asks three things. `esc` on the first step skips with
defaults; change them later with `,`.

| Step | What |
|------|------|
| **Workspace** | a folder whose child folders are projects |
| **Default agent** | `claude` or `codex` |
| **Theme** | auto, dark "Daun Teduh", light "Santan" (the screen previews it) |

**What counts as a project:** a child folder that contains `CLAUDE.md`,
`AGENTS.md` or `.git`.

**Git worktrees:** a project that is a linked worktree of another project
in the workspace (its `.git` is a file pointing into that repo) is listed
right below it as a branch, e.g. `├ i746` under `nrha-timii`.

---

## Keys

Press `?` or `space` inside mc for the key menu: every key for the current
pane; press one to run it.

### Move around

| Keys | What |
|------|------|
| `j` `k` · `↓` `↑` · `gg` `G` | move |
| `ctrl-h` · `ctrl-l` | pane left · right, also from inside INTERACT |
| `cmd`/`alt`/`ctrl` + `1` `2` `3` `4` | projects · sessions · output · terminal pane, also from inside INTERACT (`cmd` needs a [terminal mapping](#cmd--1-2-3-on-macos)) |
| `1`–`9` | jump to project N; type the next digit quickly for two digits (`1` `6` → 16) |
| `/` | search projects (`↑` `↓` pick, `enter` open, `esc` clear) |
| `fp` · `ff` · `fg` | finder popup over the workspace: projects, file names, grep (`enter` opens the project, or the file in your editor; `ff` and `fg` need [ripgrep](https://github.com/BurntSushi/ripgrep)) |
| `!` · `ctrl-]` | jump to the next session that needs you, in any project |
| `ctrl-j` `ctrl-k` · `ctrl-n` `ctrl-p` | down · up in every dialog, list and the `/` search (same as `↓` `↑`) |
| `z` | zoom the output pane |
| `R` | redraw |
| `t` | show / hide the terminal pane `[4]`: your `$SHELL` below the output pane, one shell per session, started in the folder the session works in (its worktree when it has one); with no session selected, one for the project. A card with a shell shows `>_`; the shell is closed when its session ends. The wheel scrolls it; while it is hidden a `[4] terminal` marker shows at the bottom of the output pane |
| `T` | close the selected session's shell (`exit` or `ctrl-d` inside it does the same) |

### Sessions

| Keys | What |
|------|------|
| `n` | new session in the selected project (agent, model, name, optional prompt) |
| `N` | quick session at the workspace root, in a popup (see [Quick session](#quick-session-n)) |
| `enter` · `l` · `→` · `tab` | talk to the selected session (**INTERACT**: every key goes to the agent) |
| `ctrl-\` | leave INTERACT |
| `r` | on a finished session: resume it. Anywhere else: open the agent's own list of this project's past sessions (`claude --resume` / `codex resume`) to pick any of them |
| `x` | stop a session (lists what it started too) |
| `d` | forget a finished session |

### Projects and workspaces

| Keys | What |
|------|------|
| `a` | new project in the workspace: a name, then fresh (`.git`) or with agent files (`.git` + `AGENTS.md` + a `CLAUDE.md` that imports it with `@AGENTS.md`) |
| `dd` | move the project to the Trash, after a confirm |
| `V` then `d` | the same for a line selection of projects |
| `u` | undo the move to the Trash |
| `c` | projects or sessions pane: remove the selected project's unused git worktrees after a confirm — never one a session runs in or can resume into, never one with uncommitted files; branches stay |
| `w` | workspace switcher (see [Workspace switcher](#workspace-switcher-w)) |

### App

| Keys | What |
|------|------|
| `,` | settings |
| `U` | update: install the newest release in the background, then restart mc on it (running sessions are asked about first, as on quit; `r` resumes them) |
| `?` · `space` | key menu |
| `q` | quit (stops sessions and what they started, after a confirm) |

### INTERACT mode

Focusing the output pane sends every key to the agent. `ctrl-\` leaves.

- **How you can tell:** double border, `INTERACT` in the title, the
  reverse-video mode word, and the hint line.
- **Mouse wheel:** scrolls mc's scrollback.
- **Select text:** hold a key while dragging.

  | Terminal | Hold |
  |----------|------|
  | kitty, Ghostty, iTerm2, WezTerm, Alacritty | `shift` |
  | Terminal.app | `option` |

### Quick session (`N`)

A session at the workspace root, in a popup. Every key goes to the agent,
except:

| Keys | What |
|------|------|
| `ctrl-\` | popup menu: `h` hide · `m` move |
| `ctrl-m` | move dialog directly. Only in terminals with the kitty keyboard protocol (Ghostty, kitty, WezTerm, iTerm2); elsewhere `ctrl-m` is `enter` |

**Move** puts the session into a project; the conversation comes along.
The dialog takes a name:

- it lists the 3 most recent projects, then the 3 best matches as you type;
- the last row creates a project;
- `↑`/`↓` or `ctrl-j`/`ctrl-k` pick;
- a name that matches no project creates it (folder + `git init`).

**Hide** keeps the session under the `quick` row, number `0` at the top of
the project list. `enter` reopens it, or resumes it once it has ended.

### Workspace switcher (`w`)

Lists saved workspaces, recent first, with project counts.

| Keys | What |
|------|------|
| type | filter |
| `1`–`9` · `enter` | switch |
| `+ add a folder…` | add a workspace |
| `ctrl-d` | remove one from the list |

Sessions keep running in the background; `!` follows one that needs you
into its workspace.

---

## Terminal setup

Most terminals need nothing. Read this section only when a key does not
work.

### A chord does nothing

Your terminal probably keeps it for itself: IDE terminals bind many
`ctrl`/`cmd` chords, and macOS terminals keep `cmd`-digits for tabs.

```bash
BUNGKUS_MC_DEBUG_KEYS=1 bungkus-mc
```

The hint line now shows every key mc receives. A chord that never shows up
there never reached mc.

### `ctrl-\` is taken

This happens with tmux + vim-tmux-navigator, VS Code / Cursor, and JIS or
German keyboards. Set another chord in [`config.json`](#config):

```json
{ "interactExit": "ctrl-^" }
```

### `cmd` + 1 2 3 on macOS

Terminals keep `cmd`-digits for their own tabs, so mc never sees them. Map
them to the sequences mc reads. This gives up the terminal's `cmd`-1..3
tab switching.

`alt` + 1 2 3 and `ctrl` + 1 2 3 do the same with no mapping, where the
terminal passes them on.

**Ghostty** (`~/.config/ghostty/config`)

```
keybind = cmd+1=csi:49;9u
keybind = cmd+2=csi:50;9u
keybind = cmd+3=csi:51;9u
```

**kitty** (`kitty.conf`)

```
map cmd+1 send_text all \x1b[49;9u
map cmd+2 send_text all \x1b[50;9u
map cmd+3 send_text all \x1b[51;9u
```

**WezTerm** (in `keys`)

```lua
{ key = '1', mods = 'CMD', action = wezterm.action.SendString '\x1b[49;9u' },
{ key = '2', mods = 'CMD', action = wezterm.action.SendString '\x1b[50;9u' },
{ key = '3', mods = 'CMD', action = wezterm.action.SendString '\x1b[51;9u' },
```

**iTerm2** (Settings → Profiles → Keys → Key Mappings → `+`)

| Shortcut | Action | Value |
|----------|--------|-------|
| `⌘1` | Send Escape Sequence | `[49;9u` |
| `⌘2` | Send Escape Sequence | `[50;9u` |
| `⌘3` | Send Escape Sequence | `[51;9u` |

### kitty: `ctrl+h/j/k/l` window navigation

**Who this is for:** only kitty users who move between kitty windows with
`ctrl+h/j/k/l`, as vim-kitty-navigator setups do. Such a config has lines
like `map ctrl+j neighboring_window down` plus a
`--when-focus-on var:IS_VIM=true` passthrough.

**What mc does:** nothing to configure for the keys themselves. mc sets
the kitty user variable `IS_VIM=true` while it runs, so your existing
passthrough sends those keys to mc instead of kitty.

**At mc's edges** the key goes back to kitty and moves to the neighbouring
kitty window:

| Key | Handed back when you are on |
|-----|-----------------------------|
| `ctrl-h` | the projects pane (leftmost) |
| `ctrl-l` | the output pane (rightmost) |
| `ctrl-j` · `ctrl-k` | the list panes |

**Needs:** `allow_remote_control` in `kitty.conf`, because mc hands the
key back with `kitten @ focus-window --match neighbor:…`.

---

## Config

`~/.config/bungkus/mc/config.json`. All keys are optional.

```json
{
  "workspace": "/Users/me/Works",
  "defaultAgent": "claude",
  "theme": "auto",
  "editor": "nvim",
  "background": "paint",
  "icons": "auto",
  "motion": true,
  "mouse": true,
  "notify": "bell",
  "interactExit": "ctrl-\\",
  "agents": {
    "claude": { "command": "claude", "args": [] },
    "codex":  { "command": "codex",  "args": [] }
  },
  "workspaces": ["/Users/me/Works", "/Users/me/code"],
  "cleanup": { "keep": ["postgres"] },
  "worktrees": true,
  "panes": { "projects": 22, "sessions": 38 }
}
```

| Key | Values |
|-----|--------|
| `editor` | the command `o` opens a project with (`nvim`, `code --wait`, a full path); unset: `$VISUAL`, then `$EDITOR`, then an installed `nvim`/`vim`/`vi`. `O` opens the folder in Finder / the file manager |
| `background` | `paint` · `terminal` keeps your terminal's own background (mc paints its green only on TrueColor terminals anyway) |
| `icons` | `auto` (default: `nerd` when a Nerd Font is installed, else `ascii`) · `ascii` · `unicode` · `nerd` (also `--icons`, and the `icons` row of the settings screen, which shows the glyphs before you save). mc cannot see which font your terminal uses: if the glyphs show as boxes, choose `ascii`; if your terminal draws Nerd glyphs without an installed font (kitty does), choose `nerd`. In the `nerd` set the agent badge is the company's logo (Claude, OpenAI) instead of `C` / `X`; if it shows as a box see [Agent logos](#agent-logos) |
| `notify` | `bell` (default) · `desktop` (plus OSC 9/99/777) · `off` |
| `interactExit` | the chord that leaves INTERACT (default `ctrl-\`) |
| `cleanup.keep` | process names the quit dialog starts as `[keep]` |
| `worktrees` | `true` (default) · `false`; see [Worktrees](#worktrees) |
| `panes` | pane widths: projects 16–40, sessions 28–72, output keeps 40 |

**Who writes what:** the settings screen writes `workspace`,
`defaultAgent`, `theme`, `icons` and `editor`; dragging a pane's right border with the mouse
writes `panes`. Everything else is kept as you wrote it.

### Agent logos

In the `nerd` icon set the agent badge is the Claude or OpenAI logo
instead of `C` / `X`. The two glyphs (`nf-cod-claude`
U+EC82, `nf-cod-openai` U+EC81) are in **Nerd Fonts 3.5 or newer**; an
older font shows a box. Check your terminal:

```bash
printf 'claude: \xee\xb2\x82  openai: \xee\xb2\x81\n'
```

If you see boxes, install the current symbols font (macOS:
`brew install --cask font-symbols-only-nerd-font`; elsewhere
`NerdFontsSymbolsOnly` from the
[Nerd Fonts releases](https://github.com/ryanoasis/nerd-fonts/releases)).

**kitty** draws Nerd glyphs from its own bundled symbols font, which can
be older. Add this to `kitty.conf` and reload it, so the two logos come
from the installed font:

```
symbol_map U+EC81-U+EC82 Symbols Nerd Font Mono
```

To keep the letters instead, set `"icons": "ascii"` or `"unicode"`.

### Worktrees

With `worktrees` on, a Claude session started in a project where another
mc session already runs gets its own git worktree
(`claude --worktree <name>`, under the project's `.claude/worktrees/`), so
the two do not edit the same checkout.

- The first session stays in the main checkout.
- Resuming goes back into the same worktree.
- Forgetting the session (`d`) removes the worktree unless it has
  uncommitted files; its branch (`worktree-<name>`) stays.
- Codex sessions are not affected.

### Files

| Path | What |
|------|------|
| `~/.config/bungkus/mc/config.json` | config |
| `~/.local/state/bungkus/mc/sessions.json` | remembered sessions |
| `~/.local/state/bungkus/mc/mc.log` | debug log, only with `--debug` (mode 0600) |

The debug log holds launches, exits, hook event names, stops and moves —
never what you type, prompts or agent output.

---

## How it knows what the agents do

| What | Source |
|------|--------|
| **Structure** (working / your turn / needs you, subagents) | the agents' own hooks, injected per launch (`claude --settings`, `codex -c hooks.*`) |
| **Claude usage** | Claude's status line; your own status line keeps rendering (mc runs it for you) |
| **Codex usage** | `token_count` records in Codex's own session log, and nothing else |
| **MCP servers** on a card | the servers the session's tool calls name, so a server shows once the session has used it (connectors and plugin servers too). The selected card lists them by name and, on an `mcp idle` row, the servers that are configured but not used yet: the names in the agents' config (`.mcp.json` in the project and `.claude.json` for Claude, `config.toml` for Codex), checked every 5 s. Names only. With the `nerd` icon set a known server shows as its logo, on the selected card's `mcp` and `mcp idle` rows too |

- **Nothing is written to your agent config**, so there is nothing to
  remove; your own hooks keep running.
- **Codex asks once** to trust mc's hooks ("Hooks need review" → "Trust
  all and continue"). The trust is remembered while mc stays at the same
  path.

### Sessions started outside mc

Sessions from another terminal tab or an IDE are listed under their
project with their state and pid (from `claude agents --json` and running
`codex` processes). Those that run in no project folder go under
`elsewhere` at the end of the list.

| Keys | What |
|------|------|
| `enter` on a Claude one | take it over: quit it in its own terminal (`/exit`) and mc resumes it with its history (`claude --resume`) in the output pane |
| `x` | stop one, after a confirm |

---

## Quitting and cleanup

Quitting (or `x`) stops the agents **and the processes they started**,
such as dev servers.

- The dialog lists every process with its ports first.
- `space` keeps any of them.
- These start as `[keep]`: desktop apps, agents, multiplexers,
  `ssh-agent`, Docker, and the names in `cleanup.keep`.

mc only ever signals processes it saw start under its own agents,
identified by pid and start time, never by port.

---

## Limits (v0.1)

- macOS and Linux; no Windows, no remote agents.
- Sessions do not survive mc; resume them with `r` instead.
- On a non-UTF-8 locale, borders and the mascot fall back to ASCII, but
  separators such as `·` and `→` stay as they are.
- Two mc instances on one workspace share `sessions.json` (last writer wins).

---

## Build

```bash
cargo build --release                 # target/release/bungkus-mc (Rust 1.97, pinned)
```

What CI runs:

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo deny check
```

**Releases:** run `scripts/bump-version.sh` (the next beta or patch; or
pass a version) and merge that through a PR, then merge the release PR
(`main` → `release`, opened automatically). The release workflow builds
darwin/linux × arm64/amd64 and publishes `v<version>`.

---

## Documents

| Doc | What |
|-----|------|
| [docs/PROPOSAL.md](docs/PROPOSAL.md) | Goals, milestones, decisions, risks |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Process model, hooks and status line, emulator, storage |
| [docs/DESIGN.md](docs/DESIGN.md) | Daun Pisang: palette, glyphs, mascot, mockups, keys |
| [docs/TECH_STACK.md](docs/TECH_STACK.md) | Crates and rejected alternatives, CI, release |
| [docs/CODING_RULES.md](docs/CODING_RULES.md) | Behaviour rules, tests, review checklist |
| [docs/SECURITY.md](docs/SECURITY.md) | Threat model and rules |
| [skills/bungkus-mc-setup](skills/bungkus-mc-setup/SKILL.md) | A Claude Code skill that sets mc up with you |

---

## Licence

Proprietary. Copyright (c) 2026 OSBR. All rights reserved. See `LICENSE`.
Third-party licences ship as `THIRD-PARTY-NOTICES` with each release.
