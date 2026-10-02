# CLAUDE.md

## Project Overview

**bungkus-mc** ("mission control") is a **Rust** terminal TUI for people
who run AI coding agents (Claude Code, Codex CLI) in the terminal. One
screen: projects in a workspace (left), the selected project's agent
sessions with their subagents and usage figures (middle), the selected
agent's real interactive UI (right), plan limits in the status bar.
Sibling of bungkus-cli (Go); same release pipeline, installer and
conventions; the palette is shared as a token spec. Status: **M1–M8 built**
(v0.1.0 candidate; M9 routing not started). Read the docs before changing
behaviour.

## Docs (read in this order)

- `docs/PROPOSAL.md` — goals, non-goals, stage plan (M0 done, M1–M9), decided list, open questions, risks
- `docs/ARCHITECTURE.md` — process model, threads and the event loop, how subagents and usage are observed (PTY + hooks + status line over a unix socket), adapter boundary, state machine, modes/keys, storage, crate layout, bungkus-cli relationship, terminal compatibility, routing
- `docs/DESIGN.md` — "Daun Pisang" design language: palette token spec with contrast + declared 256/16 values, glyphs, mascot, generated mockups, components, states, keybindings, notifications, microcopy
- `docs/TECH_STACK.md` — every crate and why, toolchain, CI, release
- `docs/CODING_RULES.md` — behaviour rules, tests, review checklist, YAGNI rules
- `docs/SECURITY.md` — threat model and rules
- `.claude/skills/rust-best-practices/SKILL.md` — **mandatory for every `.rs` / `Cargo.toml` change**: lints, doc comments, API style, errors, concurrency, `unsafe`, tests, dependencies
- `spikes/SPEC.md`, `spikes/rust/README.md` — the M0 prototype; `src/term/keys.rs` starts from `spikes/rust/src/keys.rs`

## Tech Stack

- **Rust stable 1.97** (`rust-toolchain.toml`), edition 2024, one binary crate; darwin/linux × arm64/amd64
- **ratatui** (UI; its re-exported `ratatui::crossterm` for input, raw mode, kitty keyboard flags on the host side)
- **alacritty_terminal 0.26** (embedded emulator) + **portable-pty**
- `thiserror`/`anyhow` (anyhow only in `main.rs` and top-level subcommand entry points), `serde`/`serde_json`, `uuid`, `lexopt`, `ureq` (rustls, sync), `semver`, `rustix` — **12 crates**, `Cargo.lock` committed; no logging crate (`debug_log!` macro)
- No async runtime; threads + `std::sync::mpsc`; one PTY writer thread per session

## Module layout

```
src/main.rs        # lexopt → subcommand (TUI, hook, statusline, update); anyhow only here; panic hook restores the terminal
src/app/           # event loop + Model (Elm-style): mode/focus, sessions, dirty flag, ticks, AppEvent
src/ui/            # panes, dialogs, first run, keymap.rs, theme.rs (token spec), mascot.rs, string sanitise
src/term/          # session.rs (PTY + Term + reader/waiter threads), keys.rs (encoder), query replies
src/agent/         # Event/Usage/Adapter, claude.rs, codex.rs, codex_usage.rs (the one transcript reader), mcp.rs (MCP server names from the agents' config); testdata/
src/ipc/           # unix socket server; hook.rs / statusline.rs subcommands
src/proc/          # descendant scan (/proc + pidfd, ps), lsof ports, kill + keep rule
src/route/         # TypeSafe Jev tier → model id (opt-in)
src/store/         # XDG paths, config.json, sessions.json, consent.json
src/update/        # release check + `update` (port of bungkus-cli's updater)
src/workspace.rs   # project dir scan
```

## Key Decisions (don't relitigate without reading the docs)

