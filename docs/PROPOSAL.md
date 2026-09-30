# bungkus-mcc — Proposal

Status: **proposal / pre-development**, revision 4 (product-owner
decisions of 2026-09-30 applied). Companion documents: ARCHITECTURE.md
(how), DESIGN.md (look and keys), TECH_STACK.md (deps), CODING_RULES.md,
SECURITY.md.

## 1. Executive summary

bungkus-mcc ("mission control") is a terminal panel for people who run AI
coding agents (Claude Code, Codex CLI) in the terminal. One screen: the
projects in a workspace, the selected project's agent sessions with their
subagents as a live list, the selected session's real interactive UI —
plus what it is costing you: tokens, cost, context fill and your plans'
5-hour / weekly limits. You watch three agents work, hear a bell when one
needs you, jump in with `!` and answer without hunting through tabs. When
you quit, mcc stops the agents *and* the dev servers they left behind, and
remembers every session so you can resume it.

Technical core (ARCHITECTURE.md): mcc runs each agent in a PTY it owns and
renders it with an embedded terminal emulator — focusing that pane is
talking to the agent. It learns session state and the subagent list from
the agents' own **hook** systems (verified live on both), and usage
figures from Claude Code's **status line** and, for Codex, from the
`token_count` records of Codex's own session log — the one documented
exception to "never parse transcripts". Optionally, off by default, it asks
TypeSafe's Jev which model tier a start prompt needs and launches the
agent with a cheaper model. One Go binary, no daemon, eight direct
dependencies, the same build/release/install pipeline as bungkus-cli,
released together with bungkus-cli's adoption of the shared Daun Pisang
palette.

## 2. Goals

1. See all running agent sessions across a workspace at a glance; "needs
   you" impossible to miss (colour + glyph + word + bell + terminal title).
2. See each session's subagents as they start, work and finish.
3. Interact with any session in place (the agent's own UI, not a re-skin).
4. Start a new session for a project in two keystrokes: `n` `enter` from
   the projects pane (last-used agent, no prompt).
5. See tokens, cost, context % per session and plan limits globally, for
   both agents.
6. Leave nothing running by accident: quitting stops the agents and the
   processes they started.
7. Work well in kitty, Ghostty, iTerm2, WezTerm, Alacritty, Terminal.app,
   tmux, zellij, VS Code/Cursor terminals — degrading, never breaking.
8. Establish the nasi-lemak "Daun Pisang" visual language for all bungkus
   products, shipped in bungkus-cli at the same time.

## 3. Non-goals (stage 1)

- Being an agent client (own chat UI, own permission UI). We render the
  vendor's UI.
- Persisting sessions across mcc restarts (stop + resume instead).
- Showing sessions that were not started from mcc (accepted for v0.1).
- Windows. Remote/SSH-hosted agents. Multiple workspaces at once.
- Parsing transcripts, except the approved Codex `token_count` reader.
- Per-subagent token counts. Nested subagent trees (flat list).
- Agents other than Claude Code and Codex.
- The `N` "new project via bungkus-cli" action (stage 2).
- Telemetry of any kind; network beyond the update check and, only when
  the user opts in, the routing request.

## 4. User stories

- As a developer running three Claude sessions on three repos, I want one
  screen showing which is working, which finished, which is waiting for a
  permission answer, and a bell when that happens, so I stop alt-tabbing.
- As a vibe coder, I want to pick a project and press `n` `enter` to get an
  agent talking to it, and `!` to get to whichever agent is waiting on me.
- As a heavy subagent user, I want the list of a session's subagents (what
  they were asked, done or not) while the main agent's screen stays visible.
- As someone on a Max plan and a Codex plan, I want both 5-hour / weekly
  usages in one bar, and how much context each session has left.
- As someone whose agents keep starting `vite` on :5173, I want quitting
  mcc to stop those too, and to see exactly what will be stopped first.
