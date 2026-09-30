# bungkus-mcc — Architecture

Status: proposal (product-owner decisions of 2026-09-30 applied). Decisions are for stage 1 unless marked "later". Facts about
Claude Code / Codex were checked on 2026-09-30 against Claude Code 2.1.285
and Codex CLI 0.153.4, the official docs, and live experiments recorded in
the review scratchpad (`sub/ev.log`: real Claude hook payloads incl.
subagents; `cx/ev.log`: Codex `-c` hook injection; `merge/`: `--settings`
hook merge; `sl/`: `--settings` statusLine injection; `vt/`: x/vt
behaviour; `dd/`: `--` prompt handling). "VERIFIED" below means one of
those. Anything marked ASSUMPTION or UNCONFIRMED is re-verified at
implementation time.

## 1. One-paragraph summary

bungkus-mcc is a single Go binary, a Bubble Tea v2 full-screen app. It
spawns each AI agent (`claude`, `codex`) as a child process in a PTY it
owns and renders that PTY through an embedded VT emulator in the right
pane; focusing that pane is interacting with the agent. It learns
*structure* (session state, subagents, current tool, waiting-for-input)
and *usage* (tokens, cost, context %, plan limits) not by parsing anything
on screen but from the agents' own **hook** and **status-line** extension
points: every hook runs `bungkus-mcc hook`, Claude's status line runs
`bungkus-mcc statusline`, and both forward a trimmed JSON line over a
per-process unix socket back into the TUI. mcc never parses agent
transcripts — with **one documented, contained exception**: Codex's usage
figures come from the `token_count` records of the rollout file Codex's
own hook names (§6.3). Optionally (off by default) it asks TypeSafe's Jev
which model tier a start prompt needs (§13).

## 2. How mcc sees subagents — options evaluated

| # | Option | Gives | Costs / breaks | Verdict |
|---|--------|-------|----------------|---------|
| 1 | **PTY + embedded VT emulator** (creack/pty + charmbracelet/x/vt) | The agent's real UI, permission prompts, colours, interactive input. Works for any CLI agent. | Zero structure. Emulator dependency, key re-encoding. x/vt has no semver tag. | **Use — live pane.** |
| 2 | **Tail transcripts** (`~/.claude/projects/<slug>/<sid>.jsonl` + `<sid>/subagents/agent-<id>.jsonl`; `~/.codex/sessions/…/rollout-*.jsonl`) | Full history, sessions mcc did not start. | Both formats are explicitly internal (Claude docs: "changes between versions … can break on any release"; Codex undocumented and mid-migration to SQLite via `codex migrate-rollouts`). Transcripts hold every secret the agent saw. | **Not for structure.** One exception, approved by the owner: Codex `token_count` records for usage only (§6.3). |
| 3 | **Hooks → unix socket** (Claude `--settings`; Codex `-c hooks.*`) | Documented, structured, push-based; identical JSON shape in both agents (VERIFIED: `session_id, transcript_path, cwd, hook_event_name, agent_id, agent_type, tool_name, tool_input, tool_use_id, last_assistant_message, background_tasks`). | Only sessions mcc launched (accepted by the owner for v0.1). Codex needs one-time hook trust. One process spawn per event. | **Use — structure.** |
| 4 | **Headless structured mode** (`claude -p --output-format stream-json`, `codex exec --json`, Codex app-server JSON-RPC) | Cleanest structured stream incl. usage. | Not the agent's interactive UI: mcc would have to build chat + permission UIs — a different product. | **Not for stage 1.** Later: optional "task" session kind. |
| 5 | **tmux/zellij panes** | Detach for free. | Two code paths, requires a multiplexer, mcc cannot draw inside another pane. | **No.** Run mcc *inside* tmux instead. |

Recommendation: **1 + 3** (plus Claude's status line and the Codex usage
reader for numbers, §6).

## 3. Process model

```
 terminal (kitty / ghostty / tmux ...)
   │ raw mode, alt screen
   ▼
 ┌──────────────────────────────── bungkus-mcc (one process) ─────────────────────────────┐
 │  Bubble Tea program: the model owns all state                                            │
 │   ├─ projects pane   ├─ sessions pane (selected project)        ├─ output pane           │
 │                                                                  (one vt.Emulator/session)│
 │  goroutines (each only Program.Send()s a tea.Msg, except the pump):                      │
 │   • pty reader ×N   → ptyDataMsg{sess, []byte}                                            │
 │   • reply pump ×N   : io.Copy(ptmx, emu)   ← the one goroutine that touches an emulator  │
 │   • proc waiter ×N  → sessionExitedMsg{sess, code}  (after the reader has drained)        │
 │   • ipc accept loop → hookEventMsg / usageMsg                                              │
 │   • codex usage tail ×N → usageMsg (§6.3)                                                  │
 │   • proc-tree scan  → descendantsMsg (§3.3, every 2 s while any session runs)             │
 │   • spinner tick    → tickMsg (only while something is running)                           │
 │   • update check    → updateAvailableMsg (copied from bungkus-cli)                         │
 │   • route request   → routedMsg (§13, only when the user starts a routed session)         │
 │                                                                                          │
 │  unix socket  $BUNGKUS_MCC_SOCK = filepath.Clean(${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}})/bungkus-mcc-<uid>/<pid>.sock │
 └───────────┬──────────────────────────────────────────┬───────────────────────────────────┘
             │ PTY; env += BUNGKUS_MCC_SOCK,             │ PTY; same env
             │        BUNGKUS_MCC_SESSION=<mcc id>        │
             ▼                                            ▼
   claude --session-id <uuid> --settings '{…}' [--model m] -- <prompt>    codex -c 'hooks.PreToolUse=[…]' [-m m] … -- <prompt>
             │ runs hooks + statusLine via `sh -c`          │ runs hooks via `sh -c`; writes rollout-*.jsonl
             ▼                                              ▼
   bungkus-mcc hook | bungkus-mcc statusline ──one JSON line──▶ socket ──▶ ipc accept loop
   (exit 0 always; never blocks the agent beyond its own runtime)
```

