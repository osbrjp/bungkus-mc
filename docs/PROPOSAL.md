# bungkus-mcc — Proposal

Status: **proposal / pre-development**. Written 2026-09-30 for the product
owner. Companion documents: ARCHITECTURE.md (how), DESIGN.md (look and
keys), TECH_STACK.md (deps), CODING_RULES.md, SECURITY.md.

## 1. Executive summary

bungkus-mcc ("mission control") is a terminal panel for people who run AI
coding agents (Claude Code, Codex CLI) in the terminal. It shows, in one
screen, every project in a workspace, every agent session per project with
its subagents as a live tree, and the selected session's real interactive
UI — so you can watch three agents work, see which one is waiting for you,
and jump in to answer it without hunting through tabs.

Technical core (details in ARCHITECTURE.md): mcc runs each agent in a PTY it
owns and renders it with an embedded terminal emulator; it learns the
subagent tree and session state from the agents' own **hook** systems (both
Claude Code and Codex now ship `SubagentStart/Stop`, `PreToolUse`, `Stop`,
permission/notification hooks with the same JSON shape), delivered over a
per-process unix socket. It never parses the agents' transcript files, which
both vendors document as internal. One Go binary, no daemon, seven direct
dependencies, same build/release/install pipeline as bungkus-cli.

Stage 1 is deliberately small: one workspace, Claude Code + Codex, dark/light
theme, vim + arrow navigation, sessions stop when mcc quits but can be
resumed. Detach/persist, other agents, and multi-workspace are staged
behind real user demand.

## 2. Goals

1. See all running agent sessions across a workspace at a glance, with
   "needs you" impossible to miss.
