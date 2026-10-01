# bungkus-mc — Architecture

Status: proposal (product-owner decisions of 2026-09-30 applied; **Rust**
per the M0 spike, PROPOSAL.md §6). Decisions are for stage 1 unless
marked "later". Facts about Claude Code / Codex were checked on 2026-09-30
against Claude Code 2.1.285 and Codex CLI 0.153.4, the official docs, live
experiments in the review scratchpad (`sub/`, `cx/`, `merge/`, `sl/`,
`dd/`), and the two M0 prototypes in `spikes/` (`spikes/SPEC.md`,
`spikes/rust/README.md`, `spikes/compare.py`). "VERIFIED" means one of
those. ASSUMPTION / UNCONFIRMED items are re-verified at implementation.

## 1. One-paragraph summary

bungkus-mc is a single Rust binary: a ratatui/crossterm full-screen app
with one event loop that owns all state. It spawns each AI agent
(`claude`, `codex`) as a child process in a PTY it owns and renders that
PTY through an embedded VT emulator (`alacritty_terminal`) in the right
pane; focusing that pane is interacting with the agent. It learns
*structure* (session state, subagents, current tool, waiting-for-input)
and *usage* (tokens, cost, context %, plan limits) not by parsing anything
on screen but from the agents' own **hook** and **status-line** extension
points: every hook runs `bungkus-mc hook`, Claude's status line runs
`bungkus-mc statusline`, and both forward a trimmed JSON line over a
per-process unix socket back into the app. mc never parses agent
transcripts — with **one documented, contained exception**: Codex's usage
figures come from the `token_count` records of the rollout file Codex's
own hook names (§6.3). Optionally (off by default) it asks TypeSafe's Jev
which model tier a start prompt needs (§13). On quit it stops the agents
and the processes they left behind (§3.3).

## 2. How mc sees subagents — options evaluated

| # | Option | Gives | Costs / breaks | Verdict |
|---|--------|-------|----------------|---------|
| 1 | **PTY + embedded VT emulator** (`portable-pty` + `alacritty_terminal`) | The agent's real UI, permission prompts, colours, interactive input. Works for any CLI agent. Spike-verified against a tmux reference on every case (slash menu, shift+enter, paste, resize, Codex, CJK/emoji, alt screen, queries). | Zero structure. Our own key encoder (the crate has none). | **Use — live pane.** |
| 2 | **Tail transcripts** (`~/.claude/projects/<slug>/<sid>.jsonl` + `<sid>/subagents/agent-<id>.jsonl`; `~/.codex/sessions/…/rollout-*.jsonl`) | Full history, sessions mc did not start. | Both formats are explicitly internal (Claude docs: "changes between versions … can break on any release"; Codex undocumented and mid-migration to SQLite). Transcripts hold every secret the agent saw. | **Not for structure.** One owner-approved exception: Codex `token_count` records for usage only (§6.3). |
| 3 | **Hooks → unix socket** (Claude `--settings`; Codex `-c hooks.*`) | Documented, structured, push-based; identical JSON shape in both agents (VERIFIED). | Only sessions mc launched (outside sessions are listed read-only, §3.4). Codex needs one-time hook trust. One process spawn per event. | **Use — structure.** |
| 4 | **Headless structured mode** (`claude -p --output-format stream-json`, `codex exec --json`, Codex app-server JSON-RPC) | Cleanest structured stream incl. usage. | Not the agent's interactive UI: mc would have to build chat + permission UIs — a different product. | **Not for stage 1.** |
| 5 | **tmux/zellij panes** | Detach for free. | Two code paths, requires a multiplexer, mc cannot draw inside another pane. | **No.** Run mc *inside* tmux instead. |

Recommendation: **1 + 3** (plus Claude's status line and the Codex usage
reader for numbers, §6).

## 3. Process model

```
 terminal (kitty / ghostty / tmux ...)
   │ raw mode, alt screen (crossterm)
   ▼
 ┌──────────────────────────────── bungkus-mc (one process) ─────────────────────────────┐
 │  UI thread: the event loop owns the whole model (Elm-style)                              │
 │   ├─ projects pane   ├─ sessions pane (selected project)   ├─ output pane               │
 │                                                             (one alacritty Term/session) │
 │  threads → one mpsc channel of AppEvent into the loop:                                   │
 │   • pty reader ×N   → PtyOutput{sess, bytes}   on a BOUNDED SyncSender (64 × 32 KiB)     │
 │   • pty writer ×N   ← Vec<u8> on an unbounded Sender: keys, paste, query replies (§4.1) │
 │   • child waiter ×N → ChildExited{sess, code}  (reader EOF, or 500 ms after wait, §4.1)  │
 │   • socket listener → Hook / Usage                                                       │
 │   • codex usage tail ×N → Usage (§6.3)                                                    │
 │   • proc scan       → Descendants (§3.3, every 2 s while any session runs)               │
 │   • update check    → UpdateAvailable (once a day)                                       │
 │   • route request   → Routed (§13, only for a routed `n` start)                          │
 │   • input reader    → Input (keys, mouse, paste, resize, focus) via ratatui::crossterm   │
 │  the loop: recv_timeout(deadline) → drain with try_recv → update model → one render     │
 │            deadline = min(next animation tick, each Term's sync_timeout())               │
 │                                                                                          │
 │  unix socket  $BUNGKUS_MC_SOCK = clean(${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}})/bungkus-mc-<uid>/<pid>.sock │
 └───────────┬──────────────────────────────────────────┬───────────────────────────────────┘
             │ PTY; env += BUNGKUS_MC_SOCK,             │ PTY; same env
             │        BUNGKUS_MC_SESSION=<mc id>        │
             ▼                                            ▼
   claude --session-id <uuid> --settings '{…}' [--model m] [--name n] -- <prompt>    codex -c 'hooks.PreToolUse=[…]' [-m m] … -- <prompt>
             │ runs hooks + statusLine via `sh -c`          │ runs hooks via `sh -c`; writes rollout-*.jsonl
             ▼                                              ▼
   bungkus-mc hook | bungkus-mc statusline ──one JSON line──▶ socket ──▶ listener thread
   (exit 0 always; never blocks the agent beyond its own runtime)
```

- One process, no daemon, no async runtime. Socket per pid so two mc
  instances never cross-talk. If the socket directory cannot be created or
  verified (§7, SECURITY.md), or the path exceeds the 104-byte `sun_path`
  limit, mc runs **without a socket**: every card is "output only" with
  the hint from DESIGN.md §11. The live pane never depends on the socket.
- Agents are ordinary children; `portable-pty` makes the child the session
  leader of its PTY (inside the crate, not in our code), so the process
  group is the child's pid.
- Prompt: `n` optionally takes an initial prompt, passed as one positional
  argument after `--` (VERIFIED for `claude -p` and `codex exec` with a
  dash-leading prompt; ASSUMPTION that the interactive entry points parse
  `--` the same way — an M3 check; if not, dash-leading prompts are
  rejected with a hint). It becomes the card title fallback (§5.3).

### 3.1 Child environment (decided)

Start from `std::env::vars_os()` and then:

- set `TERM=xterm-256color`, `COLORTERM=truecolor` — the child talks to
  *our* emulator, a 256-colour+truecolor xterm-class VT, not to the host;
- unset the host-terminal identity vars `TERM_PROGRAM`,
  `TERM_PROGRAM_VERSION`, `KITTY_WINDOW_ID`, `TMUX`, `TMUX_PANE`,
  `WEZTERM_*`, `ITERM_*` (agents probe these to pick notification and
  hyperlink strategies);
