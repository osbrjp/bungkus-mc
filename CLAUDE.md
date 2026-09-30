# CLAUDE.md

## Project Overview

**bungkus-mcc** ("mission control") is a Go terminal TUI for people who run
AI coding agents (Claude Code, Codex CLI) in the terminal. One screen:
projects in a workspace (left), agent sessions with their subagent tree per
project (middle), the selected agent's real interactive UI (right). Sibling
of bungkus-cli; same tooling, release pipeline and conventions. Status:
**proposal / pre-development** — no application code yet. Read the docs
before writing any.

## Docs (read in this order)

- `docs/PROPOSAL.md` — goals, non-goals, stage plan, open questions, risks
- `docs/ARCHITECTURE.md` — process model, how subagents are observed (PTY + hooks over a unix socket), adapter boundary, modes/keys, storage paths, concurrency rules, package layout, relationship with bungkus-cli, terminal compatibility
- `docs/DESIGN.md` — "Daun Pisang" design language: palette + contrast, glyphs, layout mockups, card states, keybindings, microcopy
- `docs/TECH_STACK.md` — every dependency and why
- `docs/CODING_RULES.md` — conventions, testing, review checklist, YAGNI rules
- `docs/SECURITY.md` — threat model and rules

## Tech Stack

- **Go 1.26** — single static binary, darwin/linux × arm64/amd64
- **Cobra** — `bungkus-mcc` (TUI), `hook`, `setup codex`, `update`
- **Bubble Tea v2 / Lip Gloss v2 / Bubbles v2** (`charm.land/*`) — TUI; versions match bungkus-cli
- **creack/pty + charmbracelet/x/vt** — agents run in a PTY, rendered by an embedded VT emulator
- stdlib for JSON config/state, unix socket IPC, slog

## Planned Structure

```
main.go, version.go          # Version via -ldflags (as bungkus-cli)
cmd/                         # root (TUI), hook, setup, update
internal/theme/              # Daun Pisang palette, icons, styles (shareable file)
internal/tui/                # tea.Model, keymap.go (single source of keys/help), panes, golden tests
internal/agent/              # Event, Adapter, state machine; claude.go, codex.go; testdata/ recorded hook payloads
internal/term/               # pty + vt emulator session
internal/ipc/                # unix socket server/client
internal/store/              # XDG paths, config.json, sessions.json
internal/workspace/          # project dir scan
```

## Key Decisions (don't relitigate without reading the docs)

- Live pane = PTY + VT emulator. Structure = agent hooks → `bungkus-mcc hook` → unix socket. **Never parse agent transcripts** (formats are vendor-internal).
- Claude hooks are injected per session with `--settings`; Codex hooks via one-time `setup codex` into `~/.codex/hooks.json`.
- Quitting mcc stops sessions (confirm dialog); they are resumable (`claude --resume` / `codex resume`). No daemon in stage 1.
- Modes: NORMAL (vim + arrows) and INTERACT (keys go to the agent); exit chord is `ctrl-\`. All keys live in `internal/tui/keymap.go`.
- Separate repo and binary from bungkus-cli; shared code is copied (theme, updater, installer, workflows), not a shared module, until a third product exists.
- Seven direct deps. Adding one requires a TECH_STACK.md entry.

## Build, Run, Test (once code exists)

```bash
go build -o bungkus-mcc .
go run .                      # TUI
go test ./...                 # includes teatest goldens; -update to regenerate
```

## Conventions

- Conventional commits (`feat:`, `fix:`, `test:`, `chore:`, `docs:`); semantic-release: `main` = canary, `release` = stable (merge commit, never squash the promotion PR)
- Branch naming: `i{issue#}-{date}-{seq}` (e.g. `i12-20261007-0930`)
- GitHub repo: `osbrjp/bungkus-mcc`
- Tests next to code, table-driven; views have goldens at 120×40 and 80×24 with `NO_COLOR=1 --icons ascii`
- No shell in `exec`; no raw agent bytes to stdout; no secrets persisted (see `docs/SECURITY.md`)
- Every state has glyph + word + color; every vim key has an arrow twin
