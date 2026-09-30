# bungkus-mcc — Proposal

Status: **proposal / pre-development**, revision 2 (after critic round 1).
Written 2026-09-30 for the product owner. Companion documents:
ARCHITECTURE.md (how), DESIGN.md (look and keys), TECH_STACK.md (deps),
CODING_RULES.md, SECURITY.md.

## 1. Executive summary

bungkus-mcc ("mission control") is a terminal panel for people who run AI
coding agents (Claude Code, Codex CLI) in the terminal. One screen: every
project in a workspace, every agent session per project with its subagents
as a live list, the selected session's real interactive UI — plus what it
is costing you: tokens, cost, context fill and your plan's 5-hour / weekly
limits. You can watch three agents work, hear a bell when one needs you,
jump in and answer without hunting through tabs.

Technical core (ARCHITECTURE.md): mcc runs each agent in a PTY it owns and
renders it with an embedded terminal emulator; it learns session state and
the subagent list from the agents' own **hook** systems (both Claude Code
and Codex ship `SubagentStart/Stop`, `PreToolUse`, `Stop`, permission
hooks with the same JSON shape — verified live), and usage figures from
Claude Code's **status-line** extension point, all delivered over a
per-process unix socket. It never parses the agents' transcript files,
which both vendors document as internal. One Go binary, no daemon, seven
direct dependencies, the same build/release/install pipeline as bungkus-cli.

Stage 1 is deliberately small: one workspace, Claude Code + Codex,
dark/light theme, vim + arrow navigation, bell notifications, sessions stop
when mcc quits but can be resumed. Detach/persist, Codex usage figures,
other agents and the bungkus-cli "new project" integration are staged
behind real demand.

## 2. Goals

1. See all running agent sessions across a workspace at a glance; "needs
   you" impossible to miss (colour + glyph + word + bell + terminal title).
2. See each session's subagents as they start, work and finish.
3. Interact with any session in place (the agent's own UI, not a re-skin).
4. Start a new session for a project in two keystrokes (`n` `enter`).
5. See tokens, cost, context % per session and plan limits globally.
6. Work well in kitty, Ghostty, iTerm2, WezTerm, Alacritty, Terminal.app,
   tmux, zellij, VS Code/Cursor terminals — degrading, never breaking.
7. Establish the nasi-lemak "Daun Pisang" visual language for all bungkus
   products.

## 3. Non-goals (stage 1)

- Being an agent client (own chat UI, own permission UI). We render the
  vendor's UI.
- Persisting sessions across mcc restarts (stop + resume instead).
- Showing sessions that were not started from mcc.
- Windows. Remote/SSH-hosted agents. Multiple workspaces at once.
- Parsing transcripts (a single, gated exception for Codex usage is
  *proposed* in §6, not planned).
- Per-subagent token counts (transcript-only data).
- Nested subagent trees (flat list).
- Agents other than Claude Code and Codex.
- The `N` "new project via bungkus-cli" action (stage 2).
- Telemetry of any kind; network beyond the daily update check.

## 4. User stories

- As a developer running three Claude sessions on three repos, I want one
  screen showing which is working, which finished, which is waiting for a
  permission answer, and a bell when that happens, so I stop alt-tabbing.
- As a vibe coder, I want to pick a project and press `n` `enter` to get an
  agent talking to it.
- As a heavy subagent user, I want the list of a session's subagents (what
  they were asked, done or not) while the main agent's screen stays visible.
- As someone on a Max plan, I want to see my 5-hour and weekly usage without
  typing `/usage`, and how much context each session has left before it
  compacts.