2. See each session's subagents as they start, work and finish.
3. Interact with any session in place (the agent's own UI, not a re-skin).
4. Start a new session for a project in two keystrokes.
5. Work well in kitty, Ghostty, iTerm2, WezTerm, Alacritty, Terminal.app,
   tmux, zellij — degrading gracefully, never breaking.
6. Establish the nasi-lemak "Daun Pisang" visual language for all bungkus
   products.

## 3. Non-goals (stage 1)

- Being an agent client (own chat UI, own permission UI, structured
  streaming). We render the vendor's UI.
- Persisting sessions across mcc restarts (kill + resume instead; see
  ARCHITECTURE.md §3.1).
- Windows. Remote/SSH-hosted agents. Multiple workspaces at once.
- Parsing transcripts, cost/token dashboards, history search.
- Agents other than Claude Code and Codex (the adapter makes it cheap later;
  we don't build the third one speculatively).
- Telemetry of any kind. Network use beyond the daily update check.

## 4. User stories

- As a developer running three Claude sessions on three repos, I want one
  screen that shows which is working, which finished, and which is waiting
  for a permission answer, so I stop alt-tabbing.
- As a vibe coder, I want to pick a project from a list and press `n` to get
  an agent talking to it, without remembering `cd` paths or flags.
- As someone who uses subagents heavily, I want to see the subagent tree of
  a session (name, what tool it is on, done/not) while the main agent's
  screen stays visible.
- As a tmux user, I want mcc to work inside my tmux session with my keys,
  and I want to know exactly when my keystrokes go to the agent versus mcc.
- As a bungkus-cli user, I want "new project" in mcc to run the scaffolder I
  already know.
- As a Codex user, I want the same panel to work for Codex, even if the
  tree is thinner.
- As a user on Apple Terminal at 80×24, I want it to still be usable.

## 5. Stage plan

### Stage 1 — MVP (target: usable daily by the team)

| Milestone | Scope | Done when |
|-----------|-------|-----------|
| M1 Skeleton | repo, CI, release pipeline, `update`, theme package, three empty panes with layout/resize/modes/keymap + help, golden tests at 120×40 and 80×24 | `bungkus-mcc` installs via install.sh, renders, `?` shows generated help, `q` quits |
| M2 Workspace + projects | config/state files, `w` picker, project list, `N` runs `bungkus-cli` in the output pane | Pick a folder, see projects, scaffold one without leaving mcc |
| M3 Live pane | PTY + x/vt session, INTERACT mode with `ctrl-\`, resize, scrollback, `n` launches `claude`/`codex`, `x` stops, quit-with-running confirm | Run a full Claude Code session inside mcc on kitty, Ghostty, tmux |
| M4 Structure (Claude) | `bungkus-mcc hook`, socket server, Claude adapter via `--settings`, state machine, subagent tree, "needs you" badge | Cards update live during a real session with parallel subagents |
| M5 Structure (Codex) | Codex adapter, `setup codex`, trust hint, "output only" degrade | Same for Codex; untrusted-hooks path shows the hint, PTY still works |
| M6 Resume + polish | `sessions.json`, `r` resume, per-session event log, light theme, `--icons`, `NO_COLOR`, terminal matrix pass, README with fonts | Release `v0.1.0` on the `release` branch |

Order rationale: M3 before M4 because the PTY pane is the only piece with
real technical risk (emulator fidelity with the agents' TUIs); learn early.

### Stage 2 — candidates, each gated on a request

- Detached sessions via `claude --bg` / `claude attach` and Codex
  app-server; list sessions mcc did not start (`claude agents --json`).
- `bungkus-cli mcc` plugin shim (20 lines in bungkus-cli).
- Headless "task" sessions (`claude -p … stream-json`) for fire-and-forget
  jobs with a structured card and no PTY.
- History adapter (transcript tailing) for sessions launched outside mcc —
  only if the demand is there, since the formats are internal.
- Third agent (Gemini CLI, opencode) if a user asks and it has hooks.
- Extract `bungkus-kit` (theme + updater) when a third product appears.

## 6. Open questions for the product owner

1. **Quit behavior**: is "stop sessions on quit, resume later" acceptable for
   v0.1, or is detach a launch requirement? (Detach pushes M3 out by ~2
   milestones and adds a Codex JSON-RPC client.)
2. **Codex hook setup**: mcc must add hooks to `~/.codex/hooks.json` and the
   user must approve them once in Codex's `/hooks`. OK to ship with that
   one-time setup step, or is Codex stage-1 support "PTY only, no tree"?
3. **Workspace model**: one directory whose subfolders are projects — enough?
   Or do you want an explicit project list (add arbitrary paths)?
4. **Session start prompt**: should `n` always ask for an initial prompt, or
   drop straight into the agent's own prompt (recommended: optional, `enter`
   to skip)?
5. **Palette change in bungkus-cli**: do you want bungkus-cli to adopt Daun
   Pisang (greener green, legible error red) in its next release, or keep
   Lackluster until mcc ships?
6. **Name on screen**: `bungkus-mcc` vs `bungkus mission control` in the
   title bar; and is "wrapped" as the word for "done" acceptable, or too cute?
7. **Screen-reader / plain mode**: out of scope for v0.1 unless you say
   otherwise; Claude Code has `--ax-screen-reader`, a mcc equivalent is a
   line-based view (significant work).
8. **Icons default**: `unicode` (recommended) vs `nerd` (prettier, breaks
   for users without a patched font) vs `ascii`.
9. **License** for the new repo (bungkus-cli's LICENSE file is empty).

## 7. Risks

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| x/vt renders an agent's TUI wrongly (Claude Code and Codex use heavy full-screen redraws, kitty keyboard, sync output) | medium | high (core feature) | M3 first; the wrapper isolates the emulator; midterm as fallback; `ctrl-l`; users can always run the agent outside mcc |
| Hook payloads change in an agent release | medium | low | all fields optional, recorded-payload tests, unknown events ignored; card degrades to "output only" |
| Claude `--settings` hooks replace instead of merge with user hooks (ASSUMPTION) | low | medium (user's own hooks silently off in mcc sessions) | verify in M4; fallback: `setup claude` merges into `~/.claude/settings.json` like Codex |
| Codex hooks silently skipped when untrusted | high (first run) | low | detect (no `SessionStart` in 10 s), show hint; `setup codex` prints the exact approval step |
| Users expect detach and lose an in-flight turn on quit | medium | medium | explicit confirm dialog with count; resume in one key; stage-2 detach path documented |
| Two mcc instances on the same workspace | low | low | per-pid socket; sessions.json last-writer-wins (acceptable); note in README |
| Palette looks wrong on users' non-black terminal backgrounds | medium | low | never paint the bg; every state has glyph + word; contrast targets computed on reference bg and documented |
| Team bandwidth: TUI + PTY + IPC is more surface than bungkus-cli | — | — | seven deps, no daemon, no transcript parsing, YAGNI rules in CODING_RULES.md |