- One mcc process, no daemon, no background service. Socket per pid so two
  mcc instances never cross-talk. If the socket directory cannot be created
  or verified (§7, SECURITY.md), or the path exceeds the 104-byte `sun_path`
  limit, mcc runs **without a socket**: every card is "output only" with the
  hint from DESIGN.md §11. The live pane never depends on the socket.
- Agents are ordinary children in their own session (creack/pty `Setsid`).
- Prompt: `n` optionally takes an initial prompt, passed as one positional
  argument after `--` (VERIFIED for `claude -p` and `codex exec` with a
  prompt starting with `-`; ASSUMPTION that the *interactive* entry points
  parse `--` the same way — an M3 check; if not, prompts starting with `-`
  are rejected with a hint). It becomes the card title (sanitised, truncated).

### 3.1 Child environment (decided)

Inherit `os.Environ()` and then:

- set `TERM=xterm-256color`, `COLORTERM=truecolor` — the child talks to
  *our* emulator, which is a 256-colour+truecolor xterm-class VT, not to the
  host terminal;
- unset `TERM_PROGRAM`, `TERM_PROGRAM_VERSION`, `KITTY_WINDOW_ID`, `TMUX`,
  `TMUX_PANE`, `WEZTERM_*`, `ITERM_*` — agents probe these to pick
  notification and hyperlink strategies — and `TYPESAFE_API_KEY` (the
  routing key is mcc's, never the agent's; a test asserts it is absent);
- add `BUNGKUS_MCC_SOCK`, `BUNGKUS_MCC_SESSION`, and for Claude
  `BUNGKUS_MCC_USER_STATUSLINE` (§6.2).

The emulator's default foreground/background are **the theme's painted
`bg`/`fg`** (`#1c2a21`/`#d6e2d3` dark, `#f0f3d8`/`#1f2a22` light) whenever
mcc paints (TrueColor and `background: "paint"`), so the agent's screen
blends into the pane; otherwise they come from `tea.BackgroundColorMsg`/
`ForegroundColor` (or the theme's reference values when the host does not
answer). The emulator answers the agent's OSC 10/11 queries with those
same colours, so agents pick their dark (or light) theme to match —
ASSUMPTION that x/vt answers OSC 10/11 from its configured defaults; if it
does not, the reply is produced in the emulator wrapper's OSC callback
(M3 check).

### 3.2 What happens when mcc quits while agents run (decided by the owner)

**Stop the agents, stop what they started, remember the session ids, offer
resume.**

- `q` with running sessions → a **fresh process-tree scan**, then the
  confirm dialog listing the sessions first, then every tracked descendant
  (§3.3) as `basename(comm) [:ports] pid <n>` — never argv — with a
  `[stop]`/`[keep]` toggle per row (`space`); after 8 rows the dialog shows
  `… and N more` (DESIGN.md §5.5). **Exactly the listed set is signalled.**
- **Default-keep rule.** Rows start as `[keep]`, and the SIGHUP path (no
  dialog) never signals them, for: any `comm` under `*.app/Contents/`;
  basenames `gpg-agent`, `ssh-agent`, `tmux`, `screen`, `watchman`,
  `ollama`, `colima`, `docker`, `code`; and anything in config
  `cleanup.keep`. Everything else starts as `[stop]`.
- Confirmed, per session: SIGTERM to the agent's process group, wait ≤ 3 s,
  then SIGKILL to `-pgid` **only if the waiter has not yet reported exit**;
  wait for the reader to drain; then for each `[stop]` descendant —
  identity re-checked as pid **and** process start time (Linux: through
  `os.FindProcess`, pidfd-backed since Go 1.23, so the check and the signal
  cannot race; macOS: re-read `ps` for that pid) — SIGTERM, 3 s grace,
  SIGKILL. EPERM → the row shows `could not stop`. Signals go only to the
  group mcc created or to observed descendants, never to anything merely
  because it holds a port. The socket stays open until the last child has
  exited so `SessionEnd` hooks are delivered.
- A user-initiated `x` or quit forces the session state to **`stopped`**,
  even when the agent's exit code is non-zero (a test covers this).
- `x` (stop one session) runs the same routine for that session, with the
  same dialog.
- Each session's `agentSessionId` is in `sessions.json`; `r` runs the resume
  argv (§5) in the stored `cwd`.
- SIGHUP/terminal close: same as quit, without the dialog, keep rule applied.

Detach was considered and rejected for v0.1 (owner decision): both agents
have daemons (`claude --bg`/`attach`, `codex app-server`), which are the
long-term answer and a stage-2 candidate.

### 3.3 Descendant tracking (dev servers and friends)

Agents start dev servers, watchers and databases that `setsid`/daemonise
and outlive the agent. Design, per owner rulings:

- **Observation, not guessing.** Every 2 s while any session runs, one
  process-tree snapshot is taken and, for each session, the set of
  processes reachable from the agent's pid via `ppid` is added to that
  session's `descendants` set. A process seen once stays in the set even
  after it reparents to init/launchd; that is the whole point of scanning
  periodically. Entries whose `{pid, startTime}` is **missing from a new
  snapshot are pruned** (the process is gone; the pid may be reused).
  (Known limit: a process that forks and reparents between two scans is
  missed; shorten the interval if that shows up in practice.)
- **Identity = pid + start time.** Each entry is `{pid, startTime, uid,
  comm}`; before any signal the start time is re-read and must match, so a
  reused pid is never signalled.
- **Discovery, minimal per OS** (one snapshot = one exec or one directory walk):
  - Linux: `/proc/<pid>/stat` for ppid, `comm` and `starttime` (clock
    ticks since boot; stable identity); `os.Stat("/proc/<pid>")` for the
    owner uid; no external tools.
  - macOS: `ps -axo pid=,ppid=,uid=,lstart=,comm=` run with `LC_ALL=C` in
    its env (argv, no shell). Parsing: three ints, then **exactly five**
    `lstart` tokens (`Wed Sep 30 13:41:02 2026`), then `comm` as the rest
    of the line — it may contain spaces. A ja_JP-locale fixture guards the
    `LC_ALL=C` requirement.
  - Entries whose uid is not ours are dropped on both OSes.
- **Ports are annotation only,** on both OSes via
  `lsof -nP -iTCP -sTCP:LISTEN -a -p <pid,pid,…>` once when the dialog
  opens (`lsof` ships with macOS and most Linux distributions; if it is
  missing or fails, the dialog shows the processes without ports — nothing
  else changes, because ports never decide what is killed). The
  `/proc/net/tcp` inode walk was dropped (YAGNI).
- `comm` goes through `sanitise()` before display (a process can be named
  anything); argv is never read.
- Scope: descendants of mcc-launched agents only. Processes that were
  already running, or started from another terminal, are never touched.

## 4. Data flow

### 4.1 Live output

```
pty fd ──read(32 KiB)──▶ reader goroutine ──Program.Send(ptyDataMsg)──▶ Update(): emu.Write(data); return nil
emu (replies: DSR, DA, XTVERSION …) ──io.Copy(ptmx, emu)── pump goroutine   ← REQUIRED: x/vt blocks Write until its
                                                                               reply pipe is drained (VERIFIED)
View(): output pane = sanitise(emu.Render()) or, with a wheel scroll offset, sanitise(scrollback[off:] + screen)
```

- Rendering uses `Render()` (a string): `tea.View.Content` is a string
  (VERIFIED build with bubbletea v2.0.9 + x/vt pseudo-version 2026-09-27).
  Bubble Tea pins the `ultraviolet` version; x/vt must build against that
  same version — checked in CI by the build itself.
- **The pump is the single exception** to "an emulator is touched only in
  `Update`/`View`". Nothing else about the emulator is shared.
- Resize: on `tea.WindowSizeMsg` (debounced one frame) **every** emulator is
  resized (`emu.Resize` + `pty.Setsize` → SIGWINCH), not only the visible one.
