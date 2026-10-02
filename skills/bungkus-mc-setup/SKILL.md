---
name: bungkus-mc-setup
description: Install and configure bungkus-mc (short command `bkmc`), the terminal mission control for Claude Code and Codex CLI sessions, on the user's machine — check prerequisites, run the installer, pick a workspace, write `config.json`, choose the icon set (Nerd Font or ASCII), add a per-workspace `.bungkus-mc/` folder, and fix common first-run problems. Use when the user asks to install, set up, configure, update, uninstall or troubleshoot bungkus-mc / bkmc / "mission control", or reports boxes instead of icons, "output only" cards, or a key that does not work in mc.
---

# bungkus-mc-setup

Walk the user through installing and configuring bungkus-mc: one screen with
the projects of a workspace (left), the selected project's agent sessions
(middle) and the selected agent's live UI (right).

## Rules

- **Look first.** Run the checks in step 1 before proposing anything.
- **Ask before every write.** Show the exact command or the exact file
  content and the path, then wait for a yes. Never overwrite an existing
  file; change only the keys the user agreed to and keep the rest.
- **No secrets.** Do not print, copy or store API keys, tokens or the
  contents of the agents' own config and session files. mc needs none.
- **Do not touch the agents' config** (`~/.claude`, `~/.codex`). mc injects
  its hooks per launch and writes nothing there.
- **mc is a full-screen TUI.** You cannot drive it from here. Run only the
  non-interactive commands (`--version`, `--help`, `update --check`); tell
  the user to open `bungkus-mc` in their own terminal.
- **The binary is the source of truth.** `bungkus-mc --help` lists the
  current flags; if it differs from this file, follow `--help`.

## 1. Inspect the machine

```bash
uname -sm                                  # Darwin or Linux; arm64/aarch64 or x86_64/amd64
command -v claude codex                    # at least one must be on PATH
command -v bungkus-mc bkmc                 # already installed?
bungkus-mc --version                       # only if installed
ls -la "${XDG_CONFIG_HOME:-$HOME/.config}/bungkus/mc/" 2>/dev/null
echo "$TERM_PROGRAM / $TERM / ${SSH_CONNECTION:+ssh} / ${LC_ALL:-${LC_CTYPE:-$LANG}}"
```

- **Supported:** macOS and Linux, arm64 and amd64. No Windows, no remote
  agents. Stop here on anything else.
- **Needs** `claude` and/or `codex` on `PATH`. If neither is there, tell the
  user to install one first and stop; do not install an agent for them
  unless they ask.
- **Font:** any monospace font works.
- If `config.json` exists, read it and tell the user what it sets before
  suggesting changes.

## 2. Install

```bash
curl -fsSL https://raw.githubusercontent.com/osbrjp/bungkus-mc/main/install.sh | bash
```

The installer verifies the binary against the release's `checksums.txt`,
installs to `~/.local/bin` without sudo, prints the line to put that folder
on `PATH` if needed, updates an existing install where it is, and adds
`bkmc` only when no command of that name exists.

Another folder (the variable goes on `bash`):

```bash
curl -fsSL https://raw.githubusercontent.com/osbrjp/bungkus-mc/main/install.sh | BUNGKUS_INSTALL_DIR=/opt/bin bash
```

Verify:

```bash
bungkus-mc --version
command -v bkmc
```

If `bungkus-mc` is not found, `~/.local/bin` is not on `PATH`: relay the
line the installer printed and let the user add it to their shell profile
(ask before you edit a profile file).

## 3. First run

Ask which folder is the user's **workspace**: a folder whose child folders
are projects. A child folder counts as a project when it contains
`CLAUDE.md`, `AGENTS.md` or `.git`. Hidden folders are not listed. You can
check a candidate:

```bash
for d in ~/Works/*/; do
  [ -e "$d/CLAUDE.md" ] || [ -e "$d/AGENTS.md" ] || [ -e "$d/.git" ] && echo "$d"
done
```

Then the user runs, in their own terminal:

```bash
bungkus-mc              # or: bkmc
bungkus-mc ~/Works      # open a workspace directly
```

The first run shows a three-step wizard: **workspace**, **default agent**
(`claude` or `codex`), **theme** (auto, dark, light; the screen previews
it). `esc` on the first step skips with defaults. `,` opens settings later.

