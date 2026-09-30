# bungkus-mcc

**Status: proposal / pre-development.** No application code yet; this repo
holds the design documents for review.

bungkus-mcc ("mission control") is a terminal panel for running AI coding
agents — Claude Code and Codex CLI — across the projects in a workspace.
Projects on the left, sessions and their subagents in the middle, the
selected agent's live interactive UI on the right. One Go binary, no daemon,
works in kitty, Ghostty, iTerm2, WezTerm, Alacritty, Terminal.app, tmux and
zellij.

It is a sub-product of [bungkus-cli](https://github.com/osbrjp/bungkus-cli)
and shares its tooling, release pipeline and the "Daun Pisang" design
language.

## Documents

| Doc | What |
|-----|------|
| [docs/PROPOSAL.md](docs/PROPOSAL.md) | Summary, goals, user stories, stage plan, open questions, risks |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | How it works: PTY + VT emulator for output, agent hooks over a unix socket for structure |
| [docs/DESIGN.md](docs/DESIGN.md) | Nasi-lemak palette with contrast ratios, glyphs, layout mockups, keys |
| [docs/TECH_STACK.md](docs/TECH_STACK.md) | Dependencies and rejected alternatives |
| [docs/CODING_RULES.md](docs/CODING_RULES.md) | Conventions, tests, review checklist |
| [docs/SECURITY.md](docs/SECURITY.md) | Threat model and rules |

## Planned install (not yet available)

```bash
curl -fsSL https://raw.githubusercontent.com/osbrjp/bungkus-mcc/main/install.sh | bash
```

Recommended font: any Nerd Font (optional — the default icon set needs only
a normal Unicode monospace font; `--icons ascii` needs nothing).