- As a tmux user (with vim-tmux-navigator), I want mcc to work inside tmux,
  to know exactly when keystrokes go to the agent, and to change the exit
  chord because `ctrl-\` is taken.
- As a cost-conscious user, I want easy tasks to start on a cheap model
  without thinking about it, and to see which model a session runs on.
- As a user on Apple Terminal at 80×24, I want it to still be usable.

## 5. Stage plan

### Stage 1

| Milestone | Scope | Done when |
|-----------|-------|-----------|
| M1 Skeleton | repo, CI (`gofmt`, test, govulncheck), release pipeline, `update`, theme package (painted + fallback sets, TrueColor gate, `background` config) with colour/width tests, mascot pixel map + static half-block/ASCII renderers, three empty panes with layout/breakpoints/modes/keymap + generated help, goldens at 120×40 and 80×24 | `bungkus-mcc` installs via install.sh, renders green at TrueColor and plain at 256, `?` shows generated help, `q` quits |
| M2 Workspace + projects | config/state files, first-run screen, workspace by CLI arg/config/text input, project list = child folders with `CLAUDE.md`/`AGENTS.md`/`.git`, sanitised names | pick a folder, see projects |
| M3 Live pane | PTY + x/vt session with reply pump, key translation table (incl. `shift-enter`; full passthrough incl. `esc`/`tab`/arrows/ctrl), sanitiser allowlist + hostile corpus, wheel scrollback, resize-all, INTERACT-on-focus with configurable exit chord, `n` launches `claude`/`codex` with `--` and `--name`, `x` stops, quit confirm; tests: DSR/DA feed returns, hostile streams, CJK/emoji width; decide Codex `--no-alt-screen`; JIS/German chord check; confirm interactive `claude`/`codex` accept `--` before a dash-leading prompt | a full Claude Code session runs inside mcc on kitty, Ghostty, tmux (+navigator), VS Code, including answering a permission prompt via passthrough |
| M4 Structure + notifications (Claude) | `bungkus-mcc hook` (silent, trimmed, quoted path), socket server, Claude adapter via `--settings` (new + resume argv, unit-tested), decided state machine with `background_tasks`, subagent list, sidebar precedence, `!` across projects, session names (`--name`, `session_title`, verify `/rename` → `session_name`), `notify` bell/desktop/off + OSC 2 title push/pop | cards and sidebar update live during a real session with parallel subagents; bell rings on needs-you; card titles follow renames |
| M5 Usage (Claude) | `bungkus-mcc statusline` wrapper (200 ms concurrent forward, then the user's status line under `sh` with buffered stdin; resolver with `CLAUDE_CONFIG_DIR`, recursion guard), `Usage` messages, compact card line, expanded selected card (sessions pane focused), getah-bar limits with thresholds and stale dimming; record a multi-turn session to settle `total_input_tokens` semantics and check which shell Claude uses | tokens/cost/ctx on every Claude card; limits in the bar on a subscription account; `-` on API-key accounts; the user's own status line still renders |
| M6 Codex | verify `/hooks` trust persistence for `-c` injected hooks (delete or build `setup codex`), record real Codex SubagentStart/PreToolUse payloads, Codex adapter, resume rules, "hooks off" hints; **Codex usage reader** (`codexusage.go`: validated path under `CODEX_HOME`, tail-only 256 KiB, `token_count` only, tolerant, fixtures) feeding cards and the `X` limits | Codex session with subagents shows structure and `312k tok · - · ctx 22%`; untrusted path degrades to output only; bad/missing rollout shows `-` |
| M7 Descendant tracking + cleanup | `internal/proc`: 2 s process-tree scan (Linux `/proc`, macOS `ps`), pid + start-time identity, port annotation (Linux `/proc/net/tcp`, macOS `lsof`), quit/`x` dialog listing, SIGTERM → 3 s → SIGKILL; fixture tests per OS, identity-mismatch refusal, kill order, missing-`lsof` path | a session that started `vite` is quit; the dialog shows `vite :5173 pid …`; the port is free afterwards; a reused pid is never signalled (test) |
| M8 Resume + polish | `sessions.json`, `r`/`d` with confirms, light theme, `--icons unicode|nerd`, `NO_COLOR`, non-UTF-8 locale, mascot animation in the empty state (tick only while visible), full terminal/keyboard matrix pass, README (exit-chord alternates, manual hook removal) | release **v0.1.0** on the `release` branch, together with bungkus-cli's Daun Pisang release |
| M9 Model routing (opt-in) | `internal/route` (net/http to TypeSafe, one Choice question, tier→model map from config, 1.5 s budget, fallback), API key from env/keychain command, consent dialog, picker `model` row, card `model` line, `--model`/`-m` in Launch; `httptest` fake-server tests | release **v0.2.0**; a routed session shows `haiku · routed 0.82`; the API down → default model, no error |

M9 is in stage 1 because it is ~200 isolated lines (one package, one
picker row, one argv flag) and it is opt-in; it does not hold v0.1.0.

### Stage 2 — candidates, each gated on a request

- Detached sessions via `claude --bg`/`attach` and the Codex app-server;
  listing sessions mcc did not start (`claude agents --json`).
- Codex thread names (extend the rollout reader once the record type is
  confirmed, or app-server).
- `N` new project: `bungkus-cli`'s wizard in the output pane; the
  `bungkus-cli mcc` PATH-exec shim.
- Headless "task" sessions with structured cards and per-turn usage.
- History adapter (transcript tailing) — only with demand.
- Nested subagent tree; per-subagent tokens (needs transcripts).
- Routing beyond the start prompt (e.g. re-routing on resume by reading the
  agent's own summary) — needs data mcc does not have without transcripts.
- Third agent, if it has hooks. `bungkus-kit` when a third product appears.

## 6. Decided by the product owner (2026-09-30)

1. **Quit:** sessions end on quit; all session ids are recorded for
   resume; mcc also stops the processes it observed as descendants of its
   agents (dev servers etc.) — tracked by a periodic process-tree scan,
   identified by pid + start time, never killed merely for holding a port,
   listed in the confirm dialog, SIGTERM then SIGKILL after a grace period
   (ARCHITECTURE.md §3.2–3.3, SECURITY.md, M7).
2. **Workspace:** direct child folders are projects only if they contain
   `CLAUDE.md`, `AGENTS.md` or `.git`; dot-dirs skipped, symlinks followed.
3. **Sessions started outside mcc are invisible** in v0.1 — accepted.
4. **Codex usage:** approved to read Codex's own session log, limited to
   the rollout `token_count` records at the path Codex's hook reports —
   the one contained transcript exception (ARCHITECTURE.md §6.3, M6).
5. **Default icon set = ASCII.** Borders stay box-drawing when the locale is
   UTF-8; ASCII borders only when it is not.
6. **Palette:** bungkus-cli adopts Daun Pisang now, released together with
   mcc; the owner implements the bungkus-cli side.
7. **Middle pane** shows the selected project's sessions only; the sidebar
   badges show other projects' state; `!` jumps across projects.
8. **Right pane is interactive on focus:** focusing it is INTERACT (full
   passthrough including `esc`/`tab`/arrows/ctrl); `ctrl-\` (configurable)
   returns to the sessions pane; no output-pane NORMAL mode, no `i` key.
9. **Card title = the session's own name** (Claude `--name`,
   `session_name`/`session_title`; prompt then `untitled` as fallbacks);
   the `n` picker gets a name field prefilled from the prompt.
10. **Model routing with TypeSafe Jev**, opt-in and off by default, scoped
    to the start prompt (M9, ARCHITECTURE.md §13).
11. **mcc paints a low-saturation green background** ("Daun Teduh"
    `#1c2a21`, body text ~11:1; light "Santan" `#f0f3d8`) when the terminal
    is TrueColor; otherwise the terminal's own bg/fg. Config `background:
    paint | terminal`. The output-pane emulator uses the same colours and
    answers OSC 11 with them. bungkus-cli keeps "never paint" for now
    (possible follow-up).
12. **Mascot:** the banana-leaf packet character (Figma
    `HwlCHEFqRm9hfOfUbtuL4h` node `17:3`; `docs/assets/mascot.svg`, `.gif`,
    generator `mascot-gif.py`) replaces the logomark; a 16×14 half-block
    sprite (idle/blink/hop/stepL/stepR, 350 ms sequence) animates only in
    the output pane's empty state, static under `NO_COLOR` or
    `motion: false`, ASCII triangle without half-blocks; cross-eyed variant
    proposed for the failed empty state and error dialog; fixed brand
    colours, never drawn over agent output.

Closed earlier: `--settings` hooks merge with user hooks (verified); Codex
hook injection per launch via `-c` (verified); the start prompt is optional
with the last agent preselected.

## 7. Open questions for the product owner

1. **License** for the new repo (bungkus-cli's LICENSE file is empty).
2. **Plain / screen-reader mode:** out of scope for v0.1 unless required
   (Claude Code has `--ax-screen-reader`; a mcc equivalent is a line-based
   view, significant work).
3. **Title-bar name:** `bungkus-mcc` or `bungkus mission control`?
4. **"wrapped"** as the word for a finished session — keep, or plain "done"?
5. **Routing tiers per agent:** Claude `quick → haiku`, `standard → sonnet`,
   `deep → opus` is the proposed default; exact model ids per tier?
6. **Route Codex too?** Codex's `-m` accepts model ids, but the tier map is
   empty by default (no obvious cheap/standard/deep triple); provide one,
   or Claude-only for M9?
7. **Jev cost vs tokens saved:** a Jev call is ~$0.00002 per routed start
   (≤ 4 KiB prompt at $0.042/Mtok), negligible; the real trade is
   quality-on-misroute vs cheaper sessions. Which `minConfidence` (0.6
   proposed) and should a fallback default to the *cheaper* or the
   *default* model?
8. **Codex thread names:** extend the approved rollout reader to the
   thread-name record once its type is confirmed, or leave Codex titles to
   mcc's own name/prompt?

## 8. Risks

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| x/vt renders an agent's TUI wrongly | medium | high | M3 first; wrapper isolates the emulator; midterm as fallback; `ctrl-l`/`R` |
| Accidental keystrokes to the agent (INTERACT on focus) | medium | medium | four instant signals; `(ctrl-\ back)` in the sessions-pane hint; INTERACT ends when the process exits |
| Key translation misses a key the agent needs | medium | medium | table + both-direction tests; `shift-enter` verified in M3 |
| Cleanup kills the wrong process | low | high | descendants observed by scan only; pid + start-time identity re-checked before every signal; never by port; dialog lists everything first; fixture tests per OS |
| Cleanup misses a daemonised process (forked between scans) | low | low | 2 s interval; `// ponytail:` note; shorten if seen |
| Hook / statusLine / rollout schema changes | medium | low | all fields optional; recorded fixtures; unknown → ignored / `-` |
| Codex hook trust does not persist for injected hooks | medium | low | M6 first task; `setup codex` fallback fully specified |
| Routing misroutes a hard task to a small model | medium | medium | opt-in; confidence threshold; card shows the model; override in picker; `/model` inside the agent |
| Jev API unavailable / key missing | medium | none | 1.5 s budget, silent fallback to the default model |
| CJK ambiguous-width terminals shift columns | medium (JP team) | medium | ascii default glyphs; narrow unicode set; recorded stream test |
| `ctrl-\` clashes (navigator, VS Code, JIS/German) | medium | low | configurable `interactExit`; documented alternates; tested keyboards |
| Two mcc instances on one workspace | low | low | per-pid socket; `sessions.json` last-writer-wins; README note |
| Team bandwidth: TUI + PTY + IPC + proc | — | — | eight deps, no daemon, one contained transcript reader, YAGNI rules |