Keys worth telling a new user:

| Keys | What |
|------|------|
| `?` or `space` | key menu for the current pane |
| `n` | new session in the selected project |
| `enter` | talk to the selected session (INTERACT: every key goes to the agent) |
| `ctrl-\` | leave INTERACT |
| `!` | jump to the next session that needs you |
| `w` | workspace switcher |
| `,` | settings |
| `q` | quit (stops sessions and what they started, after a confirm) |

Other ways to open mc: `bungkus-mc -q [WORKSPACE]` (quick session popup),
`bungkus-mc -p PROJECT [WORKSPACE]` (start on a project).

## 4. config.json

Path: `~/.config/bungkus/mc/config.json` (`$XDG_CONFIG_HOME/bungkus/mc/` when
that variable is set). All keys are optional; a missing file means defaults.
The wizard and the settings screen already write `workspace`, `defaultAgent`
and `theme`, so most users need this file only for the other keys.

A starting point (leave out what the user does not need):

```json
{
  "workspace": "/Users/me/Works",
  "defaultAgent": "claude",
  "theme": "auto",
  "icons": "auto",
  "notify": "bell"
}
```

| Key | Values |
|-----|--------|
| `workspace` | the workspace folder; may start with `~` |
| `defaultAgent` | `claude` · `codex` |
| `theme` | `auto` (follows the terminal's background) · `dark` · `light` |
| `background` | `paint` (default) · `terminal` keeps the terminal's own background |
| `icons` | `auto` (default) · `ascii` · `unicode` · `nerd`; see step 5 |
| `motion` | `true` (default) · `false`: spinners and the mascot stand still |
| `mouse` | `true` (default) · `false` |
| `notify` | `bell` (default) · `desktop` · `off` |
| `interactExit` | the chord that leaves INTERACT (default `ctrl-\`), e.g. `"ctrl-^"` |
| `agents.claude` / `agents.codex` | `{ "command": "claude", "args": [] }`: command name or path, and extra arguments |
| `workspaces` | saved workspaces for the `w` switcher (mc writes this) |
| `cleanup.keep` | process names the quit dialog starts as `[keep]`, e.g. `["postgres"]` |
| `worktrees` | `true` (default) · `false`: a Claude session joining a project where another mc session runs gets its own git worktree |
| `panes` | `{ "projects": 22, "sessions": 38 }`; projects 16–40, sessions 28–72 (mc writes this when a border is dragged) |

Editing rules:

- Read the file first. Merge keys into the existing object; never replace
  the file. Show the diff and ask.
- It must stay one valid JSON object (no comments, no trailing commas).
  Check with `python3 -m json.tool < FILE` or `jq . FILE` if available.
- New file: `mkdir -p` the folder with mode 0700 and give the file mode
  0600, the modes mc itself uses.
- Do not put secrets in `agents.*.args`.
- Tell the user to quit and reopen mc after an edit.

## 5. Icons and Nerd Fonts

`icons` picks the state glyph set: `auto` | `ascii` | `unicode` | `nerd`.
The same values work once with `bungkus-mc --icons SET`.

`auto` (the default) uses `nerd` when a Nerd Font is **installed** on the
machine, else `ascii`. It looks for a file or folder with `nerd` in its
name in the usual font folders (`~/Library/Fonts`, `/Library/Fonts`,
`~/.local/share/fonts`, `~/.fonts`, `/usr/local/share/fonts`,
`/usr/share/fonts`). It never picks `nerd` over SSH or on a non-UTF-8
locale.

mc cannot see which font the terminal actually uses. So:

- **Icons show as boxes or question marks:** a Nerd Font is installed but
  the terminal uses another font. Either select the Nerd Font in the
  terminal's settings, or set `"icons": "ascii"`.
- **The user wants the icons:** they need both: a Nerd Font installed, and
  that font selected in their terminal. A "Mono" variant keeps every icon
  one cell wide; with a "Propo" variant set `ascii`.
- **Over SSH:** the font is on the user's own machine, which mc cannot see;
  set `"icons": "nerd"` by hand if their terminal has one.
- **Unsure:** try `bungkus-mc --icons nerd` and `--icons ascii` and keep the
  one that looks right.

You can check what `auto` will find:

```bash
find ~/Library/Fonts /Library/Fonts ~/.local/share/fonts ~/.fonts \
  /usr/local/share/fonts /usr/share/fonts -maxdepth 3 -iname '*nerd*' 2>/dev/null | head -5