- unset `TYPESAFE_API_KEY` (the routing key is mc's, never the agent's);
- **unset the agents' session-marker variables** (NEW, spike finding: a
  child inherited `CLAUDE_CODE_CHILD_SESSION` from the Claude Code session
  the spike ran under and *disabled transcript saving*). Observed in a
  Claude Code 2.1.285 session (names only, values never read):
  `CLAUDECODE`, `CLAUDE_CODE_CHILD_SESSION`, `CLAUDE_CODE_ENTRYPOINT`,
  `CLAUDE_CODE_EXECPATH`, `CLAUDE_CODE_MESSAGING_SOCKET`,
  `CLAUDE_CODE_MESSAGING_TOKEN`, `CLAUDE_CODE_SESSION_ATTENDED`,
  `CLAUDE_CODE_SESSION_ID`, `CLAUDE_EFFORT`, `CLAUDE_PID`. No `CODEX_*`
  markers were present in that session; any found at M3/M6 (running mc
  from inside a Codex session) join the list. **User configuration vars
  are kept**: `CLAUDE_CONFIG_DIR`, `CLAUDE_CODE_PROJECT_DIR_NAME`,
  `CLAUDE_CODE_*` settings the user set on purpose (the list is an explicit
  denylist, not a `CLAUDE_*` wildcard, precisely so those survive),
  `CODEX_HOME`. A test asserts the denylist is absent and the kept names
  present;
- add `BUNGKUS_MC_SOCK`, `BUNGKUS_MC_SESSION`, and for Claude
  `BUNGKUS_MC_USER_STATUSLINE` (§6.2).

The emulator's default foreground/background are **the theme's painted
`bg`/`fg`** (`#1c2a21`/`#d6e2d3` dark, `#f0f3d8`/`#1f2a22` light) whenever
mc paints (TrueColor and `background: "paint"`), so the agent's screen
blends into the pane; otherwise the host's colours (queried once at start:
mc writes `OSC 10 ; ? ST` / `OSC 11 ; ? ST` to the tty and waits for the
reply with `rustix::event::poll` on stdin for up to 200 ms **before**
crossterm's input reader is started — crossterm has no API for this, and a
late reply would otherwise leak into the key stream; the theme's reference
values are used when the host does not answer). **OSC 10/11 replies to the agent
carry those same colours** (spike finding: the prototype answered fixed
values; mc answers `Event::ColorRequest` from the active theme), so
agents pick their dark (or light) theme to match.

### 3.2 What happens when mc quits while agents run (decided by the owner)

**Stop the agents, stop what they started, remember the session ids, offer
resume.**

- `q` with running sessions → a **fresh process-tree scan**, then the
  confirm dialog listing the sessions first, then every tracked descendant
  (§3.3) as `basename(comm) [:ports] pid <n>` — never argv — with a
  `[stop]`/`[keep]` toggle per row (`space`); after 8 rows the dialog shows
  `… and N more` (DESIGN.md §5.5). **Exactly the listed set is signalled.**
- **Default-keep rule.** Rows start as `[keep]`, and the SIGHUP path (no
  dialog) never signals them, for: any `comm` inside an app bundle under an
  `Applications` folder (`…/Applications/*.app/Contents/…`; M7: Homebrew's
  `Python.app` interpreter is not a desktop app and starts as `[stop]`);
  the agents `claude` and `codex`;
  basenames `gpg-agent`, `ssh-agent`, `tmux`, `screen`, `watchman`,
  `ollama`, `colima`, `docker`, `code`; and anything in config
  `cleanup.keep`. Everything else starts as `[stop]`.
- Confirmed, per session: `rustix::process::kill_process_group(pgid,
  SIGTERM)`, wait ≤ 3 s, then SIGKILL to the group **only if the waiter has
  not yet reported exit**; wait for the reader to drain; then for each
  `[stop]` descendant — identity re-checked as pid **and** process start
  time (Linux: `pidfd_open` the pid, re-read `/proc/<pid>/stat`, then
  `pidfd_send_signal`, so the check and the signal cannot race; macOS:
  re-read `ps` for that pid, then `kill`) — SIGTERM, 3 s grace, SIGKILL.
  `EPERM` → the row shows `could not stop`. Signals go only to the group
  mc created or to observed descendants, never to anything merely
  because it holds a port. The socket stays open until the last child has
  exited so `SessionEnd` hooks are delivered.
- A user-initiated `x` or quit forces the session state to **`stopped`**,
  even when the agent's exit code is non-zero (test).
- `x` (stop one session) runs the same routine for that session, with the
  same dialog.
- Each session's `agentSessionId` is in `sessions.json`; `r` runs the resume
  argv (§5) in the stored `cwd`.
- SIGHUP/terminal close: same as quit, without the dialog, keep rule applied.

Detach was considered and rejected for v0.1 (owner decision): both agents
have daemons (`claude --bg`/`attach`, `codex app-server`), which are the
long-term answer and a stage-2 candidate.

### 3.3 Descendant tracking (dev servers and friends)

- **Observation, not guessing.** Every 2 s while any session runs, one
  process-tree snapshot is taken and, for each session, the set of
  processes reachable from the agent's pid via `ppid` is added to that
  session's `descendants` set. A process seen once stays in the set even
  after it reparents to init/launchd. Entries whose `{pid, startTime}` is
  **missing from a new snapshot are pruned**. (`// ponytail:` a process
  that forks and reparents between two scans is missed; shorten the
  interval if that shows up.)
- **Identity = pid + start time.** Each entry is `{pid, start_time, uid,
  comm}`; before any signal the start time is re-read and must match.
- **Discovery, minimal per OS** (one snapshot = one exec or one `/proc` walk):
  - Linux: `/proc/<pid>/stat` for ppid, `comm` and `starttime`;
    `std::fs::metadata("/proc/<pid>")` for the owner uid; no external tools.
  - macOS: `Command::new("ps").args(["-axo", "pid=,ppid=,uid=,lstart=,comm="]).env("LC_ALL", "C")`.
    Parsing: three ints, then **exactly five** `lstart` tokens, then `comm`
    as the rest of the line (it may contain spaces). A ja_JP-locale
    fixture guards the `LC_ALL=C` requirement.
  - Entries whose uid is not ours (`rustix::process::getuid`) are dropped.
- **Ports are annotation only,** on both OSes via
  `lsof -nP -iTCP -sTCP:LISTEN -a -p <pid,pid,…>` once when the dialog
  opens; missing `lsof` = no port labels, nothing else.
- `comm` goes through `sanitise()` before display; argv is never read.
- Scope: descendants of mc-launched agents only.

### 3.4 Sessions started outside mc (read-only)

A thread lists them every 5 s (`src/external.rs`) and sends
`AppEvent::External`; the model keeps the latest list apart from its cards.

- **Claude:** `claude agents --json` (fixed argv, stdin closed, stdout
  capped at 1 MiB, killed after 3 s). Each row gives pid, cwd, name,
  session id and `status`: `busy` → working, `idle` → your turn, a status
  about waiting/input/permission → needs you.
- **Codex:** has no listing. A process of this user whose `comm` basename
  is `codex`, that is not the app-server daemon and whose parent is not
  another `codex` (the npm wrapper), counts as one session; its folder
  comes from `lsof -a -d cwd -p <pids> -Fpn`. Its state is "running".
- A row whose cwd is the project folder or below it shows in that
  project, after mc's cards; one in no project folder (outside the
  workspace, or a folder since deleted) shows under an `elsewhere` row
  that ends the projects list while there is one, with its folder; its state feeds the projects-pane spinner and
  badge. Sessions mc started are left out by pid, tracked descendant pid,
  or session id.
- `x` on one asks "Stop … (pid N)?"; `y` sends SIGTERM if a fresh
  snapshot still shows that pid as this user's `claude`/`codex`/`node`
  (pid + start time re-checked), nothing else.
- Otherwise read-only while they run: no output, no INTERACT, never signalled,
  never written to `sessions.json`, no transcript read.
