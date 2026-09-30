# CLAUDE.md

## Project Overview

**bungkus-mcc** ("mission control") is a Go terminal TUI for people who run
AI coding agents (Claude Code, Codex CLI) in the terminal. One screen:
projects in a workspace (left), agent sessions with their subagents and
usage figures per project (middle), the selected agent's real interactive
UI (right), plan limits in the status bar. Sibling of bungkus-cli; same
tooling, release pipeline and conventions. Status: **proposal /
pre-development** — no application code yet. Read the docs before writing any.

## Docs (read in this order)

- `docs/PROPOSAL.md` — goals, non-goals, stage plan (M1–M9), decided list, open questions, risks
- `docs/ARCHITECTURE.md` — process model, how subagents and usage are observed (PTY + hooks + status line over a unix socket), adapter boundary, state machine, modes/keys, storage, concurrency rules, package layout, bungkus-cli relationship, terminal compatibility
- `docs/DESIGN.md` — "Daun Pisang" design language: palette with contrast + declared 256/16 values, glyphs, generated mockups, components, states, keybindings, notifications, microcopy
- `docs/TECH_STACK.md` — every dependency and why
- `docs/CODING_RULES.md` — conventions, tests, review checklist, YAGNI rules
- `docs/SECURITY.md` — threat model and rules

## Tech Stack

- **Go 1.26** — single static binary, darwin/linux × arm64/amd64
- **Cobra** — `bungkus-mcc [workspace]` (TUI), `hook`, `statusline`, `setup codex` (if needed), `update`
- **Bubble Tea v2 / Lip Gloss v2 / Bubbles v2** (`charm.land/*`) — versions match bungkus-cli
- **creack/pty + charmbracelet/x/vt** — agents run in a PTY, rendered by an embedded VT emulator; **x/ansi** for `Strip` and colour conversion
- stdlib for JSON config/state, unix socket IPC, slog

## Planned Structure

```
main.go, version.go          # Version via -ldflags (as bungkus-cli)
cmd/                         # root (TUI), hook, statusline, setup, update (copied, with source header)
internal/theme/              # Daun Pisang tokens (hex + 256 + 16), Icon(), styles (copy-able file)
internal/tui/                # tea.Model, keymap.go (single source of keys/help), panes, dialogs, goldens
internal/agent/              # Event, Usage, Adapter, decided state machine; claude.go, codex.go, codexusage.go; testdata/ recorded payloads
internal/term/               # pty + emulator + reply pump, key translation table, output sanitiser (allowlist)
internal/proc/               # descendant tracking: tree scan (/proc, ps), ports (/proc/net/tcp, lsof), kill
internal/route/              # TypeSafe Jev tier judgement → model id (opt-in)
internal/ipc/                # unix socket server/client
internal/store/              # XDG paths, config.json, sessions.json
internal/workspace/          # project dir scan
```

## Key Decisions (don't relitigate without reading the docs)