- **Rust, not Go** (owner, M0 spike: 12× faster on heavy output, 1.2 MB binary, a maintained emulator crate on crates.io). bungkus-cli stays Go.
- Live pane = PTY + `alacritty_terminal` advanced on the UI thread (M3 re-times the flood cases; pump-thread fallback); per session a bounded reader thread, a **writer thread** that owns the PTY writer (the UI thread only sends), and a waiter (exit on reader EOF or 500 ms after `wait`); query replies (DSR/DA/OSC 10/11 from the theme, CSI 14 t; 18 t is alacritty's); `kitty_keyboard: true` + our own encoder (`term/keys.rs`) incl. kitty CSI-u toward the agent (M3); sync-update deadline + `stop_sync`; mouse forwarded when the agent enabled it; EIO after child exit ignored. **Focusing the output pane is INTERACT** (full passthrough); `ctrl-\` returns to the sessions pane.
- Structure = agent hooks → `bungkus-mc hook` → unix socket. Usage = Claude status line → `bungkus-mc statusline` (forwards within 200 ms, then runs the user's own status line under `sh`). **Never parse agent transcripts — except `agent/codex_usage.rs`** (`token_count` records only, owner-approved).
- MCP servers on a card = the ones in use: the `mcp__<server>__` tool names in hook events. The selected card also lists, as `mcp idle`, the unused names from the agents' config files (`agent/mcp.rs`: `.mcp.json`, `.claude.json`, Codex `config.toml`; streamed, names only, never commands or env). Config re-read off the UI thread every 5 s, and only when a file's mtime or size changed. Nerd set: logo, or kind glyph / plug + name (tables in `ui/icons.rs`, code points from Nerd Fonts' `glyphnames.json`); other sets: names.
- Issue / pull request links (`app/links.rs`, issue #188): read through the user's `gh` CLI (fixed argv, read-only, no token in mc) per session folder, on a branch change and every 60 s for running sessions; the issue comes from the `i{issue#}-…` branch name, else the PR's first closing issue. `P` / `I` open them in the browser, `i` lists the repository's open ones (`enter` reads one's description and comments in the popup, Markdown rendered by `app/markdown.rs`, no crate; `o` opens it); only plain `https://` URLs reach the opener.
- Render on a dirty flag or the 350 ms animation tick, never on a fixed timer.
- Child env: `TERM`/`COLORTERM` set; host-terminal identity vars, `TYPESAFE_API_KEY` and the Claude/Codex **session-marker** vars (`CLAUDECODE`, `CLAUDE_CODE_CHILD_SESSION`, …, see ARCHITECTURE §3.1) unset; `CLAUDE_CONFIG_DIR`/`CODEX_HOME` kept.
- Quit and `x` also stop observed descendants (dev servers): 2 s scan, uid filter, pid + start-time identity (pidfd on Linux), `[stop]/[keep]` dialog with a default-keep list, SIGTERM → 3 s → SIGKILL; never by port; a user stop yields `stopped`.
- States: running / your turn / needs you / failed / wrapped / stopped; `background_tasks` authoritative; `idle_prompt` ignored; sidebar badge precedence failed > needs you > your turn, spinner for working.
- Middle pane = selected project's sessions; `!` jumps across projects. Card title = the session's own name. Projects = child folders containing `CLAUDE.md`, `AGENTS.md` or `.git`.
- Palette: painted low-saturation green at TrueColor only (`background: paint`), terminal bg/fg + declared `Color::Indexed` values otherwise; the DESIGN §2 tables are the spec shared with bungkus-cli, guarded by a token-table test in each repo. Icon set `auto`: Nerd Font glyphs when one is installed, else ASCII; every state is glyph + word + colour.
- Mascot (banana-leaf packet, Figma poses): 16×14 / 8×6 half-block sprites, died for anything failed; never over agent output. Screens ≥ 120×36: a 3-row strip at the top of the output pane with the mini mascot at its right (mood of the selected session; click → hop + random quote bubble); no session: the big empty-state mascot. Smaller: corner of the output pane by mood and busyness.
- Routing (TypeSafe Jev) opt-in, `n` start prompt only, secret-shape guard, consent in `consent.json`, key from env or a once-per-process command; M9 / v0.2.0.
- No daemon, no async runtime, no `unsafe` (rustix; no `pre_exec`/`setsid` — child processes use `process_group(0)`), 12 crates. Adding one requires a TECH_STACK.md entry.

## Build, Run, Test

```bash
cargo build --release                 # target/release/bungkus-mc
cargo run -- ~/Works                  # TUI with a workspace
cargo run -- hook < payload.json      # the silent hook subcommand
UPDATE_GOLDEN=1 cargo test            # regenerate rendering goldens on purpose
```

CI (all must pass with no warnings):

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo deny check
```

## Conventions

- Conventional commits (`feat:`, `fix:`, `test:`, `chore:`, `docs:`); `main` = canary, `release` = stable via `git-pr-release` release PRs; bump `version` in `Cargo.toml` in the release PR (merge commit, never squash the promotion PR)
- Branch naming: `i{issue#}-{date}-{seq}` (e.g. `i12-20261007-0930`)
- Licence: proprietary (`LICENSE`, `publish = false`); never add code under a non-permissive licence.
- GitHub repo: `osbrjp/bungkus-mc`; binary `bungkus-mc`, short command `bkmc`
- Tests in `#[cfg(test)]` modules next to the code, table-driven; recorded hook payloads and `ps` output in `testdata/`; goldens are plain text + cursor at 120×40 and 80×24 with `NO_COLOR=1` and the ascii default
- Doc comments carry the documentation (skill §2); no agent chatter in comments
- No shell in `Command` for anything mc decides; no raw agent bytes to stdout; no secrets persisted; no `unsafe` (see `docs/SECURITY.md`)
