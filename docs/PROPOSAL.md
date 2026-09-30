# bungkus-mc — Proposal

Status: **proposal / pre-development** (product-owner decisions of
2026-09-30 applied). Companion documents: ARCHITECTURE.md
(how), DESIGN.md (look and keys), TECH_STACK.md (deps), CODING_RULES.md,
SECURITY.md.

## 1. Executive summary

bungkus-mc ("mission control") is a terminal panel for people who run AI
coding agents (Claude Code, Codex CLI) in the terminal. One screen: the
projects in a workspace, the selected project's agent sessions with their
subagents as a live list, the selected session's real interactive UI —
plus what it is costing you: tokens, cost, context fill and your plans'
5-hour / weekly limits. You watch three agents work, hear a bell when one
needs you, jump in with `!` and answer without hunting through tabs. When
you quit, mc stops the agents *and* the dev servers they left behind, and
remembers every session so you can resume it.

Technical core (ARCHITECTURE.md): **written in Rust** (owner decision on
the M0 spike, §6 item 15), mc runs each agent in a PTY it owns and
renders it with an embedded terminal emulator (`alacritty_terminal`) —
focusing that pane is talking to the agent. It learns session state and the subagent list from
the agents' own **hook** systems (verified live on both), and usage
figures from Claude Code's **status line** and, for Codex, from the
`token_count` records of Codex's own session log — the one documented
exception to "never parse transcripts". Optionally, off by default, it asks
TypeSafe's Jev which model tier a start prompt needs and launches the
agent with a cheaper model. One Rust binary (~1.2 MB), no daemon, no
async runtime, 12 crates pinned by `Cargo.lock`, the same release/install pipeline as
bungkus-cli (which stays Go), released together with bungkus-cli's
adoption of the shared Daun Pisang palette.

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
- Persisting sessions across mc restarts (stop + resume instead).
- Showing sessions that were not started from mc (accepted for v0.1).
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
  mc to stop those too, and to see exactly what will be stopped first.