- **Quick sessions** (`src/app/quick.rs`, issue #46): a session whose
  folder is the workspace root is quick (nothing extra stored). It runs in
  a popup whose PTY is sized to the popup, not the output pane. Moving it
  stops it (SIGTERM to its group, as `x`), then launches the resume in the
  project with `replaces`, so the card is replaced: Claude `--resume <id>
  --fork-session` (verified: the session continues in the new folder with
  its conversation and is saved under that project as a new id), Codex
  `resume <id>` (same session, runs in the new folder). A new project is
  `create_dir` + `git init` (fixed argv) and a rescan.
- **Take over** (`src/app/takeover.rs`): `enter` on a Claude row (its cwd
  must be the project folder and its session id a UUID) opens a dialog
  that waits for the user to quit it in its own terminal. mc checks the
  pid every 300 ms with signal 0 (`test_kill_process`; nothing is
  delivered) and, once it is gone, launches `claude --resume <id>` in the
  project like `r` does, so it becomes an ordinary card. Codex rows are
  refused: nothing tells mc which Codex session a process is.

## 4. Data flow

### 4.1 Live output

Per session: one `MasterPty`, one `alacritty_terminal::Term<Listener>` +
`Processor` (owned by the UI thread), one **reader thread**, one **writer
thread**, one **waiter thread**.

```
                 bounded SyncSender (64 × 32 KiB)                    unbounded Sender<Vec<u8>>
 pty master ──▶ reader thread ────────────────────────▶ UI thread ◀──────────────────────── writer thread ──▶ pty master
  (read)        PtyOutput{sess, bytes}                  │  update():                          owns the one
                EIO / EOF → ReaderClosed{sess}          │   processor.advance(&mut term, &bytes)   MasterPty::take_writer()
                                                        │   Term listener: Event::PtyWrite(reply)  drains the channel,
                                                        │     └─ writer_tx.send(reply)  (never blocks)  writes, ignores EIO
                                                        │   keys / paste / mouse (INTERACT):
                                                        │     └─ writer_tx.send(keys::encode(..))
                                                        │  render(): term.renderable_content() → ratatui cells
                                                        │     (skip WIDE_CHAR_SPACER; colours → Color::Rgb/Indexed)
 child ───────▶ waiter thread: Child::wait() ──▶ ChildExited{sess, code} (see exit rule below)
```

- **The UI thread never blocks on the PTY.** Everything that must reach the
  child — encoded keys, paste, mouse, and the emulator's own query replies
  (DSR/CPR, DA1/DA2, OSC 10/11/12 with the painted theme colours, CSI 18 t,
  kitty keyboard mode query) — is a `send` on the writer channel; the writer
  thread is the only holder of the PTY writer. The Term listener that
  alacritty calls with `Event::PtyWrite` is a struct holding that `Sender`.
  `Event::TextAreaSizeRequest` (CSI 14 t, pixel size) is not answered by
  the crate; mc answers it from the writer with the pane's cell size ×
  a nominal cell size (M3).
- **Backpressure.** The reader's channel is bounded; a flooding agent fills
  it and the reader blocks on the PTY, which slows the agent instead of
  growing memory. The loop drains everything queued with `try_recv` and
  renders **once** per wake-up.
- **Where `Term` lives — the M3 check.** The design advances `Term` on the
  UI thread. The spike measured `seq 500000` in 0.48 s (Go 5.9 s, tmux
  0.6 s) *headless, with `Term` advanced on a pump thread*, so that number
  proves the emulator, not this placement. M3 replays
  `spikes/cases/flood-seq.json` and `flood-color.json` **interactively**;
  if the UI thread cannot keep up (input latency over 100 ms during a
  flood), the fallback is the spike's model — a pump thread owning `Term`
  behind a mutex, the UI thread locking only to render. **Result (M3):**
  `seq 500000` through a session in the release build, measured from
  `enter` in the picker to the card showing `wrapped`, took 0.58 s in tmux
  at 130×40 — on par with tmux itself — so `Term` stays on the UI thread.
- **Child exit rule.** `ChildExited` is sent on reader EOF/EIO **or** 500 ms
  after `Child::wait` returns, whichever comes first: a descendant (a dev
  server) may hold the PTY slave open after the agent is gone, and the
  reader would otherwise never see EOF. The reader treats `EIO` as EOF.
  PTY writes after exit (`EIO`) are ignored (spike finding: the prototype
  exited 1 on them).
- **Synchronized updates from the agent (DEC 2026).** alacritty parks
  output while the agent is inside a BSU/ESU pair and exposes
  `Term::sync_timeout()`; the loop's `recv_timeout` deadline is
  `min(next animation tick, each Term's sync_timeout())`, and when that
  deadline fires the loop calls `stop_sync()` and marks the pane dirty, so
  a lost ESU can never freeze a pane (M3 test: BSU with no ESU renders
  after 150 ms).
- **Rendering is on a dirty flag or a tick**, not every 16 ms (spike
  finding). `PtyOutput` for the visible session, any model change and any
  resize set `dirty`; the loop renders once per wake-up when dirty, and on
  the 350 ms animation tick only while something animated is on screen.
- **Kitty keyboard toward the agent.** M3 builds the emulator with
  `Config { kitty_keyboard: true, .. }` so agents that push kitty flags see
  them advertised, and the encoder emits CSI-u while a flag is active
  (below).
- Resize: a crossterm `Resize` event (coalesced to one frame) resizes
  **every** emulator (`Term::resize`) and PTY (`MasterPty::resize`), not
  only the visible one.
- Scrollback: `Config::scrolling_history = 10_000` per session. The mouse
  wheel over the output pane calls `scroll_display(Scroll::Delta(±3))`;
  any forwarded key snaps to the bottom. On the alternate screen (Codex's
  TUI) there is no history and the hint `scroll inside the agent` appears.
  Whether to launch Codex with `--no-alt-screen` is tested in M3; default
  **off**.
- **Input in INTERACT** goes through our own encoder, `term/keys.rs`, taken
  from the spike (98 lines: printable → UTF-8; ctrl → C0; alt → ESC prefix;
  arrows/home/end in CSI or SS3 by DECCKM with xterm `;m` modifiers;
  PgUp/PgDn/Ins/Del/F1–F12; `shift+tab` → `CSI Z`; `shift+enter`/
  `alt+enter` → `ESC CR`). `alacritty_terminal` has no key encoder (it
  lives in the Alacritty binary), but it tracks DECCKM, 2004 and the kitty
  keyboard mode stack, so **kitty CSI-u output toward the agent is an M3
  task (~80 lines)** driven by those flags — and on the host side crossterm
  pushes `DISAMBIGUATE_ESCAPE_CODES` where the outer terminal supports it,
  so `shift+enter` is distinguishable from `enter` (on a legacy outer
  terminal it is not, and the agent gets CR). Paste → bracketed paste iff
  the child enabled mode 2004 (ESC stripped inside), else raw with LF → CR.
  **Mouse is forwarded** to the agent as SGR when `TermMode::MOUSE_MODE`/
  `SGR_MOUSE` is set (M3, ~30 lines; the spike did not). `ctrl-z` is
  swallowed. A table test covers the encoder both ways and asserts every
  other key — `esc`, `tab`, `shift-tab`, arrows, all ctrl chords — reaches
  the PTY.
- **Queries answered** (M3 checklist from the spike): DSR/CPR, DA1/DA2,
  OSC 10/11/12 with the painted theme colours, CSI 18 t (character-cell
  size, answered by alacritty itself via `PtyWrite`), **CSI 14 t** (pixel
  size, `TextAreaSizeRequest`, answered by mc — the spike ignored it),
  kitty keyboard mode query.

### 4.2 Rendered-output sanitiser (allowlist)

The emulator gives cells, not bytes, so the output pane cannot leak
sequences; the allowlist is the *cell-to-buffer* mapping: only printable
graphemes (wide and combining included) and SGR attributes (fg/bg/bold/
dim/italic/underline/reverse) are copied into the ratatui buffer; OSC 8
hyperlinks are dropped in stage 1; nothing else exists in a cell. The
string sanitiser applies to everything from hooks, prompts, directory
names, process names and the Codex rollout reader: strip any `ESC`-led
sequence (CSI/OSC/DCS/APC/PM/SOS to their terminator), then drop every
char < 0x20 except `\t` (one space), 0x7F, and U+0080–U+009F; truncate.
Hostile corpus (fed to the emulator and to the string sanitiser; the
emulator must neither leak nor mis-render): OSC 52, OSC 0/2, OSC 8 with
`file:`/`javascript:`, OSC 1337, OSC 9/99/777, kitty APC graphics, tmux
DCS passthrough, XTWINOPS `CSI 21 t`, DECRQSS, DA/DSR/XTVERSION echoes, C1
bytes, `ESC c`, `CSI ? 1049 h/l`.

### 4.3 Structure (hook events)

```
agent ──sh -c '<exe> hook'──▶ bungkus-mc hook
                               reads stdin (cap 8 MiB, drains the rest), env BUNGKUS_MC_SOCK/SESSION
                               keeps only the fields below, writes one JSON line to the socket, exit 0
listener thread ──▶ AppEvent::Hook ──▶ Model::hook ──▶ Card::reduce(&event, now)
```

Session state machine (decided; the same for both agents):

```
 launch/SessionStart ─► your turn (nothing happening yet)
 UserPromptSubmit, PreToolUse, PostToolUse, SubagentStart ─► running        (also clears needs-you)
 Notification{permission_prompt | agent_needs_input | elicitation_dialog}   ─► needs you
 PermissionRequest (codex)                                                  ─► needs you
 Notification{idle_prompt}                                                  ─► ignored
 Stop  with no running children (per background_tasks)                      ─► your turn
 Stop  with running children                                                ─► running
 SubagentStop ─► child wrapped; last child + main already stopped           ─► your turn
 process exit 0 ─► wrapped      exit ≠ 0 ─► failed (reason = last non-empty emulator line, sanitised)
 user x / quit ─► stopped (regardless of exit code)
```

Subagents (flat list, no nesting in stage 1):

- `SubagentStart{agent_id, agent_type}` adds a child (VERIFIED fields).
- Description: the `PreToolUse` for `tool_name == "Agent"` (Claude) / a
  name ending in `spawn_agent` (Codex 0.159.2 sends
  `collaborationspawn_agent`, recorded in M6) carries
  `tool_input.description` (Claude) or `tool_input.task_name` (Codex, e.g.
  `say_hi`); the next unpaired `SubagentStart` takes it (FIFO). Codex's
  `Stop` has no `background_tasks`; its list comes from
  `SubagentStart`/`SubagentStop` alone.
- **`background_tasks` is authoritative.** Every Claude `Stop` and
  `SubagentStop` carries `background_tasks: [{id, type: "subagent",
  agent_type, description, status}]` (VERIFIED); the child list is
  reconciled to it on each.
- `SubagentStop{agent_id, agent_transcript_path, last_assistant_message}`
  marks the child wrapped.
- `PreToolUse`/`PostToolUse` carrying `agent_id` update that child's tool.

Recorded in M4 (Claude Code 2.1.285, `src/agent/testdata/claude/session.jsonl`,
scrubbed): **hooks run concurrently** (two events of one tool call can
arrive in either order, so each connection is independent); Claude runs
internal helper agents whose `SubagentStop` has an empty `agent_type` and
an unknown `agent_id` — they are ignored, since only agents seen through
`SubagentStart` or a running `background_tasks` entry are listed; a
`Notification` for a permission prompt carries `notification_type:
"permission_prompt"` and the message "Claude needs your permission";
`SessionStart` and `UserPromptSubmit` carry `session_title` (the `--name`).
On macOS the listener must not drop a connection whose peer already
closed: setting its read timeout then fails with `EINVAL`.

Session-id binding: Claude sessions get `--session-id <uuid>` (generated
with `uuid::Uuid::new_v4`). Codex sessions bind on the first event carrying
their `BUNGKUS_MC_SESSION`; after that the reducer **ignores any event
whose `session_id` differs** (nested `claude`/`codex` runs inherit the
env). All ids must parse with `uuid::Uuid::parse_str` before use in argv
or file names — `codex resume <non-uuid>` would be treated as a *name* and
`claude --resume <non-uuid>` opens an interactive picker.

Claude runs hooks and the status line **only after the workspace-trust
prompt has been answered**, and managed settings with `disableAllHooks` /
`allowManagedHooksOnly` drop ours entirely: no event of any kind within
10 s → "output only" with a hint naming both causes; the card leaves
"output only" the moment any event arrives.

## 5. Agent adapter boundary

There is no adapter trait. An agent is a variant of `agent::Kind`, and
what differs between agents is a `match` arm on it. The boundary is four
pieces in three modules:

```rust
// src/agent/mod.rs — which agent, and its launch argv

pub(crate) enum Kind { Claude, Codex }          // serde: "claude" | "codex"; default Claude
impl Kind {
    const ALL: [Kind; 2];                        // picker order
    const fn command(self) -> &'static str;      // "claude" | "codex"
    const fn badge(self) -> char;                // 'C' | 'X'
    const fn product(self) -> &'static str;      // "Claude Code" | "Codex"
    const fn models(self) -> &'static [&'static str]; // `n` picker; "default" = no flag
}

/// What the user chose in the `n` picker.
pub(crate) struct Launch {
    id: SessionId,                // mc's id; Claude also gets it as --session-id
    model: Option<String>,
    name: Option<String>,         // --name (claude); Codex has no such flag
    prompt: Option<String>,       // one argument after `--`
    settings: Option<String>,     // claude --settings JSON; None without a socket
    hook_args: Vec<String>,       // codex -c hooks.* arguments; empty without a socket
    resume: Option<String>,       // the agent's own session id; must be a UUID
    pick: bool,                   // open the agent's own list of past sessions
    fork: bool,                   // with resume: claude --fork-session
}

/// The one place CLI flags are written (§5.1, §5.2). No shell.
pub(crate) fn argv(kind: Kind, program: &Path, args: &[String], launch: &Launch) -> Vec<OsString>;
pub(crate) fn find_on_path(name: &str, path: &OsStr) -> Option<PathBuf>;

// src/agent/claude.rs — EVENTS, command(exe, subcommand), user_statusline(cwd, config_dir), settings(exe, user)
// src/agent/codex.rs  — EVENTS, hook_args(exe)

// src/ipc/mod.rs — what crosses the socket

/// One hook event, trimmed; every field defaults when absent.
pub(crate) struct HookEvent {
    name: String,                     // hook_event_name, ≤ 64 chars (ids too)
    session_id: Option<String>,       // the agent's own; compared, not parsed
    agent_id: Option<String>,
    agent_type: Option<String>,
    tool_name: Option<String>,
    tool_use_id: Option<String>,
    tool_desc: Option<String>,        // tool_input.description | task_name (codex), ≤ 200 chars — the only tool_input field kept
    notify: Option<String>,           // notification_type (claude) | "permission_request" (codex)
    last_message: Option<String>,     // ≤ 200 chars
    session_title: Option<String>,    // claude, ≤ 80 chars
    background_tasks: Option<Vec<BackgroundTask>>, // ≤ 64 of {id, kind, agent_type, status, description(≤200)}; None when absent
    transcript: Option<String>,       // stored opaque; read only by the Codex usage reader
}

/// One line on the socket.
pub(crate) struct Wire { mc_session: String /* BUNGKUS_MC_SESSION */, event: HookEvent, usage: Option<Usage> }

/// Raw payload → HookEvent in the `hook` subcommand; a field of the wrong type is absent. Never fails.
pub(crate) fn trim(raw: &Value) -> HookEvent;

// src/agent/usage.rs — Claude from `statusline`, Codex from the rollout reader

pub(crate) struct Window { label: String /* "5h" | "7d" */, used_pct: f64, resets_at: Option<u64> /* unix s */ }
#[derive(Default)]
pub(crate) struct Usage {
    session_name: Option<String>,     // statusline.session_name (claude), ≤ 80 chars
    input: Option<u64>, output: Option<u64>, cache_read: Option<u64>, cache_write: Option<u64>,
    cost_usd: Option<f64>,            // None = unknown (Codex)
    ctx_pct: Option<f64>, ctx_size: Option<u64>,
    limits: Vec<Window>,              // empty when the account has none
}

// src/app/sessions.rs — the state machine of §4.3, the same for both agents

impl Card {
    fn reduce(&mut self, event: &HookEvent, now: Instant);
    fn report(&mut self, usage: Usage);
}
```

`app.rs` assembles a launch: it fills `settings` / `hook_args` from
`claude::settings` / `codex::hook_args`, calls `agent::argv`, and adds
`BUNGKUS_MC_SESSION` (always) and `BUNGKUS_MC_SOCK` (when the socket is
up) to the child env. `Model::hook` decodes a `Wire` line and calls
`Card::report` or `Card::reduce`. Limits are kept per vendor in
`Kind::ALL` order (`store::state::Limits`).

Adding an agent is **not** one match arm: a `Kind` variant, its arms in
`Kind`'s methods and in `argv`, a hook-injection file next to
`claude.rs` / `codex.rs`, and the `Kind::Claude` / `Kind::Codex` sites
in `app/`, `store/` and `ui/` (the per-vendor limits array is sized 2).
No capability flags, no registry.

### 5.1 Claude (`claude.rs`)

- **New:** `claude --session-id <uuid> --settings <json> [--model <id>] [--name <name>] -- <prompt?>`
- **Resume:** `claude --resume <id> --settings <json> [--model <id>]` in the
  stored `cwd` (never together with `--session-id`; unit test covers both).
  `--name` is not repeated on resume. `--fork-session` follows the id
  when the session continues as a new one under the launch folder.
- **Past sessions:** `claude --resume --settings <json>` (no id, no
  `--session-id`, no `--name`) opens Claude's own list.
- `<json>` is built with `serde_json`, passed as one argv element;
  contains only `hooks` (synchronous) and `statusLine` (§6.2). Hook
  command = `'<std::env::current_exe() → canonicalize, POSIX single-quoted>' hook`
  — tested with a path containing a space and `'`.
- Events: `SessionStart, UserPromptSubmit, PreToolUse, PostToolUse,
  SubagentStart, SubagentStop, Notification, Stop, SessionEnd`.
- `--settings` hooks **merge** with the user's own hooks (VERIFIED).

### 5.2 Codex (`codex.rs`)

- **New:** `codex -c 'hooks.<Event>=[{…}]' … [-m <model>] -- <prompt?>` —
  per-launch hook injection via `-c` works (VERIFIED). Events: `SessionStart,
  UserPromptSubmit, PreToolUse, PostToolUse, PermissionRequest,
  SubagentStart, SubagentStop, Stop, SessionEnd`.
- **Resume:** `codex resume <uuid>` with the same `-c` hooks; `codex
  resume` without an id opens Codex's own list. Never `resume --last`. If hooks are not trusted the card says
  `not resumable — hooks off`.
- Trust: Codex only runs hooks the user approved, and records the trust
  against the hook definition's hash. **Verified in M6 (Codex 0.159.2):**
  on the first launch with mc's `-c` hooks Codex itself shows "Hooks need
  review · 9 hooks are new or changed" in the pane (answered through
  INTERACT: "Trust all and continue"), and a restart with the
  byte-identical definition goes straight to the prompt. So there is **no
  `setup codex`** subcommand. The definition changes when mc's executable
  path does (a new install location asks once more). A session whose
  hooks never report reads `hooks not trusted · /hooks in codex` after
  10 s. **Never `--dangerously-bypass-hook-trust`.**