- Scrollback: explicit cap of **10 000 lines per session**. The mouse wheel
  over the output pane changes mcc's offset (there is no keyboard scroll,
  because the output pane is always INTERACT when focused — decided). When
  the agent is on the alternate screen (Codex's TUI), the hint `scroll
  inside the agent` appears. Whether to launch Codex with
  `--no-alt-screen` is tested in M3; default **off**.
- Input in INTERACT: Bubble Tea gives `tea.KeyPressMsg`. **x/vt has no kitty
  keyboard encoder**, so mcc translates keys itself with a small table:
  printable → UTF-8; `enter` → CR; `backspace` → DEL; `esc` → ESC;
  arrows/home/end/pgup/pgdn → CSI or SS3 depending on DECCKM; `tab` → HT,
  `shift-tab` → CSI Z; `ctrl-x` → C0 byte; `alt-x` → ESC + x;
  `shift-enter` → ESC CR (what Claude Code accepts as "newline" — verify in
  M3); F-keys → CSI n~ / SS3. Paste → bracketed paste if the agent enabled
  mode 2004, else raw. Mouse → SGR encoding only if the agent enabled mouse
  reporting. `ctrl-z` is swallowed. A table test covers the table both ways
  and asserts that **every** other key — including `esc`, `tab`,
  `shift-tab`, arrows and all ctrl chords — reaches the PTY.

### 4.2 Rendered-output sanitiser (allowlist)

`sanitise()` runs on everything that reaches the host terminal from an
untrusted source: the emulator's `Render()` output and (a simpler variant)
every string from hooks, prompts, directory names, process names and the
Codex rollout reader (SECURITY.md).

Allowed in emulator output: printable runes (incl. wide and combining),
`SGR` (`CSI … m`) only. Everything else is stripped: all other CSI, all OSC
— including OSC 8, which x/vt's `Render()` re-emits (VERIFIED) and which we
drop rather than filter by scheme in stage 1 — DCS, APC, PM, SOS, C1
controls, and raw C0 except `\n`. Hostile-sequence test corpus: OSC 52
clipboard, OSC 0/2 title, OSC 8 with `file:` and `javascript:`, OSC 1337,
OSC 9/99/777, kitty APC graphics, tmux DCS passthrough, XTWINOPS
(`CSI 21 t`), DECRQSS, DA/DSR/XTVERSION query echoes, C1 bytes 0x80–0x9F,
`ESC c`, `CSI ? 1049 h/l`.

Strings from hooks/prompts/dir names/process names: `ansi.Strip`, then drop
every rune < 0x20 except `\t` (one space), 0x7F, and 0x80–0x9F; truncate.

### 4.3 Structure (hook events)

```
agent ──sh -c '<exe> hook'──▶ bungkus-mcc hook
                               reads stdin (cap 8 MiB, drains the rest), env BUNGKUS_MCC_SOCK/SESSION
                               keeps only the fields below, writes one JSON line to the socket, exit 0
ipc listener ──▶ Program.Send(hookEventMsg) ──▶ agent.Adapter.Reduce(session, event)
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
 x ─► stopped
```

Subagents (flat list, no nesting in stage 1):

- `SubagentStart{agent_id, agent_type}` adds a child (VERIFIED fields).
- Description: the `PreToolUse` for `tool_name == "Agent"` (Claude) /
  `"spawn_agent"` (Codex, ASSUMPTION until M6 records it) carries
  `tool_input.description` and `tool_use_id`; the next unpaired
  `SubagentStart` takes it (FIFO).
- **`background_tasks` is authoritative.** Every Claude `Stop` and
  `SubagentStop` carries `background_tasks: [{id, type: "subagent",
  agent_type, description, status}]` (VERIFIED); the child list is
  reconciled to it on each.
- `SubagentStop{agent_id, agent_transcript_path, last_assistant_message}`
  marks the child wrapped (`agent_transcript_path` exists only on Stop).
- `PreToolUse`/`PostToolUse` carrying `agent_id` update that child's tool.

Session-id binding: Claude sessions get `--session-id <uuid>` chosen by mcc.
Codex sessions bind on the first event carrying their `BUNGKUS_MCC_SESSION`;
after that the reducer **ignores any event whose `session_id` differs**
(nested `claude`/`codex` runs inherit the env). All ids must match the
strict UUID regex
`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`
before use in argv or file names — `codex resume <non-uuid>` would be
treated as a *name* and `claude --resume <non-uuid>` opens an interactive
picker, both of which would leave the card bound to the wrong session.

Claude runs hooks and the status line **only after the workspace-trust
prompt has been answered**, and managed settings with `disableAllHooks` /
`allowManagedHooksOnly` drop ours entirely: no event of any kind within
10 s → "output only" with a hint naming both causes; the card leaves
"output only" the moment any event arrives.

## 5. Agent adapter boundary

```go
// internal/agent

type Kind string // "claude" | "codex"

type Event struct {
    Kind        Kind
    MccSession  string          // from BUNGKUS_MCC_SESSION
    Name        string          // hook_event_name
    SessionID   string
    PromptID    string
    Transcript  string          // stored opaque; read only by the Codex usage reader (§6.3)
    AgentID     string
    AgentType   string
    AgentTranscript string      // SubagentStop only
    ToolName    string
    ToolUseID   string
    ToolDesc    string          // tool_input.description, ≤ 200 chars — the only tool_input field kept
    Notify      string          // notification_type (claude) | "permission_request" (codex)
    LastMessage string          // ≤ 200 chars
    SessionTitle string         // SessionStart.session_title (claude), ≤ 80 chars
    BackgroundTasks []BackgroundTask // {ID, Type, AgentType, Description(≤200), Status}
}

type Usage struct {             // Claude: from `bungkus-mcc statusline`; Codex: from the rollout reader
    SessionID string
    SessionName string          // statusline.session_name (claude), ≤ 80 chars; "" for Codex
    In, Out, CacheRead, CacheWrite int64   // Codex: In includes cached; CacheRead from cached_input_tokens
    CostUSD   *float64          // nil = unknown (Codex)
    CtxUsedPct *float64; CtxSize int64
    Limits    *Limits            // nil when the account has none
}
type Limits struct{ Vendor Kind; Windows []Window }         // Window{Label "5h"|"7d", UsedPct float64, ResetsAt time.Time}

type LaunchOpts struct{ Cwd, Prompt, Name, ResumeID, Model, SockPath string }

type Adapter interface {
    Kind() Kind
    Launch(o LaunchOpts) (argv, env []string, err error) // new or resume; Model → --model / -m when non-empty
    Parse(line []byte) (Event, error)                    // unknown events → Name only
    Reduce(s *Session, e Event)                          // state machine of §4.3
}
```

### 5.1 Claude (`claude.go`)

- **New:** `claude --session-id <uuid> --settings <json> [--model <id>] [--name <name>] -- <prompt?>`
- **Resume:** `claude --resume <id> --settings <json> [--model <id>]` in the
  stored `cwd` (never together with `--session-id`; unit test covers both).
  `--name` is not repeated on resume (the session keeps its name).
- `<json>` is built with `encoding/json`, passed as one argv element;
  contains only `hooks` (synchronous) and `statusLine` (§6.2). Hook command
  = `'<os.Executable() → EvalSymlinks, POSIX single-quoted>' hook` — tested
  with a path containing a space and `'`.
- Events: `SessionStart, UserPromptSubmit, PreToolUse, PostToolUse,
  SubagentStart, SubagentStop, Notification, Stop, SessionEnd`.
- `--settings` hooks **merge** with the user's own hooks (VERIFIED).

### 5.2 Codex (`codex.go`)

- **New:** `codex -c 'hooks.<Event>=[{…}]' … [-m <model>] -- <prompt?>` —
  per-launch hook injection via `-c` works (VERIFIED). Events: `SessionStart,
  UserPromptSubmit, PreToolUse, PostToolUse, PermissionRequest,
  SubagentStart, SubagentStop, Stop, SessionEnd`.
- **Resume:** `codex resume <id>` with the same `-c` hooks. Never
  `resume --last`. If hooks are not trusted the card says
  `not resumable — hooks off`.
- Trust: Codex only runs hooks the user approved once via `/hooks`.
  **M6's first task** is to verify whether that approval persists for our
  byte-stable injected hooks. If yes, `setup codex` is deleted. If not,
  `bungkus-mcc setup codex` merges an mcc block into `~/.codex/hooks.json`
  (round-trip `map[string]any`, abort on malformed JSON, `.bak`, keep mode,
  `EvalSymlinks`, idempotent, y/N, manual removal documented). **Never
  `--dangerously-bypass-hook-trust`.**
- **M6's second task**: record real `SubagentStart`/`SubagentStop`/
  `PreToolUse(spawn_agent)` payloads into `testdata/codex/`.

### 5.3 Session names (decided: the card title is the session's own name)

| Source | Claude Code 2.1.285 | Codex 0.153.4 |
|--------|---------------------|---------------|
| set at launch | `-n, --name <name>` — VERIFIED in `claude --help` ("Set a display name for this session"); mcc passes the picker's name | no launch flag (checked `codex --help`); mcc keeps the name for its own card only |
| read live | status line stdin `session_name` — VERIFIED present in the captured payload (§6.2), refreshed on every assistant message / 30 s; `SessionStart` hook `session_title` — documented field (UNCONFIRMED whether it reflects a later rename) | no hook or status-line field; the thread name lives in `~/.codex/session_index.jsonl` (`{id, thread_name, updated_at}`, observed) and presumably in a rollout record (UNCONFIRMED type) — neither is inside the approved `token_count` exception, so **not read in stage 1** |
| renamed inside the agent | `/rename` (documented command; verify in M4 that `session_name` follows) | `/rename` exists (UNCONFIRMED) — mcc would not see it |
| auto-generated title | Claude generates a session title after the first turns (`ai-title` records exist); UNCONFIRMED whether `session_name` carries it when the user set none | — |

Resolution order for the card title: live `session_name` (Claude) →
`session_title` from SessionStart → the picker's name → first prompt line →
`untitled`. Stored in `sessions.json` as `name` + `nameSource`. Open
question for the owner: extend the Codex rollout reader to the thread-name
record once its type is confirmed, or leave Codex names to mcc.

### 5.4 What is fragile and where it is contained

| Fragile | Blast radius | Containment |
|---------|--------------|-------------|
| Hook field renamed | Card shows less | every field optional; recorded-payload tests |
| FIFO description pairing | description on a sibling | corrected by `background_tasks` |
| CLI flags | launch fails | flags exist only in `Launch`; CI smoke greps `--help` |
| Codex hook trust | Codex card "output only" | detection + hint; PTY unaffected |
| statusLine schema | Claude usage shows `-` | fields optional |
| Codex rollout `token_count` shape (internal) | Codex usage shows `-` | one file, tolerant parser, fixtures (§6.3) |
| Process-tree parsing (`ps` format, `/proc`) | descendants not listed/killed | fixture tests per OS; kill list is never *wider* on parse failure |
| x/vt rendering quirks | visual glitch | `ctrl-l`/`R` full repaint |
| x/vt untagged API | build break on bump | one wrapper package; pinned; diff reviewed |
| Jev API/schema change | routing falls back to default model | one package, timeout, fake-server tests (§13) |

## 6. Usage figures — sources, verified vs assumed

| Number | Claude Code 2.1.285 | Codex 0.153.4 |
|--------|---------------------|---------------|
| tokens (session) | **statusLine stdin** `context_window.total_input_tokens/total_output_tokens` (input **includes cache**; cumulative vs per-request UNCONFIRMED → M5), `current_usage{…}` — VERIFIED | **rollout `token_count`** `info.total_token_usage{input_tokens, cached_input_tokens, output_tokens, reasoning_output_tokens, total_tokens}` — schema confirmed from `codex-rs/protocol`, internal, approved exception (§6.3) |
| tokens (per subagent) | not exposed | not exposed (subagent rollouts are separate files; not read) |
| cost | **statusLine** `cost.total_cost_usd` — VERIFIED | none (credits; USD only for eligible workspaces) → `-` |
| context % | **statusLine** `context_window.used_percentage` — VERIFIED | `token_count.info.model_context_window` + `last_token_usage` → computed |
| plan limits | **statusLine** `rate_limits.five_hour/seven_day` — VERIFIED (absent for API-key users) | `token_count.rate_limits{primary, secondary}{used_percent, window_minutes, resets_at}` — often `null` (issue #14880) → `-` when absent |
| hooks carry usage? | no (VERIFIED) | no |

### 6.1 Decision (owner, 2026-09-30)

Claude: all four numbers from the status line. Codex: tokens, context %
and limits from the rollout `token_count` records; cost `-`. Both feed the
getah bar per vendor (DESIGN.md §6.1). Per-subagent tokens: not shown.

### 6.2 Claude status line delivery (`bungkus-mcc statusline`)

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
   execution and Claude would run the same command without mcc.
2. **Inject** `{"type":"command","command":"'<exe>' statusline",
   "padding": <user's>, "refreshInterval": <user's, or 30 when none>}`.
   30 s keeps `rate_limits` fresh; it also means the user's own command now
   runs every 30 s — documented; their `refreshInterval: 0` is honoured.
3. **Run**: `bungkus-mcc statusline` reads stdin up to 1 MiB. Concurrently,
   with a **≤ 200 ms budget** that never delays the user's line, it forwards
   a `Usage` line (`session_id, session_name, cost, context_window,
   rate_limits` — nothing else) to the socket. Then it runs the user's command with
   `exec.Command("sh", "-c", cmd)`, `Stdin = bytes.NewReader(buf)`,
   `Stdout = os.Stdout`, `Stderr` discarded, and exits with its status.
   Whether Claude uses `sh` or `$SHELL` is an M5 check. No user command →
   print nothing. Stdin > 1 MiB → passed through, not forwarded.
4. Same silence rules as `hook`.

Token semantics (M5 records a multi-turn session): `total_input_tokens`
includes cache and may be per-request; `used_percentage`/`current_usage`
are `null` before the first reply (→ `-`); `context_window_size` varies
(200k, 1M observed). Limits past `resets_at` without a fresher report are
shown dimmed as stale.

### 6.3 Codex usage reader — the one transcript exception (approved)

- **One file:** `internal/agent/codexusage.go`. Nothing else in mcc opens
  a transcript.
- **Input:** the `transcript_path` from Codex's own `SessionStart` hook,
  validated: `filepath.EvalSymlinks` on both the path and
  `${CODEX_HOME:-~/.codex}`, then `filepath.Rel(codexHome, resolved)` must
  not start with `..` and must not be absolute (a string-prefix check is
  not enough: `~/.codex-evil/…` would pass one); extension `.jsonl`. The
  file is opened `O_RDONLY|O_NONBLOCK` and the **fd** is `fstat`ed and
  must be a regular file (no FIFOs, no devices, no swap under our feet).
  Anything else → reader disabled for that session, usage `-`.
- **Read-only, tail only, capped.** Poll every 1 s (`os.Stat` size; on
  shrink restart from 0); read from the last offset, at most 256 KiB per
  poll (a burst larger than that skips ahead to the newest 256 KiB — we
  want the latest record, not history); on attach, start from
  `max(0, size − 256 KiB)`. Lines are split on `\n`; **after any
  skip-ahead the bytes up to the first `\n` are discarded** (they are a
  partial line); a partial trailing line is carried to the next poll, and
  a carry buffer that reaches 256 KiB without a `\n` is dropped up to the
  next `\n`.
- **Only `token_count` records.** A line is first prefiltered with
  `bytes.Contains(line, []byte("\"token_count\""))`; only lines that pass
  are `json.Unmarshal`ed into a fixed struct that reads `type`,
  `payload.type` and the fields below (Go's decoder skips unknown fields;
  the line is not otherwise retained). A line is kept only if
  `type == "event_msg"` and `payload.type == "token_count"`. Extracted:
  `payload.info.total_token_usage.{input_tokens, cached_input_tokens,
  output_tokens}`, `payload.info.last_token_usage.total_tokens`,
  `payload.info.model_context_window`,
  `payload.rate_limits.{primary,secondary}.{used_percent, window_minutes,
  resets_at}` (`resets_at` is **unix seconds**). All optional; `info` or
  `rate_limits` may be `null`. Watch items seen in 0.153.4: a top-level
  `token_usage_record` line type (carries per-turn usage; not read) and a
  `cache_write_input_tokens` field in `turn.completed` usage — if
  `token_count` disappears in a later release, the reader degrades to `-`
  and these are the candidates.
- **Tolerant.** Unknown fields ignored; missing fields → that figure `-`;
  malformed JSON → line skipped; any I/O error → reader stops, card shows
  `-`, one debug-log line. Never an error to the user.
- **Never stored.** Only the resulting `Usage` (numbers) is kept; no line
  text is retained, displayed or logged.
- **Tests:** recorded fixtures (`testdata/codex/rollout-*.jsonl` scrubbed
  to `token_count` + decoy lines), path-validation cases (outside
  `CODEX_HOME`, symlink escape, non-jsonl), truncation/shrink, partial
  line, `null` info/limits.
- **Milestone:** M6. Format changes (0.153 already changed usage
  persistence; paginated rollouts are coming) degrade to `-`, never break.

## 7. State and config storage

| What | Path | Mode |
|------|------|------|
| config | `${XDG_CONFIG_HOME:-~/.config}/bungkus/mcc/config.json` | 0600, dir 0700 |
| state | `${XDG_STATE_HOME:-~/.local/state}/bungkus/mcc/sessions.json` | 0600, dir 0700 |
| routing consent | `${XDG_STATE_HOME:-~/.local/state}/bungkus/mcc/consent.json` (`{"routing": "2026-09-30T…"}`) | 0600 |
| debug log (`--debug` only) | `${XDG_STATE_HOME:-~/.local/state}/bungkus/mcc/mcc.log` | 0600 |
| socket | `filepath.Clean(${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}})/bungkus-mcc-<uid>/<pid>.sock` | dir 0700, sock 0600 |
| update-check cache | `os.UserCacheDir()/bungkus-mcc/latest-release` | 0600 |

`config.json` (all keys optional; workspace can also be the first CLI argument):

```json
{
  "workspace": "/Users/me/Works/OSBR",
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

mcc never writes `config.json`. The Jev model id (`jev-latest`), the
request budget (1.5 s) and the confidence floor (0.6) are constants in
`internal/route` (YAGNI); consent lives in the state dir.

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
checked with `os.Stat`, so symlinked markers count). Dot-dirs skipped,
symlinked child directories followed, names sanitised for display, sorted
by name. Other folders are ignored. No recursion, no project file.

## 8. Concurrency rules

1. The model is the only state. Goroutines own I/O and communicate through
   `Program.Send`.
2. `vt.Emulator` instances are touched only in `Update`/`View` — except the
   reply pump per session (VERIFIED necessity).
3. PTY readers block on `Send` when the UI is busy — backpressure. 32 KiB chunks.
4. The waiter waits for the reader to drain, then sends `sessionExitedMsg`.
5. Socket: one goroutine per connection, one line, 2 s deadline, close.
6. The Codex usage tailer and the process-tree scanner are `tea.Tick`-driven
   commands, not free-running goroutines; each tick does one bounded read
   (≤ 256 KiB / one `ps`) and sends one message.
7. No `time.Sleep` in the model. One **global animation clock** (a single
   `tea.Tick` at 350 ms) drives the sidebar/card/header spinner and the
   mascot (mood sequences in DESIGN.md §5.7); it is armed only while
   something animated is visible and `motion` is on (a running session
   anywhere, or a mascot on screen), and re-armed from `View()` state, so
   a hidden or static UI costs no wake-ups. The corner mascot's
   blank-cell check runs per frame on at most 16×7 cells of the visible
   emulator screen; "busy" = PTY output in the last 1 s (a timestamp set
   in the `ptyDataMsg` handler — typing echo counts).
8. Shutdown (§3.2) runs as a `tea.Cmd` with a "stopping…" view; `tea.Quit`
   only after it completes.

## 9. Package layout

```
main.go, version.go
cmd/
  root.go                      # TUI; optional workspace arg; --debug, --icons, --no-mouse, --theme
  hook.go statusline.go        # silent forwarders
  setup.go                     # `setup codex` (only if M6 proves it necessary)
  update.go                    # copied from bungkus-cli (header: // copied from osbrjp/bungkus-cli@<sha> …)
internal/
  theme/    theme.go           # Daun Pisang tokens (painted + terminal-fallback sets), Icon(), styles — reference copy for bungkus-cli
            mascot.go          # 16×14 + 8×6 pixel maps, frames idle/blink/lookL/lookR/duck/hop/stepL/stepR/died, mood sequences, fixed brand colours + legs token; half-block and ascii renderers
  tui/      app.go keymap.go projects.go sessions.go output.go dialogs.go firstrun.go
            testdata/          # goldens (120×40, 80×24), hostile streams
  agent/    agent.go claude.go codex.go codexusage.go   testdata/{claude,codex}/
  term/     session.go keys.go sanitise.go  # pty + emulator + pump + key table + allowlist
  proc/     tree.go ports.go kill.go        # descendant tracking (§3.3), per-OS files with build tags
  route/    route.go           # Jev tier judgement → model id (§13)
  ipc/      server.go client.go
  store/    paths.go config.go state.go
  workspace/scan.go
```

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
INTERACT. `z` (zoom) is orthogonal to mode. The keymap is one struct of
`key.Binding`s grouped per pane and mode; help, the getah bar and the
per-pane uniqueness test are generated from it.

The sessions pane lists the selected project's sessions (order in
DESIGN.md §5.2); the sidebar badges carry the other projects' worst state.
`!` searches all projects for the next needs-you session, switches the
sidebar selection, selects the card and enters INTERACT. `n` targets the
selected project. The card comparator and the `!` search are pure
functions with table tests.

## 11. Relationship with bungkus-cli

| Option | Pros | Cons | Stage 1? |
|--------|------|------|----------|
| A. Separate repo + binary, **copy** the ~350 shareable lines (theme, update, install.sh, workflows) | zero coupling, own cadence | two copies drift | **Yes** |
| B. Shared module `bungkus-kit` | one source of truth | third repo to tag/release | when a 3rd product exists |
| C. `bungkus-cli mc` subcommand | one install | PTY/VT/socket stack inside a scaffolder | no |
| D. Monorepo | shared code | breaks bungkus-cli's conventions | no |
| E. `bungkus-cli mcc …` exec shim | discoverability, ~20 lines | it is a shim | after mcc's first stable release |

**Recommendation: A now, E later, B only with a third product.** Palette:
**bungkus-cli adopts Daun Pisang now and both ship together** (owner
decision; the owner implements the bungkus-cli side from mcc's
`internal/theme/theme.go`). Copied code carries `// copied from
osbrjp/bungkus-cli@<sha> <path>`; both SECURITY.md files state that updater
fixes apply to both repos. Integration (stage 2): `N` runs `bungkus-cli`'s
wizard in the output pane.

## 12. Terminal compatibility strategy

| Concern | Handled by | Our part |
|---------|-----------|----------|
| colour depth, `NO_COLOR`, tmux RGB | Bubble Tea v2 + colorprofile | **painted green bg + fg only at TrueColor** (`tea.ColorProfileMsg`); at 256/16/`NO_COLOR` the terminal's own bg/fg and the declared 256/16 indices per token (DESIGN.md §2). `background: "terminal"` forces the fallback |
| kitty keyboard vs legacy | Bubble Tea negotiates for *our* input | never bind protocol-only keys; exit chord is a C0 byte; own key table for the child |
| mouse | Bubble Tea | `mouse: false`; wheel = scrollback; modifier-drag for selection documented |
| synchronized output | Bubble Tea | — |
| OSC 8 | Lip Gloss (header path only) | stripped from agent output |
| Nerd Font / glyphs | undetectable | `icons` setting, **default ascii**; borders follow the locale |
| light/dark | `BackgroundColorMsg` | `theme` setting |
| notifications | OSC 9/99/777, BEL fallback; title via OSC 2 with XTWINOPS push/pop | `notify` setting |
| tmux / zellij | `TERM=tmux-256color`; OSC 8 ≥ 3.4 | test matrix |
| Apple Terminal | 256 colours, no OSC 8, no kitty keys | must be fully usable — the floor |
| resize storms | Bubble Tea coalesces | debounce PTY resize by one frame |

Release test matrix: kitty, Ghostty, iTerm2, WezTerm, Alacritty,
Terminal.app, tmux 3.4+ (plain and with vim-tmux-navigator), zellij 0.4x
(default and lock mode), VS Code and Cursor terminals, Warp; keyboards US,
JIS, German. Each: colours, `gg`/exit chord, wheel scroll, resize, INTERACT
with `claude` and `codex` (permission prompt answered via passthrough),
`NO_COLOR=1`, `--icons unicode`, non-UTF-8 locale, BEL/desktop notification.

## 13. Model routing with TypeSafe Jev (opt-in, off by default)

Goal (owner): pick a cheaper model for easy tasks. Where mcc can do that:
only where it sees prompt text — **the optional start prompt at `n`, and
nothing else** (resume is not routed: the session already has a model).
**Keystrokes typed in INTERACT go straight to the PTY and are never
routed.** Honest ceiling: the routed model is the
session's model for its whole life (unless the user changes it inside the
agent, e.g. `/model`), so the saving is "sessions that start with a prompt
run on the tier the first prompt suggests". Sessions started with no
prompt are never routed; a misrouted hard task on a small model costs
quality until the user notices. Whether Claude's plan limits are consumed
model-weighted (so a cheaper model also stretches the 5-hour window) is an
ASSUMPTION to verify.

TypeSafe facts (docs.typesafe.ai, read 2026-09-30): `POST
https://api.typesafe.ai/v1/systemone`, `Authorization: Bearer <key>`, JSON
`{state, model, questions{id:{type:"choice", instructions, criteria}}}` →
`{model, answers{id:{type, choice, probabilities, confidence}}, usage}`;
models `jev-latest` / `jev-1.13.0`; $0.042 per million input tokens,
output free (a 1 KiB prompt costs ~$0.00002); 64k context, text only;
errors 401/422/429/529; no published latency; **no Go SDK** (Python/JS
only) → plain `net/http`; TypeSafe states it does not train on requests;
zero-data-retention only for enterprise, standard retention per their DPA
(UNCONFIRMED period). Direct API access is early-access (UNCONFIRMED
whether a waitlist applies to new keys).

Design — `internal/route` (~200 lines including the key runner and the
secret-shape filter):

```go
type Decision struct{ Tier string; Confidence float64; Source string } // Source: routed | fallback | chosen
func Route(ctx context.Context, c Config, prompt string) Decision       // never returns an error to the caller
```

- One `choice` question: instructions "Which capability tier does this
  software task need from a coding agent?", criteria `quick` (small, local,
  well-specified edits; questions; renames), `standard` (typical feature or
  bug work across a few files), `deep` (architecture, cross-cutting
  refactors, hard debugging, long plans), `unclear` (not a task, or
  ambiguous). State = `{"prompt": <sanitised, truncated to 4 KiB>}` — only
  the prompt, never the project name, cwd, env or history.
- Mapping `tiers[agent][tier]` → model id from config (Claude `--model`,
  Codex `-m`). Constants: model `jev-latest`, budget 1.5 s, confidence
  floor 0.6 (TypeSafe's own guidance is "below 0.5 do not act"). `unclear`,
  confidence below the floor **or outside `[0, 1]`**, a `choice` that is
  not a configured tier, any HTTP/JSON error, 429/529, a response body
  over 64 KiB (`io.LimitReader`), or the budget → `Source: fallback`, agent
  default model. An empty tier map for an agent (Codex by default) → no
  request is made.
- **Secret-shape guard.** If the prompt matches `sk-`, `ghp_`,
  `github_pat_`, `AKIA`, `xox[bp]-` or `-----BEGIN`, no request is made
  and the card shows `default · not routed`.
- The user can override in the `n` picker (`Source: chosen`). The card
  shows `haiku · routed 0.82` / `opus · chosen` / `default · routing fell
  back` (DESIGN.md §10). `sessions.json` records model, source, confidence.
- API key: `TYPESAFE_API_KEY` env var, else `routing.apiKeyCommand` (an
  argv array — no shell — whose stdout is the key: macOS `security`, Linux
  `secret-tool`, `op read`, …). The command runs **once per mcc process,
  lazily on the first routed start**, with its own 10 s timeout, `Stdin`
  nil, stderr discarded, `Setsid` (no controlling tty, so it cannot prompt
  into our screen), stdout capped at 4 KiB and trimmed; the key is kept in
  memory for the process lifetime, never on disk, never logged.
- Consent: the first routed start shows the dialog (DESIGN.md §10); `y`
  writes `consent.json` in the state dir; `n` leaves routing enabled in
  config but every start falls back until consented (the picker shows
  `auto (not consented)`).
- Tests: `httptest` fake server for the 200 path, 401/429/529, malformed
  JSON, oversized body, confidence out of range, unknown choice, slow
  server (timeout), low confidence, empty tier map, secret-shaped prompt
  (no request); the mapping table; the prompt sanitiser/truncation; the
  key runner (timeout, cap, once-only); that no field other than `prompt`
  appears in the request body (golden request); resume never routes.
- Stage: **M9, the last stage-1 milestone, shipped as v0.2.0** — the code
  is small and isolated (one package + one picker row + one argv flag), so
  it is cheap to schedule right after v0.1.0; it does not hold v0.1.0.