- As a tmux user (with vim-tmux-navigator), I want mc to work inside tmux,
  to know exactly when keystrokes go to the agent, and to change the exit
  chord because `ctrl-\` is taken.
- As a cost-conscious user, I want easy tasks to start on a cheap model
  without thinking about it, and to see which model a session runs on.
- As a user on Apple Terminal at 80×24, I want it to still be usable.

## 5. Stage plan

### Stage 1

| Milestone | Scope | Done when |
|-----------|-------|-----------|
| **M0 Foundation spike — done** | Two prototypes of the embedded terminal pane built to `spikes/SPEC.md` (Go: Bubble Tea v2 + x/vt + creack/pty; Rust: ratatui + crossterm + portable-pty + alacritty_terminal 0.26), a headless case harness and `spikes/compare.py` scoring both against a tmux reference | Both matched tmux on every case; the numbers below decided the language (§6 item 15) |
| M1 Skeleton | crate, `rust-toolchain.toml`, lint set from the skill, CI (`fmt`, `clippy -D warnings`, `test`, `doc -D warnings`, `cargo deny check` with `deny.toml` targets = the four unix triples), release pipeline (native runners or `cargo-zigbuild`, `checksums.txt`, reused `install.sh`), `update` ported from bungkus-cli, `ui/theme.rs` implementing the token spec (painted + fallback sets, TrueColor gate, `background` config) with the token-table and contrast tests, mascot pixel maps + static half-block/ASCII renderers, three empty panes with layout/breakpoints/modes/keymap + generated help, goldens at 120×40 and 80×24 | `bungkus-mc` installs via install.sh, renders green at TrueColor and plain at 256, `?` shows generated help, `q` quits |
| M2 Workspace + projects | config/state files, first-run screen, workspace by CLI arg/config/text input, project list = child folders with `CLAUDE.md`/`AGENTS.md`/`.git`, sanitised names | pick a folder, see projects |
| M3 Live pane | `term/session.rs` from the spike: PTY + `alacritty_terminal` advanced on the UI thread (flood cases re-timed interactively; pump-thread fallback if needed), reader (bounded) / **writer** (owns the PTY writer, unbounded channel) / waiter threads, key encoder (`keys.rs` from the spike + **kitty CSI-u output toward the agent, ~80 lines**, with `Config { kitty_keyboard: true }`), full passthrough incl. `esc`/`tab`/arrows/ctrl, **spike findings**: OSC 10/11 replies from the painted theme colours, mouse forwarding when the agent enabled it, EIO on PTY writes after child exit ignored, CSI 14 t reply (18 t is alacritty's), sync-update deadline + `stop_sync` (BSU-without-ESU test), child exit on reader EOF or 500 ms after `wait`, render on dirty flag/tick instead of a 16 ms timer; host OSC 11 query via `rustix::event::poll` before the input reader; session-marker env scrub with test; cell allowlist + hostile corpus; wheel scrollback; resize-all; INTERACT-on-focus with configurable exit chord; `n` launches `claude`/`codex` with `--` and `--name`; `x` stops; quit confirm; tests: query replies, hostile streams, CJK/emoji width, `spikes/cases/` replayed green; decide Codex `--no-alt-screen`; JIS/German chord check; confirm interactive `claude`/`codex` accept `--` | a full Claude Code session runs inside mc on kitty, Ghostty, tmux (+navigator), VS Code, including answering a permission prompt via passthrough; `compare.py` matches tmux on every case |
| M4 Structure + notifications (Claude) | `bungkus-mc hook` (silent, trimmed, quoted path), socket server, Claude adapter via `--settings` (new + resume argv, unit-tested), decided state machine with `background_tasks`, subagent list, sidebar precedence, `!` across projects, session names (`--name`, `session_title`, verify `/rename` → `session_name`), `notify` bell/desktop/off + OSC 2 title push/pop | cards and sidebar update live during a real session with parallel subagents; bell rings on needs-you; card titles follow renames |
| M5 Usage (Claude) | `bungkus-mc statusline` wrapper (200 ms concurrent forward, then the user's status line under `sh` with buffered stdin; resolver with `CLAUDE_CONFIG_DIR`, recursion guard), `Usage` messages, compact card line, expanded selected card (sessions pane focused), getah-bar limits with thresholds and stale dimming; record a multi-turn session to settle `total_input_tokens` semantics and check which shell Claude uses | tokens/cost/ctx on every Claude card; limits in the bar on a subscription account; `-` on API-key accounts; the user's own status line still renders |
| M6 Codex | verify `/hooks` trust persistence for `-c` injected hooks (delete or build `setup codex`), record real Codex SubagentStart/PreToolUse payloads, Codex adapter, resume rules, "hooks off" hints; **Codex usage reader** (`agent/codex_usage.rs`: canonicalised path under `CODEX_HOME`, `fstat` regular file, tail-only 256 KiB, `token_count` only, tolerant serde structs, fixtures) feeding cards and the `X` limits | Codex session with subagents shows structure and `312k tok · - · ctx 22%`; untrusted path degrades to output only; bad/missing rollout shows `-` |
| M7 Descendant tracking + cleanup | `src/proc/`: 2 s process-tree scan (Linux `/proc` + `rustix` pidfd, macOS `ps` with `LC_ALL=C` and `uid=`), pruning, uid filter, pid + start-time identity, `lsof` port annotation on both OSes, default-keep rule + `cleanup.keep`, quit/`x` dialog (sessions then processes, `[stop]/[keep]` toggle, `… and N more`), fresh scan on open, SIGTERM → 3 s → SIGKILL, EPERM handling, forced `stopped` state; fixture tests per OS incl. ja_JP `ps` | a session that started `vite` is quit; the dialog shows `vite :5173 pid …`; the port is free afterwards; `ssh-agent` is kept; a reused pid is never signalled (test) |
| M8 Resume + polish | `sessions.json`, `r`/`d` with confirms, light theme, `--icons unicode|nerd`, `NO_COLOR`, non-UTF-8 locale, mascot animation in the empty state (tick only while visible), full terminal/keyboard matrix pass, README (exit-chord alternates, manual hook removal) | release **v0.1.0** on the `release` branch, together with bungkus-cli's Daun Pisang release |
| M9 Model routing (opt-in) | `src/route/` (`ureq` to TypeSafe, one Choice question on the `n` start prompt only, tier→model map from config, `const`s for model/budget/floor, 64 KiB body cap, confidence range check, secret-shape guard, fallback), API key from env or a once-per-process keychain command, `consent.json`, consent dialog, picker `model` row, card `model` line, `--model`/`-m` in `launch`; fake-server tests | release **v0.2.0**; a routed session shows `haiku · routed 0.82`; the API down → default model, no error; a prompt with `sk-…` is never sent |

M9 is in stage 1 because it is ~200 isolated lines (one module, one
picker row, one argv flag) and it is opt-in; it does not hold v0.1.0.

Other milestones (M2, M4–M8) are unchanged in scope by the language
decision; their Go-specific wording (packages, `teatest`, `os.FindProcess`)
is replaced by the Rust equivalents in ARCHITECTURE.md and CODING_RULES.md
(modules, `#[cfg(test)]` goldens, `rustix` pidfd).