- Recorded payloads: `src/agent/testdata/codex/session.jsonl` (scrubbed;
  the prompt and the encrypted spawn message removed) and
  `rollout.jsonl` (the session's three `token_count` records plus decoys).

### 5.3 Session names (decided: the card title is the session's own name)

| Source | Claude Code 2.1.285 | Codex 0.153.4 |
|--------|---------------------|---------------|
| set at launch | `-n, --name <name>` — VERIFIED in `claude --help`; mc passes the picker's name | no launch flag; mc keeps the name for its own card only |
| read live | status line stdin `session_name` — VERIFIED present, refreshed on every assistant message / 30 s; `SessionStart` hook `session_title` — documented (UNCONFIRMED whether it reflects a later rename) | no hook or status-line field; the thread name lives in `~/.codex/session_index.jsonl` and presumably a rollout record (UNCONFIRMED type) — outside the approved `token_count` exception, so **not read in stage 1** |
| renamed inside the agent | `/rename` (verify in M4 that `session_name` follows) | `/rename` (UNCONFIRMED) — mc would not see it |
| auto-generated title | Claude generates titles (`ai-title` records exist); UNCONFIRMED whether `session_name` carries them | — |

Resolution order: live `session_name` → `session_title` → the picker's
name → first prompt line → `untitled`. Stored in `sessions.json` as
`name` + `nameSource`.

### 5.4 What is fragile and where it is contained

| Fragile | Blast radius | Containment |
|---------|--------------|-------------|
| Hook field renamed | Card shows less | every field `Option`/`default`; recorded-payload tests |
| FIFO description pairing | description on a sibling | corrected by `background_tasks` |
| CLI flags | launch fails | flags exist only in `agent::argv`; CI smoke greps `--help` |
| Codex hook trust | Codex card "output only" | detection + hint; PTY unaffected |
| statusLine schema | Claude usage shows `-` | fields optional |
| Codex rollout `token_count` shape (internal) | Codex usage shows `-` | one module, tolerant structs, fixtures (§6.3) |
| Process-tree parsing (`ps` format, `/proc`) | descendants not listed/killed | fixture tests per OS; the kill list is never *wider* on parse failure |
| `alacritty_terminal` API on a minor bump | build break | one module (`term`); exact-minor pin; `Cargo.lock` |
| Jev API/schema change | routing falls back | one module, timeout, fake-server tests (§13) |

## 6. Usage figures — sources, verified vs assumed

| Number | Claude Code 2.1.285 | Codex 0.153.4 |
|--------|---------------------|---------------|
| tokens (session) | **statusLine stdin** `context_window.total_input_tokens/total_output_tokens` (input **includes cache**; cumulative vs per-request UNCONFIRMED → M5), `current_usage{…}` — VERIFIED | **rollout `token_count`** `info.total_token_usage{input_tokens, cached_input_tokens, output_tokens, reasoning_output_tokens, total_tokens}` — schema confirmed from `codex-rs/protocol`, internal, approved exception (§6.3) |
| tokens (per subagent) | not exposed | not exposed |
| cost | **statusLine** `cost.total_cost_usd` — VERIFIED | none (credits; USD only for eligible workspaces) → `-` |
| context % | **statusLine** `context_window.used_percentage` — VERIFIED | `token_count.info.model_context_window` + `last_token_usage` → computed |
| plan limits | **statusLine** `rate_limits.five_hour/seven_day` — VERIFIED (absent for API-key users) | `token_count.rate_limits{primary, secondary}{used_percent, window_minutes, resets_at}` — often `null` → `-` when absent |
| hooks carry usage? | no (VERIFIED) | no |

### 6.1 Decision (owner, 2026-09-30)

Claude: all four numbers from the status line. Codex: tokens, context %
and limits from the rollout `token_count` records; cost `-`. Both feed the
getah bar per vendor (DESIGN.md §6.1). Per-subagent tokens: not shown.

### 6.2 Claude status line delivery (`bungkus-mc statusline`)

`--settings` sets `statusLine` for the session and **displaces the user's
own status line** (VERIFIED). To keep the user's status line working:

1. **Resolve** the user's effective `statusLine` object at launch from
   `<cwd>/.claude/settings.local.json`, `<cwd>/.claude/settings.json`,
   `${CLAUDE_CONFIG_DIR:-~/.claude}/settings.json` (first hit wins).
   Malformed JSON = no status line there; a resolved command that is our
   own `statusline` is dropped (recursion guard); managed settings are not
   readable and would override ours anyway (usage then `-`). Project/local
   settings are repo-controlled; running their command is acceptable
   because Claude's workspace-trust gate precedes any status-line
   execution and Claude would run the same command without mc.
2. **Inject** `{"type":"command","command":"'<exe>' statusline",
   "padding": <user's>, "refreshInterval": <user's, or 30 when none>}`.
   30 s keeps `rate_limits` fresh; it also means the user's own command now
   runs every 30 s — documented; their `refreshInterval: 0` is honoured.
3. **Run**: `bungkus-mc statusline` reads stdin up to 1 MiB. Concurrently
   (one thread, joined with a **≤ 200 ms budget** that never delays the
   user's line) it forwards a `Usage` line (`session_id, session_name,
   cost, context_window, rate_limits` — nothing else) to the socket. Then
   it runs the user's command with `Command::new("sh").args(["-c", cmd])`,
   `stdin` piped from the buffer, `stdout` inherited, `stderr` null, and
   exits with its status. Whether Claude uses `sh` or `$SHELL` is an M5
   check. No user command → print nothing. Stdin > 1 MiB → passed through,
   not forwarded.
4. Same silence rules as `hook`.

Token semantics, **settled in M5** by a recorded two-turn session (Claude
Code 2.1.285, `src/ipc/testdata/statusline*.json`): `total_input_tokens`
includes cache (it equals `current_usage.input_tokens +
cache_creation_input_tokens + cache_read_input_tokens`) and is **per
request, not cumulative** — about 46k after both the first and the second
turn — so it is the current context size, and `used_percentage` is that
over `context_window_size`. `total_cost_usd` is cumulative. The card's
token figure is therefore "the last request's input + output", and the
expanded card's `used` is `total_input_tokens`.
`used_percentage`/`current_usage` are `null` before the first reply (→ `-`);
`context_window_size` varies (200k, 1M observed). Limits past `resets_at`
without a fresher report are shown dimmed as stale. Whether Claude
itself runs status-line commands through `sh` or `$SHELL` is still
unverified; the wrapper uses `sh -c`, which ran the owner's bash-invoking
command unchanged.

### 6.3 Codex usage reader — the one transcript exception (approved)

- **One module:** `agent/codex_usage.rs`. Nothing else in mc opens a
  transcript.
- **Input:** the `transcript_path` from Codex's own `SessionStart` hook,
  validated: `Path::canonicalize` on both the path and
  `${CODEX_HOME:-~/.codex}`, then `resolved.strip_prefix(&codex_home)`
  must succeed (component-wise, so `~/.codex-evil/…` does not pass and
  `..` cannot appear after canonicalisation), extension `jsonl`. The file
  is opened read-only with `OpenOptions::custom_flags(OFlags::NONBLOCK)`
  and the **open file's** `File::metadata()` (an fstat on the fd, not a
  path lookup) must report a regular file. Anything else → reader disabled
  for that session, usage `-`.
- **Read-only, tail only, capped.** Poll every 1 s (`metadata().len()`;
  on shrink restart from 0); `read_at` from the last offset, at most
  256 KiB per poll (a larger burst skips ahead to the newest 256 KiB); on
  attach, start from `max(0, len − 256 KiB)`. Lines split on `\n`; **after
  any skip-ahead the bytes up to the first `\n` are discarded**; a partial
  trailing line is carried to the next poll, and a carry buffer that
  reaches 256 KiB without a `\n` is dropped up to the next `\n`.
- **Only `token_count` records.** A line is first prefiltered with a
  byte search for `"token_count"`; only lines that pass are
  `serde_json::from_slice`d into a fixed tolerant struct (`type`,
  `payload.type`, and the fields below; unknown fields ignored). A line is
  kept only if `type == "event_msg"` and `payload.type == "token_count"`.
  Extracted: `payload.info.total_token_usage.{input_tokens,
  cached_input_tokens, output_tokens}`, `payload.info.last_token_usage.
  total_tokens`, `payload.info.model_context_window`,
  `payload.rate_limits.{primary,secondary}.{used_percent, window_minutes,
  resets_at}` (`resets_at` is **unix seconds**). All optional; `info` or
  `rate_limits` may be `null`. Watch items seen in 0.153.4: a top-level
  `token_usage_record` line type and a `cache_write_input_tokens` field in
  `turn.completed` usage — if `token_count` disappears, the reader degrades
  to `-` and these are the candidates.
- **Tolerant.** Malformed JSON → line skipped; any I/O error → reader
  stops, card shows `-`, one `debug_log!` line. Never an error to the user.
- **Never stored.** Only the resulting `Usage` (numbers) is kept.
- **Tests:** fixtures (`testdata/codex/rollout-*.jsonl` scrubbed to
  `token_count` + decoy lines), path-validation cases, truncation/shrink,
  partial line, `null` info/limits, prefilter counting decoder.
- **Milestone:** M6.

## 7. State and config storage

| What | Path | Mode |
|------|------|------|
| config | `${XDG_CONFIG_HOME:-~/.config}/bungkus/mc/config.json` | 0600, dir 0700 |
| state | `${XDG_STATE_HOME:-~/.local/state}/bungkus/mc/sessions.json` | 0600, dir 0700 |
| routing consent | `${XDG_STATE_HOME:-~/.local/state}/bungkus/mc/consent.json` (`{"routing": "2026-09-30T…"}`) | 0600 |
| debug log (`--debug` only) | `${XDG_STATE_HOME:-~/.local/state}/bungkus/mc/mc.log` | 0600 |
| socket | `clean(${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}})/bungkus-mc-<uid>/<pid>.sock` | dir 0700, sock 0600 |
| update-check cache | `${XDG_CACHE_HOME:-~/.cache}/bungkus-mc/latest-release` (macOS: `~/Library/Caches`, matching bungkus-cli's `os.UserCacheDir`) | 0600 |

`config.json` (all keys optional; workspace can also be the first CLI argument):

```json
{
  "workspace": "/Users/me/Works/OSBR",
  "defaultAgent": "claude",
  "theme": "auto",
  "background": "paint",
  "motion": true,
  "icons": "ascii",
  "mouse": true,
  "notify": "bell",
  "interactExit": "ctrl-\\",
  "agents": {
    "claude": { "command": "claude", "args": [] },
    "codex":  { "command": "codex",  "args": [] }
  },
  "cleanup": { "keep": ["postgres"] },
  "routing": {
    "enabled": false,
    "apiKeyCommand": ["security", "find-generic-password", "-s", "typesafe", "-w"],
    "tiers": {
      "claude": { "quick": "haiku", "standard": "sonnet", "deep": "opus" },
      "codex":  { }
    }
  }
}
```

mc writes `config.json` in one place only: the setup wizard and the
settings screen (DESIGN.md §5.8) save `workspace`, `defaultAgent` and
`theme`. The file is read as a `serde_json::Value` (with `preserve_order`),
those three keys are set, and it is written back atomically (temp file +
`rename`, 0600): every other key, and the key order, survive. A file that
is not a JSON object is never replaced; the save fails with a message.
Consent is still recorded in the state dir, not in config. The Jev model id (`jev-latest`), the
request budget (1.5 s) and the confidence floor (0.6) are `const`s in
`route`; consent lives in the state dir.

`sessions.json` (array; atomic temp + rename):

```json
[{ "id": "m-7c5d", "agent": "claude", "project": "kedai-web",
   "cwd": "/Users/me/Works/OSBR/kedai-web", "agentSessionId": "5f1c…",
   "transcript": "/Users/me/.claude/projects/…/5f1c….jsonl",
   "name": "write proposal", "nameSource": "session_name", "status": "wrapped",
   "model": "haiku", "modelSource": "routed", "routeConfidence": 0.82,
   "startedAt": "2026-09-30T13:41:02+09:00", "endedAt": "…",
   "toolCalls": 38, "subagents": 2,
   "usage": { "in": 486000, "out": 13000, "costUsd": 1.42, "ctxPct": 37 } }]
```

Workspace (decided): one directory; **a project is a direct child directory
that contains `CLAUDE.md`, `AGENTS.md` or `.git`** (file or directory,
checked with `fs::metadata`, so symlinked markers count). Dot-dirs
skipped, symlinked child directories followed, names sanitised, sorted by
name. No recursion, no project file.

## 8. Concurrency rules

1. **The UI thread owns all state** (`app::Model`). Every other thread is
   an I/O source that sends `AppEvent` over one `mpsc::Sender` clone and
   otherwise touches nothing shared. No `Arc<Mutex<_>>` in stage 1 unless
   a channel is clearly worse (then document the lock order).
2. `alacritty_terminal::Term` instances live in the model and are advanced
   and read only on the UI thread (M3 may move them behind a mutex on a
   pump thread if the flood check fails, §4.1). The reader thread sends
   bytes; it never touches the `Term`.
3. PTY readers send on a **bounded** `SyncSender` (64 × 32 KiB) and block
   when it is full — that is the backpressure. `std::sync::mpsc` has no
   unbounded clone of a sync channel, so every other source (input, exits,
   later hooks) shares the same bounded channel; they are rare and the UI
   thread drains everything on each wake-up.
4. Each session has one **writer thread** owning the PTY writer, fed by an
   unbounded `Sender<Vec<u8>>`; the UI thread only `send`s to it (keys,
   paste, mouse, the Term listener's query replies) and never blocks on the
   PTY.
5. The child waiter (`Child::wait` on its own thread) sends `ChildExited`
   on reader EOF/EIO or 500 ms after `wait` returned, whichever is first
   (§4.1), so a descendant holding the slave open cannot hide an exit.
6. Socket: `UnixListener::accept` on one thread; one line per connection,
   2 s read timeout, close; the line is sent as an event and parsed on the
   UI thread.
7. The Codex usage tailer and the process-tree scanner are threads that
   sleep between bounded reads (≤ 256 KiB / one `ps`) and send one event
   each; they exit when the session ends or the last session stops.
8. No `thread::sleep` on the UI thread. The loop blocks in
   `recv_timeout(deadline)` with `deadline = min(next animation tick, each
   Term's sync_timeout())`, then drains with `try_recv` and renders once;
   the 350 ms animation tick exists only while something animated is
   visible and `motion` is on; the corner mascot's blank-cell check runs
   per render on at most 16×7 cells; "busy" = PTY output within the last
   1 s (an `Instant` set in the `PtyOutput` handler — typing echo counts).
9. Shutdown (§3.2) runs on the UI thread with a "stopping…" render between
   steps (the waits are short and bounded); the raw-mode guard's `Drop`
   restores the terminal last.

## 9. Crate layout

```
Cargo.toml, Cargo.lock, rust-toolchain.toml (stable 1.97), rustfmt.toml, clippy.toml, deny.toml
src/
  main.rs            # lexopt parsing → subcommand; anyhow at this level only; panic hook restores the terminal
  app.rs, app/       # the event loop and Model: mode/focus, sessions, dirty flag, ticks, AppEvent
  ui/                # panes, dialogs, first run, keymap.rs (single source of keys/help), theme.rs (token spec impl),
                     # mascot.rs (pixel maps, frames, moods), sanitise.rs (the one string sanitiser)
  term/              # session.rs (PTY + Term + reader/writer/waiter threads), keys.rs (encoder, from the spike), screen.rs (cell allowlist → ratatui buffer); query replies (OSC 10/11, CSI 14 t) live in session.rs
  agent/             # mod.rs (Kind, Launch, argv), usage.rs (Usage), claude.rs, codex.rs, codex_usage.rs; testdata/{claude,codex}/
  ipc/               # server.rs (UnixListener), hook.rs and statusline.rs (the silent subcommands), wire types
  proc/              # mod.rs (scan: /proc on Linux, ps on macOS), ports.rs (lsof), kill.rs, keep rule
  route/             # M9, not built yet: Jev tier judgement → model id, key runner, secret-shape guard, consent
  store/             # mod.rs (XDG paths, atomic writes), config.rs, state.rs (sessions.json, limits.json), debug.rs (the ~20-line debug_log! macro → 0600 file under --debug)
  update.rs          # release check + `update` subcommand (port of bungkus-cli's ~150 lines, through the user's `gh`)
  external.rs        # agent sessions running outside mc, shown read-only (§3.4)
  workspace.rs       # project dir scan
install.sh           # bungkus-cli's script, REPO/BIN_NAME changed
```

One binary crate; no workspace until a second crate exists.

## 10. Modes and focus

```
            l / → / tab / enter-on-session / click output
 NORMAL ────────────────────────────────────────────────▶ INTERACT (output pane focused)
 (projects | sessions pane)                                every key → PTY, except interactExit and ctrl-z
        ▲                                                     │
        │  interactExit (ctrl-\) → sessions pane focused       │
        │  click on projects/sessions pane → that pane         │
        │  session process exits → sessions pane               │
        └──────────────────────────────────────────────────────┘
 FILTER = NORMAL with a text input capturing printable keys (sessions/projects pane)
```

There is no NORMAL state for the output pane (decided): its focus *is*
INTERACT. `z` (zoom) is orthogonal to mode. The keymap is one table of
bindings grouped per pane and mode (`ui/keymap.rs`); help, the getah bar
and the per-pane uniqueness test are generated from it. The sessions pane
lists the selected project's sessions; the sidebar badges carry the other
projects' worst state; `!` searches all projects for the next needs-you
session, switches the sidebar selection, selects the card and enters
INTERACT. `n` targets the selected project.

## 11. Relationship with bungkus-cli

| Option | Pros | Cons | Stage 1? |
|--------|------|------|----------|
| A. Separate repo + binary; **share by spec and script**, not by code | zero coupling, own cadence, own language | two implementations of the palette | **Yes** |
| B. Shared module | one source of truth | impossible across Go and Rust without FFI | no |
| C. `bungkus-cli mc` subcommand | one install | would force mc into Go | no (decided: Rust) |
| D. Monorepo | shared CI | breaks bungkus-cli's conventions; two toolchains in one tree | no |
| E. `bungkus-cli mc …` exec shim (git/kubectl style) | discoverability, ~20 lines of Go | it is a shim | after mc's first stable release |

**Decided: A now, E later.** Since mc is Rust, nothing is copied from the
Go code any more:

- **The repo is private (owner decision)**, so anonymous GitHub downloads
  and API calls don't work. Everything release-related goes through the
  user's authenticated `gh` CLI; mc never reads or stores a GitHub token.
- The updater (~150 lines: latest-release lookup with a daily 0600 cache,
  semver compare, `update --check`, re-running the installer at the
  resolved tag) is **ported** to `src/update.rs`, with the same
  `BUNGKUS_NO_UPDATE_CHECK` and the same one-line hint. The lookup runs
  `gh release view --repo osbrjp/bungkus-mc --json tagName` (argv, 3 s
  timeout). If `gh` is missing or not logged in, the check is skipped
  silently.
- **No sudo by default** (bungkus-cli v1.9.1 practice, like uv, rustup,
  bun, deno and Claude Code). `install.sh` picks the folder in this order:
  `BUNGKUS_INSTALL_DIR`; else the folder of the existing binary
  (`BUNGKUS_CURRENT_BIN`, which `bungkus-mc update` sets to the running
  binary's canonical path, else `command -v bungkus-mc`, symlinks
  resolved); else `~/.local/bin`. If that folder is not writable and
  `~/.local/bin` comes earlier on `PATH`, it installs there instead (the
  new copy shadows the old one) and prints `sudo rm <old path>` to remove
  the old copy; otherwise it uses sudo in place. A folder not on `PATH`
  gets the one line to add for fish, zsh and bash. The logic lives in the
  installer, so older clients get it on their next update.
  `BUNGKUS_INSTALL_DRY_RUN=1` prints the decision and exits before any
  network call; `tests/install.rs` covers the six cases with fake `PATH`
  folders.
- `install.sh` starts from bungkus-cli's script with `REPO=osbrjp/bungkus-mc`
  and `BIN_NAME=bungkus-mc`. It is attached to every release, and fetches
  the binary and `checksums.txt` with `gh release download` (SHA-256
  verified as before). It falls back to `curl` only when the repo is
  public, so it keeps working if the repo is opened later. One addition: after installing, it creates the
  short command `bkmc` as a symlink next to the binary, but only if nothing
  named `bkmc` is already on `PATH` or in the install dir. Otherwise it
  prints one line suggesting `alias bkmc=bungkus-mc` and leaves the existing
  command alone. The release workflow is the same four-workflow
  shape, with the build job in Rust (TECH_STACK.md).
- The **palette is shared as a token spec** — the tables in DESIGN.md §2
  — implemented twice: bungkus-cli `internal/tui/styles.go` (Go, the owner
  is doing it) and mc `src/ui/theme.rs`. A token-table test in each repo
  compares its implementation with the spec, so drift is caught in CI on
  both sides. Both ship together (owner decision).
- The `bungkus-cli mc` PATH-exec shim is language-agnostic and unchanged.
- Both SECURITY.md files say the installer script is shared and fixes to it
  apply to both repos. Stage 2: `N` runs `bungkus-cli`'s wizard in the
  output pane (zero coupling).

## 12. Terminal compatibility strategy

| Concern | Handled by | Our part |
|---------|-----------|----------|
| colour depth, `NO_COLOR`, tmux RGB | our own detection (`COLORTERM`, `TERM`, `NO_COLOR`, `CLICOLOR_FORCE`, `TERM_PROGRAM`), ~30 lines | `Color::Rgb` painted set at TrueColor; **declared** `Color::Indexed` values at 256/16 (DESIGN.md §2); `background: "terminal"` forces the fallback |
| kitty keyboard vs legacy (host) | `ratatui::crossterm` `PushKeyboardEnhancementFlags(DISAMBIGUATE_ESCAPE_CODES)` when supported | never bind protocol-only keys; the exit chord is a C0 byte |
| kitty keyboard toward the agent | our encoder (`term/keys.rs`) driven by the emulator's kitty mode stack (M3) | — |
| mouse | `ratatui::crossterm` `EnableMouseCapture` | `mouse: false`; wheel = scrollback; forwarded to the agent when it enabled mouse modes; modifier-drag for selection documented |
| synchronized output | `BeginSynchronizedUpdate`/`End…` per frame (host side); alacritty's `sync_timeout()` + `stop_sync()` for the agent side (§4.1) | — |
| OSC 8 | not emitted by ratatui; header link written raw only where supported | stripped from agent output |
| Nerd Font / glyphs | undetectable | `icons` setting, **default ascii**; borders follow the locale |
| light/dark | one OSC 11 query at start, reply awaited with `rustix::event::poll` on stdin (200 ms) before the input reader starts (§3.1) | `theme` setting |
| notifications | OSC 9/99/777 raw writes by terminal, BEL fallback; title via OSC 2 with XTWINOPS push/pop | `notify` setting |
| tmux / zellij | `TERM=tmux-256color`; OSC 8 ≥ 3.4 | test matrix |
| Apple Terminal | 256 colours, no OSC 8, no kitty keys | must be fully usable — the floor |
| resize storms | coalesce `Resize` events to one per frame | debounce PTY resize by one frame |

Release test matrix: kitty, Ghostty, iTerm2, WezTerm, Alacritty,
Terminal.app, tmux 3.4+ (plain and with vim-tmux-navigator), zellij 0.4x
(default and lock mode), VS Code and Cursor terminals, Warp; keyboards US,
JIS, German. Each: colours, `gg`/exit chord, wheel scroll, resize, INTERACT
with `claude` and `codex` (permission prompt answered via passthrough),
`NO_COLOR=1`, `--icons unicode`, non-UTF-8 locale, BEL/desktop
notification; plus the `spikes/cases/` corpus replayed by `compare.py`
against tmux.

## 13. Model routing with TypeSafe Jev (opt-in, off by default)

**An optional add-on (owner, 2026-09-30).** With `routing.enabled` absent
or false (the default) no routing code runs at all: no request, no consent
dialog, no key lookup (`apiKeyCommand` never runs), and no "routed / not
routed" wording on cards or in the `n` picker, which shows the plain model
choice. mc then looks exactly as if the feature did not exist. Enabled but
without a key (`TYPESAFE_API_KEY` unset and `apiKeyCommand` unset, failing,
timing out or printing nothing): routing is skipped silently for that
start and the agent's default model is used, with no dialog, toast or
retry of the key command within the process; the card reads plain
`default`, the settings screen shows one line `Jev: no key — routing
skipped`, and the debug log records the reason. The setup wizard never
asks about Jev; it lives only on the settings screen as an add-on toggle
with a key hint. Tests: disabled = zero side effects (no process, socket
or HTTP); enabled without a key = no request, default model.

Goal (owner): pick a cheaper model for easy tasks. Where mc can do that:
only where it sees prompt text — **the optional start prompt at `n`, and
nothing else** (resume is not routed: the session already has a model).
**Keystrokes typed in INTERACT go straight to the PTY and are never
routed.** Honest ceiling: the routed model is the session's model for its
whole life (unless the user changes it inside the agent, e.g. `/model`),
so the saving is "sessions that start with a prompt run on the tier the
first prompt suggests". Sessions started with no prompt are never routed;
a misrouted hard task on a small model costs quality until the user
notices. Whether Claude's plan limits are consumed model-weighted is an
ASSUMPTION to verify.

TypeSafe facts (docs.typesafe.ai, read 2026-09-30): `POST
https://api.typesafe.ai/v1/systemone`, `Authorization: Bearer <key>`, JSON
`{state, model, questions{id:{type:"choice", instructions, criteria}}}` →
`{model, answers{id:{type, choice, probabilities, confidence}}, usage}`;
models `jev-latest` / `jev-1.13.0`; $0.042 per million input tokens,
output free (a 1 KiB prompt costs ~$0.00002); 64k context, text only;
errors 401/422/429/529; no published latency; **no Rust SDK** (Python/JS
only) → `ureq` + `serde_json`; TypeSafe states it does not train on
requests; zero-data-retention only for enterprise, standard retention per
their DPA (UNCONFIRMED period). Direct API access is early-access
(UNCONFIRMED whether a waitlist applies to new keys).

Design — `src/route/` (~200 lines including the key runner and the
secret-shape filter):

```rust
pub(crate) struct Decision { tier: Tier, confidence: f64, source: Source } // Source: Routed | Fallback | Chosen
pub(crate) fn route(cfg: &Config, prompt: &str) -> Decision                // never returns an error to the caller
```

- One `choice` question: instructions "Which capability tier does this
  software task need from a coding agent?", criteria `quick` (small,
  local, well-specified edits; questions; renames), `standard` (typical
  feature or bug work across a few files), `deep` (architecture,
  cross-cutting refactors, hard debugging, long plans), `unclear` (not a
  task, or ambiguous). State = `{"prompt": <sanitised, truncated to 4 KiB>}`
  — only the prompt, never the project name, cwd, env or history.
- Mapping `tiers[agent][tier]` → model id from config (Claude `--model`,
  Codex `-m`). `const`s: model `jev-latest`, budget 1.5 s, confidence
  floor 0.6. `unclear`, confidence below the floor **or outside `[0, 1]`**,
  a `choice` that is not a configured tier, any HTTP/JSON error, 429/529,
  a response body over 64 KiB (`take(64 * 1024)` on the reader), or the
  budget → `Source::Fallback`, agent default model. An empty tier map
  (Codex by default) → no request is made.
- **Secret-shape guard.** If the prompt contains `sk-`, `ghp_`,
  `github_pat_`, `AKIA`, `xoxb-`/`xoxp-` or `-----BEGIN`, no request is
  made and the card shows `default · not routed`.
- The user can override in the `n` picker (`Source::Chosen`). The card
  shows `haiku · routed 0.82` / `opus · chosen` / `default · routing fell
  back` (DESIGN.md §10). `sessions.json` records model, source, confidence.
- API key: `TYPESAFE_API_KEY`, else `routing.apiKeyCommand` (an argv array
  — no shell — whose stdout is the key). The command runs **once per mc
  process, lazily on the first routed start**, in its own process group
  (`std::os::unix::process::CommandExt::process_group(0)`, safe — no
  `pre_exec`, no `setsid`), `Stdio::null()` stdin (so it cannot prompt
  into our screen), stderr discarded, its own 10 s timeout after which the
  whole group is killed, stdout capped at 4 KiB and trimmed; the key is
  kept in memory for the process lifetime, never on disk, never logged.
- Consent: the first routed start shows the dialog (DESIGN.md §10); `y`
  writes `consent.json` in the state dir; `n` leaves routing enabled in
  config but every start falls back until consented (the picker shows
  `auto (not consented)`).
- Tests: a local `TcpListener` fake server for the 200 path, 401/429/529,
  malformed JSON, oversized body, confidence out of range, unknown choice,
  slow server (timeout), low confidence, empty tier map, secret-shaped
  prompt (no request); the mapping table; sanitiser/truncation; the key
  runner (timeout, cap, once-only); golden request body; resume never routes.
- Stage: **M9, the last stage-1 milestone, shipped as v0.2.0**.