```

Do not install a font unless the user asks; choosing the terminal font is
theirs to do in the terminal's own settings.

Borders and the mascot are not part of the icon set: they follow the
locale (ASCII on a non-UTF-8 locale).

## 6. The workspace's own `.bungkus-mc/` folder

Optional. `<workspace>/.bungkus-mc/` holds what is specific to one
workspace. mc only reads it, at start and on every workspace switch.

| File | Effect |
|------|--------|
| `config.json` | overrides the global file for this workspace, key by key. Only `defaultAgent`, `worktrees`, `notify` and `cleanup.keep` are read; every other key is ignored there |
| `CLAUDE.md` | appended to the system prompt of every Claude session mc starts or resumes in the workspace |
| `AGENTS.md` | given to every Codex session mc starts in the workspace as developer instructions (the first 64 KiB; it takes the place of `developer_instructions` in `~/.codex/config.toml`) |

Both instruction files add to the ones the agents already read from each
project. Sessions started outside mc get neither. A `config.json` there
that cannot be parsed sets nothing, and mc's message line says so.

Offer this when the user wants one rule for every project in a workspace
("always answer in Japanese", "never push") or a different default agent
per workspace. Ask what the instructions should say; do not invent them.

## 7. Update and uninstall

```bash
bungkus-mc update --check    # only reports
bungkus-mc update            # installs the newest release
```

Inside mc, `U` updates and restarts. mc checks for a newer release once an
hour and shows it in the header; `BUNGKUS_NO_UPDATE_CHECK=1` turns the
check off.

```bash
bungkus-mc uninstall            # the binary and its bkmc link
bungkus-mc uninstall --purge    # also config, remembered sessions and the update cache
```

`uninstall` lists what it removes and asks first; let the user answer it
themselves rather than passing `--yes`. The agents' own files are never
touched.

## 8. Troubleshooting

| Symptom | Cause and fix |
|---------|---------------|
| Icons are boxes | step 5: set `"icons": "ascii"` or select the Nerd Font in the terminal |
| A Codex card says `hooks not trusted · /hooks in codex` | Codex asks once to trust mc's hooks: in the session, answer "Hooks need review" with "Trust all and continue". The trust is remembered while mc stays at the same path |
| A Claude card says `output only` | no hook event arrived: the workspace-trust prompt in that session is not answered yet (answer it in INTERACT), or managed settings turn hooks off (`disableAllHooks` / `allowManagedHooksOnly`). The live pane still works; the card recovers on the first event |
| Every card says `output only` | mc could not create its socket (the runtime directory is not usable or its path is too long). Check `$XDG_RUNTIME_DIR` / `$TMPDIR` |
| A chord does nothing | the terminal keeps it. `BUNGKUS_MC_DEBUG_KEYS=1 bungkus-mc` shows every key mc receives in the hint line |
| `ctrl-\` is taken (tmux + vim-tmux-navigator, VS Code / Cursor, JIS or German keyboards) | set `"interactExit": "ctrl-^"` or another chord |
| `cmd`+`1` `2` `3` do nothing on macOS | the terminal keeps them for tabs. `alt`/`ctrl` + digit work without setup; for `cmd`, see "Terminal setup" in the README (Ghostty, kitty, WezTerm, iTerm2 mappings) |
| A folder is missing from the projects list | it has no `CLAUDE.md`, `AGENTS.md` or `.git`, or it is not a direct child of the workspace |
| mc's green background is unwanted | `"background": "terminal"` |
| Cannot select text with the mouse | hold `shift` while dragging (`option` in Terminal.app) |
| Something else | `bungkus-mc --debug` writes `~/.local/state/bungkus/mc/mc.log` (launches, exits, hook event names; never prompts or agent output). Ask before reading it |

Known limits: sessions do not survive mc (resume them with `r`); two mc
instances on one workspace share `sessions.json`, last writer wins.

## 9. Finish

Tell the user what was installed and where, which files you wrote or
changed, and the command to open mc. Point them to `?` inside mc and to
the README at https://github.com/osbrjp/bungkus-mc for the full key list.