### Stage 2 — candidates, each gated on a request

- Detached sessions via `claude --bg`/`attach` and the Codex app-server;
  listing sessions mc did not start (`claude agents --json`).
- Codex thread names (extend the rollout reader once the record type is
  confirmed, or app-server).
- `N` new project: `bungkus-cli`'s wizard in the output pane; the
  `bungkus-cli mc` PATH-exec shim.
- Headless "task" sessions with structured cards and per-turn usage.
- History adapter (transcript tailing) — only with demand.
- Nested subagent tree; per-subagent tokens (needs transcripts).
- Routing beyond the start prompt (e.g. re-routing on resume by reading the
  agent's own summary) — needs data mc does not have without transcripts.
- Third agent, if it has hooks. `bungkus-kit` when a third product appears.

## 6. Decided by the product owner (2026-09-30)

1. **Quit:** sessions end on quit; all session ids are recorded for
   resume; mc also stops the processes it observed as descendants of its
   agents (dev servers etc.) — tracked by a periodic process-tree scan,
   identified by pid + start time, never killed merely for holding a port,
   listed in the confirm dialog, SIGTERM then SIGKILL after a grace period
   (ARCHITECTURE.md §3.2–3.3, SECURITY.md, M7).
2. **Workspace:** direct child folders are projects only if they contain
   `CLAUDE.md`, `AGENTS.md` or `.git`; dot-dirs skipped, symlinks followed.
3. **Sessions started outside mc are invisible** in v0.1 — accepted.
4. **Codex usage:** approved to read Codex's own session log, limited to
   the rollout `token_count` records at the path Codex's hook reports —
   the one contained transcript exception (ARCHITECTURE.md §6.3, M6).
5. **Default icon set = ASCII.** Borders stay box-drawing when the locale is
   UTF-8; ASCII borders only when it is not.
6. **Palette:** bungkus-cli adopts Daun Pisang now, released together with
   mc; the owner implements the bungkus-cli side.
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
11. **mc paints a low-saturation green background** ("Daun Teduh"
    `#1c2a21`, body text ~11:1; light "Santan" `#f0f3d8`) when the terminal
    is TrueColor; otherwise the terminal's own bg/fg. Config `background:
    paint | terminal`. The output-pane emulator uses the same colours and
    answers OSC 11 with them. bungkus-cli keeps "never paint" for now
    (possible follow-up).
12. **Mascot:** the banana-leaf packet character (Figma
    `HwlCHEFqRm9hfOfUbtuL4h` node `17:3`; `docs/assets/mascot.svg`, `.gif`,
    generator `mascot-gif.py`; poses idle `18:59`, look-up-left `18:108`,
    look-up-right `18:127`, duck `18:86`, died `18:60` in
    `docs/assets/mascot/`) replaces the logomark. A 16×14 half-block sprite
    (idle/blink/lookL/lookR/duck/hop/stepL/stepR/died) animates centred in
    the output pane's empty state, and **while a
    session runs it sits in the pane's top-right corner** — full size when
    quiet, an 8×6 mini sprite when the PTY is busy — drawn only over blank
    cells (otherwise a `/..\` / `/xx\` title-bar form), with a mood per
    session state (needs you / working / your turn); **cross eyes for
    anything that goes wrong** (failed session, error dialog, live tree
    unavailable, failed-project empty state). Static under `NO_COLOR` or
    `motion: false`; ASCII triangle without half-blocks; fixed brand
    colours with a per-theme legs token.
13. **Sidebar spinner:** a project with any running session shows an ASCII
    `| / - \` spinner in a column after the selection marker; the badge
    covers only needs-you / failed / your-turn.
14. **Kill safety (critic round 3):** default-keep list (`*.app`, agents,
    multiplexers, `docker`, `code`, …, plus `cleanup.keep`) with a
    `[stop]/[keep]` toggle in the dialog; uid-filtered, `LC_ALL=C` `ps`
    parsing; `lsof` for ports on both OSes; user-initiated stop = state
    `stopped`. Routing: `n` start prompt only, secret-shape guard, consent
    in the state dir, constants instead of config knobs.
15. **bungkus-mc is written in Rust; bungkus-cli stays Go.** Evidence, the
    M0 spike (`spikes/`, both prototypes matched the tmux reference on
    every case — Claude Code slash menu, shift+enter, paste and resize;
    Codex; CJK/emoji; alt screen; terminal queries):

    | | Go (Bubble Tea v2 + x/vt + creack/pty) | Rust (ratatui + crossterm + portable-pty + alacritty_terminal 0.26) | tmux (reference) |
    |---|---|---|---|
    | `seq 500000` flood | 5.9 s | **0.48 s** (≈12×) | 0.6 s |
    | colour flood | 236 ms | **92 ms** | — |
    | release binary | 4.3 MB | **1.2 MB** | — |
    | idle CPU | ≈0 % | ≈0 % | — |
    | emulator | `x/vt`, pseudo-version only (no tags), needed a reply-pump goroutine because `Write` blocks on query replies | `alacritty_terminal`, tagged on crates.io, Apache-2.0, answers queries as events | — |
    | key encoder | legacy only, drops shifted keys; kitty ≈150 lines | none in the crate; ours ≈100 lines, kitty ≈80 more | — |
    | code | 405 LOC | 727 LOC | — |
    | clean release build | 2.2 s | 7.3 s | — |
    | dependency graph | 25 modules | 118 crates | — |

    Rust costs more lines, build time and crates; it buys an order of
    magnitude on the one path that matters (an agent flooding the pane), a
    quarter of the binary, and an emulator with real releases. The flood
    numbers were measured **headless, with the emulator advanced on a pump
    thread**; the design advances it on the UI thread, and M3 re-times the
    flood cases interactively before committing to that placement
    (ARCHITECTURE.md §4.1). Spike findings carried into M3: OSC 10/11
    replies from the theme, mouse forwarding, ignore EIO after child exit,
    CSI 14 t replies (18 t is already answered by alacritty), a dedicated
    PTY writer thread, render on dirty/tick, sync-update timeout, the
    child-exit rule, `kitty_keyboard: true`, and the session-marker env
    leak (ARCHITECTURE.md §3.1).

Closed earlier: `--settings` hooks merge with user hooks (verified); Codex
hook injection per launch via `-c` (verified); the start prompt is optional
with the last agent preselected.

16. **Name: bungkus-mc** (was bungkus-mcc), short command **`bkmc`**. Checked
    on 2026-09-30: `bungkus-mc` and `bkmc` are free on crates.io, Homebrew,
    Debian, npm and PyPI, and on GitHub under `osbrjp`. Bare `mc` is not used
    as a command because it clashes with Midnight Commander and the MinIO
    client. `bgmc` was rejected because an npm package installs a `bgmc`
    command. From bungkus-cli: `bungkus-cli mc`.

17. **Licence: proprietary, all rights reserved** (owner decision, for
    now). The repo has a `LICENSE` file. `Cargo.toml` sets
    `license-file = "LICENSE"` and `publish = false`, so the crate can never
    be published to crates.io by accident. Every dependency's licence
    (MIT/Apache-2.0/ISC) allows use in proprietary software, provided its
    notice ships with the binary: each release includes a generated
    `THIRD-PARTY-NOTICES`.

18. **Screen-reader mode: not in v0.1.** Revisit if anyone asks. Users who
    need it can run Claude Code on its own with its screen-reader option.

19. **Title-bar name: `bungkus-mc`**, e.g. `bungkus-mc · 1 needs you`.
    Short, matches the command, survives tab-title truncation.

20. **"wrapped"** is the word for a session whose process has ended. It's
    part of the bungkus voice, the help screen explains it, and the `+`
    icon marks it as finished.

21. **Private GitHub repo** (`osbrjp/bungkus-mc`) for now. Install, update
    check and self-update go through the user's authenticated `gh` CLI;
    `install.sh` is a release asset and falls back to `curl` if the repo
    is made public later. mc never handles a GitHub token.

## 7. Open questions for the product owner

1. **Routing tiers per agent:** Claude `quick → haiku`, `standard → sonnet`,
   `deep → opus` is the proposed default; exact model ids per tier?
2. **Route Codex too?** Codex's `-m` accepts model ids, but the tier map is
   empty by default (no obvious cheap/standard/deep triple); provide one,
   or Claude-only for M9?
3. **Jev cost vs tokens saved:** a Jev call is ~$0.00002 per routed start
   (≤ 4 KiB prompt at $0.042/Mtok), negligible; the real trade is
   quality-on-misroute vs cheaper sessions. Is the 0.6 confidence floor
   right, and should a fallback default to the *cheaper* or the *default*
   model?
4. **Codex thread names:** extend the approved rollout reader to the
   thread-name record once its type is confirmed, or leave Codex titles to
   mc's own name/prompt?

## 8. Risks

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| `alacritty_terminal` renders an agent's TUI wrongly in a case the spike did not cover | low (spike matched tmux on all cases) | high | `spikes/cases/` corpus in CI; `term` is one module; `ctrl-l`/`R`; the agent runs fine outside mc |
| Rust build time / crate count slows the team | medium | low | 7.3 s clean, incremental seconds; `cargo deny` bans duplicate heavy crates; no async runtime |
| Accidental keystrokes to the agent (INTERACT on focus) | medium | medium | four instant signals; `(ctrl-\ back)` in the sessions-pane hint; INTERACT ends when the process exits |
| Key translation misses a key the agent needs | medium | medium | table + both-direction tests; `shift-enter` verified in M3 |
| Cleanup kills the wrong process | low | high | descendants observed by scan only; pid + start-time identity re-checked before every signal; never by port; dialog lists everything first; fixture tests per OS |
| Cleanup misses a daemonised process (forked between scans) | low | low | 2 s interval; documented limit; shorten if seen |
| Hook / statusLine / rollout schema changes | medium | low | all fields optional; recorded fixtures; unknown → ignored / `-` |
| Codex hook trust does not persist for injected hooks | medium | low | M6 first task; `setup codex` fallback fully specified |
| Routing misroutes a hard task to a small model | medium | medium | opt-in; confidence threshold; card shows the model; override in picker; `/model` inside the agent |
| Jev API unavailable / key missing | medium | none | 1.5 s budget, silent fallback to the default model |
| CJK ambiguous-width terminals shift columns | medium (JP team) | medium | ascii default glyphs; narrow unicode set; recorded stream test |
| `ctrl-\` clashes (navigator, VS Code, JIS/German) | medium | low | configurable `interactExit`; documented alternates; tested keyboards |
| Two mc instances on one workspace | low | low | per-pid socket; `sessions.json` last-writer-wins; README note |
| Team bandwidth: TUI + PTY + IPC + proc, in a second language | — | — | 12 crates, no daemon, no async, one contained transcript reader, the Rust skill + YAGNI rules; the spike's `session.rs`/`keys.rs`/`ui.rs` are the starting point |
| `Term` on the UI thread cannot keep up with a flood | low–medium | medium | M3 replays `spikes/cases/flood-*` interactively; fallback is the spike's pump-thread model (ARCHITECTURE.md §4.1) |