- Live pane = PTY + VT emulator with a reply-pump goroutine (x/vt blocks `Write` otherwise) and our own key translation table (x/vt has no kitty encoder). Rendered output passes an allowlist (printable + `CSI…m`). **Focusing the output pane is INTERACT** (full passthrough); `ctrl-\` returns to the sessions pane; there is no output-pane NORMAL mode.
- Structure = agent hooks → `bungkus-mcc hook` (silent, trimmed fields) → unix socket. Usage = Claude status line → `bungkus-mcc statusline` (forwards within 200 ms, then runs the user's own status line under `sh` with the buffered stdin). **Never parse agent transcripts — except `internal/agent/codexusage.go`**, which tails only `token_count` records of the rollout file Codex's hook names (owner-approved).
- Quit and `x` also stop the agents' observed descendants (dev servers): 2 s process-tree scan (uid-filtered, `LC_ALL=C` `ps` / `/proc` + pidfd), pid + start-time identity, listed in the dialog with a `[stop]/[keep]` toggle and a default-keep list (`*.app`, `ssh-agent`, `tmux`, `docker`, `code`, …, `cleanup.keep`), SIGTERM → 3 s → SIGKILL; never by port, never unobserved processes; a user stop yields state `stopped` (`internal/proc`).
- Mascot while a session runs: top-right corner of the output pane, full (16×7 cells) when quiet, mini (8×3) when the PTY is busy, drawn only over blank cells (else `/..\` / `/xx\` in the title bar), mood by state; cross eyes for anything that fails. Sidebar shows an ASCII spinner for projects with a running session. One global 350 ms animation clock, armed only while something animated is visible and `motion` is on.
- Routing sends only the `n` start prompt (never resume/INTERACT), skips secret-shaped prompts, caps the response at 64 KiB, keeps consent in `consent.json`; `TYPESAFE_API_KEY` is unset in child envs.
- Middle pane = selected project's sessions; sidebar badges show the rest; `!` jumps across projects. Card title = the session's own name (`--name`, `session_name`, `session_title`; prompt, then `untitled`).
- Projects = child folders containing `CLAUDE.md`, `AGENTS.md` or `.git`. Default icon set is ASCII; borders follow the locale.
- Model routing (TypeSafe Jev, `internal/route`) is opt-in, off by default, start-prompt only, consent dialog, key from env/keychain command; M9 / v0.2.0.
- bungkus-cli adopts Daun Pisang now; both ship together (owner implements the bungkus-cli side).
- Claude hooks + statusLine injected per session with `--settings` (merges with user hooks — verified). Codex hooks injected per launch with `-c hooks.*` (verified); trust persistence is M6's first task. Never `--dangerously-*`.
- New: `claude --session-id <uuid> --settings … -- <prompt>`; resume: `claude --resume <id> --settings …` (never both flags). `--` before prompts on both CLIs.
- States: running / your turn / needs you / failed / wrapped / stopped; `background_tasks` is authoritative for subagents; `idle_prompt` ignored; sidebar precedence failed > needs you > running > your turn.
- Quitting stops sessions (confirm); resumable. No daemon in stage 1.
- Modes: NORMAL (projects/sessions panes, vim + arrows) and INTERACT (output pane); exit chord `ctrl-\` by default, configurable (`interactExit`); `ctrl-z` swallowed. All keys in `internal/tui/keymap.go`, unique per pane/mode.
- mcc paints a low-saturation green background (`#1c2a21` dark / `#f0f3d8` light) **only at TrueColor** (`background: paint`, default); at 256/16/`NO_COLOR` the terminal's own bg/fg and the declared indices. Every state is glyph + word + colour; state glyphs are East-Asian-Narrow; `notify` default `bell`.
- Mascot (banana-leaf packet, Figma `HwlCHEFqRm9hfOfUbtuL4h` 17:3 with poses idle/look-up-left/look-up-right/duck/died, `docs/assets/mascot/`): 16×14 and 8×6 half-block sprites (idle/blink/lookL/lookR/duck/hop/stepL/stepR/died), fixed brand colours + a per-theme legs token, mood sequences per session state; never over agent output.
- Separate repo and binary from bungkus-cli; shared code is copied with a `// copied from …@<sha>` header. Eight direct deps; adding one requires a TECH_STACK.md entry.
- Every string not from the PTY (hook fields, prompts, dir names) goes through `sanitise()`.

## Build, Run, Test (once code exists)

```bash
go build -o bungkus-mcc .
go run . ~/Works                # TUI with a workspace
go test ./...                   # includes teatest goldens; -update to regenerate
```

## Conventions

- Conventional commits (`feat:`, `fix:`, `test:`, `chore:`, `docs:`); semantic-release: `main` = canary, `release` = stable (merge commit, never squash the promotion PR)
- Branch naming: `i{issue#}-{date}-{seq}` (e.g. `i12-20261007-0930`)
- GitHub repo: `osbrjp/bungkus-mcc`
- Tests next to code, table-driven; recorded hook payloads in `testdata/`; goldens at 120×40 and 80×24 with `NO_COLOR=1 --icons ascii`
- No shell in `exec` for anything mcc decides; no raw agent bytes to stdout; no secrets persisted (see `docs/SECURITY.md`)
