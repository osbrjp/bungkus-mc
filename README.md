# bungkus-mc

<p align="center"><img src="docs/assets/mascot.gif" width="160" alt="the bungkus mascot: a green banana-leaf packet with tan paper corners, hopping"></p>

**Status: proposal / pre-development.** This repo holds the design
documents for review and the M0 foundation spike (`spikes/`). bungkus-mc
is written in **Rust** (decided on the spike's evidence); bungkus-cli
stays Go.

bungkus-mc ("mission control") is a terminal panel for running AI coding
agents — Claude Code and Codex CLI — across the projects in a workspace.
Projects on the left, sessions with their subagents and usage (tokens,
cost, context %) in the middle, the selected agent's live interactive UI
on the right, your plan's 5-hour / weekly limits in the status bar. One
small binary, no daemon; works in kitty, Ghostty, iTerm2, WezTerm,
Alacritty, Terminal.app, tmux, zellij and editor terminals.

It is a sub-product of [bungkus-cli](https://github.com/osbrjp/bungkus-cli)
and shares its tooling, release pipeline and the "Daun Pisang" design
language.

## Documents

| Doc | What |
|-----|------|
| [docs/PROPOSAL.md](docs/PROPOSAL.md) | Summary, goals, user stories, milestones, open questions, risks |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | PTY + VT emulator for output; agent hooks and status line over a unix socket for structure and usage |
| [docs/DESIGN.md](docs/DESIGN.md) | Daun Pisang design language: palette, glyphs, mascot, generated mockups, keys, notifications |
| [docs/TECH_STACK.md](docs/TECH_STACK.md) | Crates and rejected alternatives, toolchain, CI, release |
| [docs/CODING_RULES.md](docs/CODING_RULES.md) | Behaviour rules, tests, review checklist |
| [docs/SECURITY.md](docs/SECURITY.md) | Threat model and rules |
| [spikes/SPEC.md](spikes/SPEC.md) | The M0 spike: Go vs Rust prototypes of the embedded terminal pane, measured against tmux |
| [.claude/skills/rust-best-practices/SKILL.md](.claude/skills/rust-best-practices/SKILL.md) | Mandatory Rust style for every `.rs` / `Cargo.toml` change |

## Build (once code exists)

```bash
cargo build --release              # target/release/bungkus-mc (Rust stable 1.97, pinned)
cargo run -- ~/Works               # run with a workspace
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings \
  && cargo test --all-features && RUSTDOCFLAGS="-D warnings" cargo doc --no-deps \
  && cargo deny check                         # what CI runs
```

## Planned install (not yet available)

```bash
curl -fsSL https://raw.githubusercontent.com/osbrjp/bungkus-mc/main/install.sh | bash
bkmc        # short command; same as bungkus-mc
```

Notes for later users (from the design):

- Font: any monospace font works; the default icon set is ASCII, with
  `icons: "unicode"` or `"nerd"` as opt-ins.
- The right pane talks to the agent whenever it has focus; `ctrl-\` brings
  you back. If tmux's vim-tmux-navigator, VS Code, or a JIS/German keyboard
  takes that key, set `interactExit` in `~/.config/bungkus/mc/config.json`
  (suggested: `ctrl-^`).
- Only sessions started from bungkus-mc appear in it. A project is a
  folder in the workspace that contains `CLAUDE.md`, `AGENTS.md` or `.git`.
- Quitting stops the agents and the processes they started (dev servers);
  the quit dialog lists them first and lets you keep any of them
  (`space`); agents, multiplexers, Docker and app bundles are kept by
  default (`cleanup.keep` adds more). Sessions can be resumed later.
- Codex usage figures are read from Codex's own session log
  (`token_count` records only); Claude's come from its status line.
- Model routing via TypeSafe Jev is off by default and sends only the
  start prompt you type, after a consent dialog.
- Two instances on the same workspace work but share `sessions.json`
  (last writer wins).