- As a tmux user (with vim-tmux-navigator), I want mcc to work inside tmux,
  to know exactly when keystrokes go to the agent, and to change the exit
  chord because `ctrl-\` is taken.
- As a Codex user, I want the same panel for Codex, even if the card is thinner.
- As a user on Apple Terminal at 80×24, I want it to still be usable.

## 5. Stage plan

### Stage 1 — MVP

| Milestone | Scope | Done when |
|-----------|-------|-----------|
| M1 Skeleton | repo, CI (`gofmt`, test, govulncheck), release pipeline, `update`, theme package with colour tests, three empty panes with layout/breakpoints/modes/keymap + generated help, goldens at 120×40 and 80×24 | `bungkus-mcc` installs via install.sh, renders, `?` shows generated help, `q` quits |
| M2 Workspace + projects | config/state files, first-run screen, workspace by CLI arg/config/text input, project list with sanitised names | pick a folder, see projects |
| M3 Live pane | PTY + x/vt session with reply pump, key translation table (incl. `shift-enter`), sanitiser allowlist + hostile corpus, scrollback, resize-all, INTERACT with configurable exit chord, `n` launches `claude`/`codex` with `--`, `x` stops, quit-with-running confirm; tests: DSR/DA feed returns, hostile streams, CJK/emoji width; decide Codex `--no-alt-screen`; JIS/German chord check | a full Claude Code session runs inside mcc on kitty, Ghostty, tmux (+navigator), VS Code |
| M4 Structure + notifications (Claude) | `bungkus-mcc hook` (silent, trimmed, quoted path), socket server, Claude adapter via `--settings` (new + resume argv, unit-tested), decided state machine with `background_tasks`, subagent list, sidebar precedence, `notify` bell/desktop/off + OSC 2 title, `!` jump | cards and sidebar update live during a real session with parallel subagents; bell rings on needs-you |
| M5 Usage (Claude) | `bungkus-mcc statusline` wrapper (forward + exec the user's status line), `Usage` messages, compact card line, expanded selected card, getah-bar limits with thresholds | tokens/cost/ctx on every Claude card; limits in the bar on a subscription account; `—` on API-key accounts; the user's own status line still renders |
| M6 Codex | verify `/hooks` trust persistence for `-c` injected hooks (delete or build `setup codex` accordingly), record real Codex SubagentStart/PreToolUse payloads into testdata, Codex adapter, resume rules, "hooks off" hints | Codex session with subagents shows structure; untrusted path degrades to output only |
| M7 Resume + polish | `sessions.json`, `r`/`d` with confirms, light theme, `--icons`, `NO_COLOR`, full terminal/keyboard matrix pass, README (fonts, exit-chord alternates, manual hook removal) | release `v0.1.0` on the `release` branch |

M3 comes before M4 because the PTY pane is the piece with real technical
risk (emulator fidelity with the agents' TUIs, key translation).

### Stage 2 — candidates, each gated on a request

- Detached sessions via `claude --bg`/`attach` and the Codex app-server;
  listing sessions mcc did not start (`claude agents --json`).
- Codex usage figures via the app-server protocol (or the gated rollout
  exception, §6).
- `N` new project: `bungkus-cli`'s wizard in the output pane (zero
  coupling); the `bungkus-cli mcc` PATH-exec shim (20 lines in bungkus-cli).
- Headless "task" sessions (`claude -p … stream-json`, `codex exec --json`)
  with structured cards and per-turn usage.
- History adapter (transcript tailing) — only with demand; formats are internal.
- Nested subagent tree; per-subagent tokens (needs transcripts).
- Third agent, if it has hooks.
- Extract `bungkus-kit` when a third product appears.

## 6. Open questions for the product owner

1. **Detach vs stop-on-quit.** v0.1 stops sessions on quit (confirm dialog)
   and resumes them with one key. Is that acceptable, or is detach a launch
   requirement (+2 milestones, Codex JSON-RPC client)?
2. **Workspace model.** One directory whose direct child folders are the
   projects — enough, or an explicit list of arbitrary paths?
3. **Sessions started outside mcc are invisible** in v0.1 (only sessions
   launched from mcc have the hooks). OK for v0.1?
4. **Codex usage figures.** Stage 1 shows `—` for Codex (Codex's own status
   line is visible in the pane). Options: (a) accept `—`; (b) approve the
   contained transcript exception (ARCHITECTURE.md §6.1: tail only
   `token_count` lines of the file Codex's own hook names, off by default,
   internal format); (c) wait for the app-server client in stage 2.
5. **Default icon set:** `unicode` (recommended) vs `nerd` vs `ascii`.
6. **bungkus-cli palette adoption timing:** next bungkus-cli release, or
   after mcc ships?
7. **License** for the new repo (bungkus-cli's LICENSE file is empty).
8. **Plain / screen-reader mode:** out of scope for v0.1 unless required
   (Claude Code has `--ax-screen-reader`; a mcc equivalent is a line-based
   view, significant work).
9. **Title-bar name:** `bungkus-mcc` or `bungkus mission control`?
10. **"wrapped"** as the word for a finished session — keep, or plain "done"?

Closed since round 1: `--settings` hooks merge with user hooks (verified);
Codex hook injection per launch works via `-c` (verified); the session
start prompt is optional with the last agent preselected (decided).

## 7. Risks

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| x/vt renders an agent's TUI wrongly (full-screen redraws, sync output, alt screen) | medium | high | M3 first; wrapper isolates the emulator; midterm as fallback; `ctrl-l`/`R`; the agent runs fine outside mcc |
| Key translation misses a key the agent needs (kitty-only combos) | medium | medium | table + tests; `shift-enter` verified in M3; INTERACT hint lists unsupported keys |
| Hook payloads change in an agent release | medium | low | all fields optional; recorded payloads in testdata; unknown events ignored; "output only" degrade |
| statusLine schema changes | low | low | fields optional; usage shows `—` |
| Codex hook trust does not persist for injected hooks | medium | low | M6 first task; `setup codex` fallback fully specified |
| Users expect detach; lose an in-flight turn on quit | medium | medium | explicit confirm with count; one-key resume; open question 1 |
| CJK ambiguous-width terminals shift columns | medium (JP team) | medium | narrow glyphs only; recorded CJK/emoji stream test; `--icons ascii` |
| `ctrl-\` clashes (vim-tmux-navigator, VS Code, JIS/German) | medium | low | configurable `interactExit`; documented alternates; tested keyboards |
| Two mcc instances on one workspace | low | low | per-pid socket; `sessions.json` last-writer-wins; README note |
| Palette on non-black backgrounds | medium | low | never paint bg; `fg` = terminal default; ratios checked on five common themes |
| Team bandwidth: TUI + PTY + IPC | — | — | seven deps, no daemon, no transcript parsing, YAGNI rules |
