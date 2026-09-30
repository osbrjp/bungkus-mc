# bungkus-mcc — Architecture

Status: proposal (pre-development). Decisions here are for stage 1 unless
marked "later". Facts about Claude Code / Codex were checked on 2026-09-30
against Claude Code 2.1.285 and Codex CLI 0.153.4 and the official docs;
anything marked ASSUMPTION or UNCONFIRMED must be re-verified at
implementation time.

## 1. One-paragraph summary

bungkus-mcc is a single Go binary, a Bubble Tea v2 full-screen app. It spawns
each AI agent (`claude`, `codex`) as a child process in a PTY it owns and
renders that PTY through an embedded VT emulator in the right pane, so the
user gets the agent's real interactive TUI. It learns *structure* (session
state, subagents, current tool, waiting-for-input) not by parsing anything on
screen or on disk but from the agents' own **hook** systems: every hook runs
`bungkus-mcc hook`, which forwards the hook's stdin JSON over a per-process
unix socket back into the TUI. mcc never parses agent transcripts in stage 1.

## 2. How mcc sees subagents — the options, evaluated

| # | Option | What it gives | What it costs / breaks | Verdict |
|---|--------|---------------|------------------------|---------|
| 1 | **PTY + embedded VT emulator** (creack/pty + charmbracelet/x/vt) | The agent's real UI, permission prompts, colors, interactive input passthrough. Works for any CLI agent, today and tomorrow. | Zero structure: a screen full of cells says nothing about subagents. Needs a VT emulator dependency and keyboard/mouse re-encoding. x/vt has no semver tag (pin pseudo-version). | **Use — for the live pane.** |
| 2 | **Tail transcripts** (`~/.claude/projects/<slug>/<sid>.jsonl` + `<sid>/subagents/agent-<id>.jsonl`; `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`) | Full history, works for sessions mcc did not start, read-only. | Both formats are explicitly internal ("changes between versions … can break on any release" — Claude docs; Codex format undocumented and mid-migration to SQLite `thread_history_1.sqlite` via `codex migrate-rollouts`). Needs a JSONL follower per file, path-slug reconstruction, and schema knowledge (`isSidechain`, `agentId`, `.meta.json` are all unofficial). | **Do not use in stage 1.** Keep as a later, optional "history" adapter behind the same event type. |
| 3 | **Hooks → unix socket** (Claude Code `hooks` via `--settings`; Codex `hooks.json`) | Documented, structured, push-based: `SubagentStart/Stop` (agent_id, agent_type, agent_transcript_path), `PreToolUse/PostToolUse` (tool_name, tool_input), `Stop` (last_assistant_message), Claude `Notification` (`permission_prompt`, `idle_prompt`, `agent_needs_input`), Codex `PermissionRequest`, `SessionStart/End`. Same JSON shape in both agents. | Only sees sessions launched with the hooks installed (i.e. by mcc). Codex requires hook trust (`/hooks` approval once, or the hooks are silently skipped). Hooks add a process spawn per event (~5 ms; `async: true` keeps the agent from waiting). No "task description" on `SubagentStart` — pair it with the preceding `PreToolUse` for the `Agent`/`spawn_agent` tool. | **Use — for structure.** |
| 4 | **Headless structured mode** (`claude -p --output-format stream-json --input-format stream-json`, `codex exec --json`, Codex app-server JSON-RPC) | The cleanest structured stream (`parent_tool_use_id`, `task_started`, item events). Documented. | It is *not* the agent's interactive UI: no permission prompts UI, no slash commands, no `/compact`, no editing the prompt; mcc would have to build a chat UI and a permission UI for every agent. That is a different, much bigger product (an agent client), not a mission-control panel. | **Not for stage 1.** Later: an optional "headless session" kind for fire-and-forget tasks. |
| 5 | **tmux/zellij panes** | Detach/persist for free; users already have it. | Two code paths (tmux vs zellij vs none), requires the multiplexer, mcc cannot draw inside another pane, keybinding clashes, and users on plain kitty/Ghostty get nothing. | **No.** Users who want it can run mcc *inside* tmux, which works out of the box. |

### Recommendation: hybrid 1 + 3

- Right pane = PTY + VT emulator (option 1).
- Cards/tree = hook events over a unix socket (option 3).
- Everything undocumented is optional enrichment, isolated in one file per
  agent, and the app is fully functional without it.

Why this holds up: both agents converged on the same hook design (Claude
Code 2.1.x, Codex 0.150+ — event names, config shape
`{"hooks":{"<Event>":[{"matcher":…,"hooks":[{"type":"command",…}]}]}}`,
stdin fields `session_id, transcript_path, cwd, hook_event_name, agent_id,
agent_type, agent_transcript_path, tool_name, tool_input,
last_assistant_message` are the same in both). Hooks are a supported
extension point, not a reverse-engineered file. If a future agent (Gemini
CLI, opencode, …) has hooks, the adapter is ~100 lines; if it does not, it
still gets a PTY card with "output only" and no tree.

## 3. Process model

```
 terminal (kitty / ghostty / tmux ...)
   │ stdin/stdout, raw mode, alt screen
   ▼
 ┌──────────────────────────────── bungkus-mcc (one process) ─────────────────────────────┐
 │  Bubble Tea program (single goroutine owns all state)                                   │
 │   ├─ projects pane   ├─ sessions pane   ├─ output pane (vt.Emulator per session)        │
 │                                                                                        │
 │  goroutines (each only sends tea.Msg via Program.Send):                                │
 │   • pty reader ×N   → ptyDataMsg{sess, []byte}                                          │
 │   • proc waiter ×N  → sessionExitedMsg{sess, code}                                      │
 │   • ipc accept loop → hookEventMsg{sess, agent.Event}                                   │
 │   • spinner tick    → tickMsg (only while ≥1 session running)                           │
 │   • update check    → updateAvailableMsg (copied from bungkus-cli, once per day)        │
 │                                                                                        │
 │  unix socket  $BUNGKUS_MCC_SOCK = <runtime dir>/bungkus-mcc/<pid>.sock  (0600, dir 0700)│
 └───────────┬──────────────────────────────────────────┬─────────────────────────────────┘
             │ PTY (creack/pty), env += BUNGKUS_MCC_SOCK, │ PTY
             │       BUNGKUS_MCC_SESSION=<mcc id>          │
             ▼                                            ▼
   claude --session-id <uuid> --settings '{"hooks":…}'   codex  (hooks from ~/.codex/hooks.json,
             │  runs hook commands                        │  gated by $BUNGKUS_MCC_SOCK)
             ▼                                            ▼
      bungkus-mcc hook  ──stdin JSON──▶ connect($BUNGKUS_MCC_SOCK) ──one line──▶ ipc accept loop
      (exit 0 always, 2 s budget, silent if the socket is gone)
```

- One mcc process, no daemon, no background service. The socket is per
  process (pid in the name) so two mcc instances never cross-talk; stale
  sockets are removed on start.
- Agents are ordinary children with `Setsid` (creack/pty does this). They get
  the user's environment plus two variables. `TERM` is passed through
  unchanged; `COLORTERM` too — the emulator supports what the host terminal
  supports, and x/vt answers capability queries itself.
- Sessions can also start with a prompt (`n` then type): mcc passes it as the
  positional argument (`claude "<prompt>"`, `codex "<prompt>"`) — same as
  typing it, but visible in the card title.

### 3.1 What happens when mcc quits while agents run (stage 1 answer)

**Stop them, remember them, offer resume.**

- On `q` with running sessions: confirm dialog ("2 sessions are still
  working. They will be stopped (you can resume them later). Quit? [y/N]").
- On confirm: send SIGTERM to each session's process group, wait ≤ 3 s, then
  SIGKILL; close PTYs; exit. Claude Code runs its `SessionEnd` hooks on
  SIGTERM and persists the transcript; Codex persists rollouts continuously.
- Each session's `agentSessionId` (Claude: the UUID we chose with
  `--session-id`; Codex: `session_id` from the first hook event) is in
  `sessions.json`. `r` on a stopped card runs `claude --resume <id>` /
  `codex resume <id>` in a fresh PTY — the conversation continues, the tree
  is rebuilt from new hook events (old subagents show as "wrapped" from the
  saved event log).
- On SIGHUP/terminal close: same as quit-confirmed, without the dialog.

Why not persist/detach: both agents now have their own daemons (`claude --bg`
+ `claude attach <id>` / `claude agents --json`; `codex app-server daemon` +
`codex agents`). They are the *right* way to get detach in the long run and
would make mcc a pure viewer. But `--bg` is weeks old, rejects `-p`,
`attach` has its own Ctrl+Z semantics inside a PTY we would have to test, and
Codex parity means speaking its JSON-RPC app-server protocol. Stage 1 should
not depend on either. A tiny mcc daemon (a "session holder" process that keeps
PTYs alive) is the classic answer, but it is a second process with its own
lifecycle, upgrade story and socket protocol — real complexity for a
convenience. Kill + resume covers the actual loss (you don't lose the
conversation, only the in-flight turn) with zero new moving parts.

Later (stage 2, if users ask): "detached" session kind that launches via
`claude --bg` and renders `claude attach <id>` in the PTY; sessions listed
from `claude agents --json --cwd <project>` (`{id, sessionId, name, cwd, kind,
state, status, waitingFor, pid, startedAt}`) also appear in the sidebar for
projects mcc did not start. Codex equivalent via app-server when needed.

## 4. Data flow

### 4.1 Live output

```
pty fd ──read(32 KiB)──▶ goroutine ──Program.Send(ptyDataMsg)──▶ Update():
                                                                  emu.Write(data)   (emulator owned by the model, no mutex)
                                                                  return nil        (Bubble Tea coalesces repaints, ≤60 fps,
                                                                                     DEC 2026 sync where supported)
View(): output pane = emu.Render() placed in the pane box   (x/vt also offers Draw(uv.Screen) — use it if tea.View
                                                            accepts a Drawable; otherwise Render() string, which is fine
                                                            at ≤ 200×60 cells)
```

- Resize: `tea.WindowSizeMsg` → recompute pane rectangles → for the focused
  (visible) emulator `emu.Resize(w,h)` + `pty.Setsize(...)` (kernel sends
  SIGWINCH to the child). Non-visible sessions keep their last size; they are
  resized when shown (agents redraw on SIGWINCH).
- Scrollback: x/vt keeps it; NORMAL-mode `j/k/ctrl-d/ctrl-u` scroll the
  emulator's viewport; INTERACT mode forwards keys instead.
- Input in INTERACT: `tea.KeyPressMsg`/`PasteMsg`/mouse → `emu.SendKeys` /
  `emu.Paste` / mouse encoding → `emu.InputPipe()` bytes → PTY write. x/vt
  encodes according to the modes the *agent* enabled (application cursor
  keys, kitty keyboard flags, mouse reporting), which is exactly the
  translation a multiplexer does.

### 4.2 Structure (hook events)

```
agent process ──runs hook──▶ bungkus-mcc hook
                              reads stdin (≤ 1 MiB), env BUNGKUS_MCC_SOCK, BUNGKUS_MCC_SESSION
                              writes {"v":1,"mccSession":"…","agent":"claude","event":<stdin JSON>}\n
                              exit 0 (always; never blocks the agent; async:true in hook config)
ipc listener ──parse──▶ Program.Send(hookEventMsg) ──▶ agent.Adapter.Reduce(session, event) ──▶ session state
```

State machine per session (both agents map onto it; see 5):

```
            SessionStart / launch
                    │
                    ▼
   ┌───────────► idle ◄──────────────────────────┐
   │              │ UserPromptSubmit              │ Stop (last_assistant_message → summary)
   │              ▼                               │
   │           running ── PreToolUse ─► running(tool=X) ── PostToolUse ─┘
   │              │
   │   Notification{permission_prompt|idle_prompt|agent_needs_input|elicitation_dialog}
   │   PermissionRequest (codex)
   │              ▼
   └── any hook ─ waiting
                  
   process exit code 0 ─► wrapped      exit ≠ 0 / SessionEnd{end_reason≠…} ─► failed (last stderr/emulator line as reason)
   stopped by user (x) ─► stopped
```

Subagents: `SubagentStart{agent_id, agent_type, agent_transcript_path}` adds
a child; `SubagentStop{agent_id, last_assistant_message}` marks it wrapped.
`PreToolUse`/`PostToolUse` that carry `agent_id` update that child's current
tool. Description: the `PreToolUse` for tool `Agent` (Claude) /
`spawn_agent` (Codex) carries `tool_input.description` (Claude) or the prompt
(Codex) and `tool_use_id`; the next unpaired `SubagentStart` takes it (FIFO
pairing — ASSUMPTION: start events arrive in tool-call order; parallel
launches in one assistant turn are the risky case; worst case a description
lands on a sibling, never on the wrong session). Optional enrichment for
Claude: if `<session dir>/subagents/agent-<id>.meta.json` exists it contains
`description`, `agentType`, `model` (UNDOCUMENTED; observed 2.1.285) — read
it in `claude.go` with every field optional, never required.

Nesting: a subagent's own `SubagentStart` (Claude nests; `spawnDepth` in
meta.json, `parent_tool_use_id` in stream-json) is shown one level deeper;
`agent_id` of the *parent* is present in the hook payload when the hook runs
inside a subagent. Stage 1 renders two levels and flattens deeper ones with
a `…` suffix.

## 5. Agent adapter boundary

Only as abstract as two implementations need.

```go
// internal/agent

type Kind string // "claude" | "codex"

type Event struct {          // the normalized subset mcc cares about
    Kind        Kind
    MccSession  string       // from BUNGKUS_MCC_SESSION
    Name        string       // hook_event_name, verbatim
    SessionID   string       // agent's own id (session_id)
    Transcript  string       // transcript_path (kept for `o`, never parsed)
    AgentID     string       // subagent id if any
    AgentType   string
    ToolName    string
    ToolUseID   string
    ToolInput   json.RawMessage
    Notify      string       // notification_type (claude) / "permission_request" (codex)
    LastMessage string       // last_assistant_message
    Raw         json.RawMessage
}

type Adapter interface {
    Kind() Kind
    // Launch returns argv + extra env to start (or resume) a session with hooks wired to sockPath.
    Launch(o LaunchOpts) (argv []string, env []string, err error)
    // Parse turns one hook stdin JSON into an Event. Unknown events → Name set, everything else empty.
    Parse(raw []byte) (Event, error)
    // Reduce applies an Event to a session state; the state machine of §4.2 lives here
    // because the two agents differ only in which hook means "waiting".
    Reduce(s *Session, e Event)
}
```

Two implementations, ~150 lines each:

- `claude.go`: argv `claude --session-id <uuid> --settings '<json>' [--resume <id>] [prompt]`.
  The settings JSON contains only `hooks` with `async: true` commands
  `bungkus-mcc hook` for: `SessionStart, UserPromptSubmit, PreToolUse,
  PostToolUse, SubagentStart, SubagentStop, Notification, Stop, SessionEnd`.
  `--settings` is session-scoped: the user's `~/.claude/settings.json` is
  never touched, and their own hooks keep running (settings merge; only
  identical keys override — ASSUMPTION to verify: `hooks` arrays merge, not
  replace).
- `codex.go`: argv `codex [resume <id>] [prompt]`. Hooks cannot be passed
  per invocation (`-c hooks.…` TOML override is UNCONFIRMED and trust would
  still apply), so `bungkus-mcc setup codex` merges an mcc block into
  `~/.codex/hooks.json` once (idempotent, shows the diff, asks y/N), and the
  user approves it once in Codex's `/hooks`. The hook command exits
  immediately when `BUNGKUS_MCC_SOCK` is unset, so Codex started outside mcc
  pays a few milliseconds and nothing else. Events: `SessionStart,
  UserPromptSubmit, PreToolUse, PostToolUse, PermissionRequest, SubagentStart,
  SubagentStop, Stop, SessionEnd`. If hooks are not trusted, Codex silently
  skips them: mcc shows the card as "output only" after 10 s without a
  `SessionStart` event, with the setup hint. Codex's `notify` config is not
  used (only `agent-turn-complete`, strictly less than `Stop`).

Not in the interface (YAGNI): capability flags, transcript readers, a
generic "provider registry". Adding an agent = new file + one entry in a
`map[Kind]Adapter`.

### 5.1 What is fragile and where it is contained

| Fragile thing | Blast radius | Containment |
|---------------|--------------|-------------|
| Hook payload field renamed | Card shows less (e.g. no tool name) | `Parse` reads fields defensively; every field optional; unknown events ignored; table test per agent with recorded payloads (`testdata/*.json`) |
| `agent-<id>.meta.json` (undocumented) | Description falls back to `agent_type` | one function in `claude.go`, guarded by `os.ReadFile` error → skip |
| Codex hook trust / config location | Codex card is "output only" | detection + one-line hint; PTY view unaffected |
| CLI flags (`--session-id`, `--settings`, `resume`) | Launch fails | `Launch` is the only place flags exist; smoke test in CI runs `claude --help`/`codex --help` and greps the flags (skips if binary absent) |
| x/vt rendering quirks with a specific agent UI | Visual glitch in output pane | `ctrl-l` forces a full repaint; user can always drop to plain terminal — mcc is not the only way to run the agent |

## 6. Modes and keys (the passthrough problem)

Two input modes, one global variable `mode` in the model:

- **NORMAL**: keys are mcc's (vim + arrows; see DESIGN.md §7). All key
  handling goes through `bubbles/key.Matches` against `internal/tui/keymap.go`;
  the help overlay and the getah bar are rendered from that same struct.
- **INTERACT**: every `tea.KeyPressMsg`, `tea.PasteMsg` and (if the agent
  enabled mouse reporting) mouse message is encoded and written to the
  focused session's PTY. One exception, checked before forwarding: `ctrl-\`
  returns to NORMAL.

Entering: `i` or `enter` on the output pane, or `i`/`enter` on a live session
card (which also focuses the output pane). Leaving: `ctrl-\`. Also left
automatically when the session process exits (the pane shows the final
screen and "wrapped/failed", and keys are mcc's again).

Visual: double-line border on the output pane, `INTERACT · ctrl-\ to leave`
in its title, the mode word in reverse-video `warn` color in the getah bar,
and the bar's hints change. `FILTER` mode (after `/`) is the same idea for
the list panes: text goes to the filter input until `enter`/`esc`.

Why not a leader key (tmux-style prefix): a prefix means every mcc key needs
two presses in INTERACT and we'd still need an escape from the prefix. A
single well-chosen exit chord is what `ssh`, `screen`, `docker attach` and
`kubectl attach` do; users know the pattern.

The `gg` motion is the only multi-key sequence in NORMAL; `pendingG bool` in
the model, cleared on any next key. No general chord engine.

## 7. State and config storage

Paths follow XDG on both macOS and Linux (dotdirs in `$HOME`, like the agents
themselves; `os.UserConfigDir` would put macOS config under
`~/Library/Application Support`, which is wrong for a terminal tool).

| What | Path | Format | Mode |
|------|------|--------|------|
| config | `${XDG_CONFIG_HOME:-~/.config}/bungkus/mcc/config.json` | JSON | 0600 |
| state | `${XDG_STATE_HOME:-~/.local/state}/bungkus/mcc/sessions.json` | JSON | 0600 |
| per-session event log | `${XDG_STATE_HOME:-~/.local/state}/bungkus/mcc/events/<mcc id>.jsonl` | JSONL of `Event` (without `Raw`) | 0600 |
| log (only with `--debug` or `BUNGKUS_MCC_DEBUG=1`) | `${XDG_STATE_HOME:-~/.local/state}/bungkus/mcc/mcc.log` | `log/slog` text | 0600 |
| socket | `${XDG_RUNTIME_DIR:-$TMPDIR}/bungkus-mcc-<uid>/<pid>.sock` | — | dir 0700, sock 0600 |
| update-check cache | `os.UserCacheDir()/bungkus-mcc/latest-release` (as bungkus-cli) | text | 0644 |

`config.json`:

```json
{
  "workspace": "/Users/me/Works/OSBR",
  "theme": "auto",
  "icons": "unicode",
  "mouse": true,
  "agents": {
    "claude": { "command": "claude", "args": [] },
    "codex":  { "command": "codex",  "args": [] }
  }
}
```

`sessions.json` (array; written atomically via temp file + rename):

```json
[{ "id": "m-7c5d", "agent": "claude", "project": "bungkus-mcc",
   "cwd": "/Users/me/Works/OSBR/bungkus-mcc", "agentSessionId": "5f1c…",
   "transcript": "/Users/me/.claude/projects/…/5f1c….jsonl",
   "title": "write proposal", "status": "wrapped",
   "startedAt": "2026-09-30T13:41:02+09:00", "endedAt": "…",
   "toolCalls": 38, "subagents": 2 }]
```

Workspace = one directory; projects = its immediate subdirectories (hidden
dirs skipped), sorted by name, `.git` presence shown. No recursion, no
project metadata file — the folder *is* the project. `w` opens a
`bubbles/filepicker` limited to directories.

Everything mcc stores is derivable or disposable: deleting the state dir loses
the session list and nothing else.

## 8. Concurrency model in Bubble Tea

Rules (enforced in review):

1. The model is the only state. Goroutines own I/O, never state; they
   communicate only through `Program.Send(msg)`.
2. `vt.Emulator` instances are owned by the model and touched only in
   `Update`/`View`. Plain `Emulator`, not `SafeEmulator` — no lock needed.
3. PTY readers block on `Send` when the UI is busy; that is the backpressure.
   Chunk size 32 KiB; a session that emits 10 MB/s of output makes its pane
   laggy, not the app.
4. The socket listener is one goroutine per connection, connection lifetime
   is one line, then close. Malformed lines are dropped and counted.
5. No `time.Sleep` in the model; timers are `tea.Tick`. The spinner tick is
   only scheduled while something is running.
6. Process wait goroutine per session; `sessionExitedMsg` triggers the
   wrapped/failed transition and PTY close (in that order).
7. Shutdown: `tea.Quit` only after the kill/wait sequence in §3.1 completed,
   run from a `tea.Cmd` so the UI can show "stopping…".

## 9. Package layout

```
main.go, version.go            # same shape as bungkus-cli (Version via -ldflags)
cmd/
  root.go                      # starts the TUI (Cobra root, no subcommand)
  hook.go                      # `bungkus-mcc hook`: stdin → socket, exit 0; ~40 lines
  setup.go                     # `bungkus-mcc setup codex`: merge hooks.json (y/N)
  update.go                    # copied from bungkus-cli, repo name changed
internal/
  theme/    theme.go           # Daun Pisang palette (dark/light), Icon(), styles; the file future bungkus-kit extracts
  tui/      app.go             # tea.Model: layout, modes, message routing
            keymap.go          # key.Binding set; help + getah bar generated from it
            projects.go sessions.go output.go dialogs.go
            *_test.go          # teatest golden views at 120×40 and 80×24, NO_COLOR and ascii icons
  agent/    agent.go           # Event, Adapter, Session state machine
            claude.go codex.go # two adapters; testdata/ recorded hook payloads
  term/     session.go         # pty.Start + vt.Emulator + reader/waiter goroutines
  ipc/      server.go client.go# unix socket listener; client used by cmd/hook.go
  store/    paths.go config.go state.go  # XDG paths, JSON load/save (atomic)
  workspace/scan.go            # list project dirs
```

No `pkg/`: nothing is meant to be imported by others. `internal/theme` is the
one package written so it can be lifted into a shared module verbatim.

## 10. Relationship with bungkus-cli

Options considered:

| Option | Pros | Cons | Stage 1? |
|--------|------|------|----------|
| A. Separate repo + binary, **copy** the ~350 shareable lines (theme, update.go, install.sh, release workflow) | Zero coupling, each repo releases on its own cadence, same tooling by copy-paste, nothing new to learn | Drift between two copies of the palette / updater | **Yes** |
| B. Separate repos sharing a Go module `bungkus-kit` (theme, selfupdate, version) | One source of truth | A third repo to version/tag/release; every palette tweak = kit release + two dependency bumps; `go.work` for local dev; for 2 consumers and 350 lines it costs more than the drift it prevents | Later, when a 3rd product exists or the copies diverge twice |
| C. `bungkus-cli mc` subcommand (same binary) | One install, one update | A 12 MB scaffolder grows a PTY/VT/socket stack; different release cadence and risk profile; a bug in mcc's PTY code ships in the scaffolder; Cobra tree and TUI styles entangle | No |
| D. Monorepo, two binaries | Shared code without a module release | Breaks the existing bungkus-cli repo/CI/branch conventions and the semantic-release setup that tags one product per repo | No |
| E. Plugin exec: `bungkus-cli mcc …` finds `bungkus-mcc` on PATH and `exec`s it (git/kubectl style) | Discoverability, one brand entry point; ~20 lines in bungkus-cli | It is a shim; nothing else changes | Cheap; do it in bungkus-cli when mcc reaches its first stable release, not before (speculative until then) |

**Recommendation: A now, E as a 20-line follow-up, B only if a third product appears.**

Reuse (by copy, then adapt):
- `pkg/update.go` + `cmd/update.go` + `install.sh` + `.github/workflows/{release,run-tests,promote,start-pull-request}.yml` + `.releaserc.json` — change repo/binary name only. Same channels (main = canary, release = stable), same checksum-verified installer, same daily update check with `BUNGKUS_NO_UPDATE_CHECK` honored (shared env var name on purpose).
- Palette: `internal/tui/styles.go` → `internal/theme/theme.go` with the Daun Pisang values (DESIGN.md §2.4); bungkus-cli adopts the same file in its own PR later.
- Conventions: CLAUDE.md structure, conventional commits, `i{issue#}-{date}-{seq}` branches, table-driven tests, govulncheck in CI, SECURITY.md format.

Do not reuse: the registry/template machinery, the wizard model, `pkg/*`
scaffolding code — mcc has no scaffolding concern.

Integration ideas (cheap because the output pane is a terminal):
- `N` "new project" runs `bungkus-cli` (its interactive wizard) in the output
  pane with cwd = workspace; when it exits 0, rescan the workspace. No
  library coupling, no flags to keep in sync. If `bungkus-cli` is not on
  PATH, the hint shows the install one-liner.
- Both products print the same "newer version available" line and use the
  same installer shape, so a future `bungkus` umbrella installer is trivial.

## 11. Terminal compatibility strategy

Rely on the stack, verify on a matrix, and keep every feature optional.

| Concern | Who handles it | Our part |
|---------|----------------|----------|
| Color depth (truecolor → 256 → 16 → none), `NO_COLOR`, `CLICOLOR_FORCE`, tmux `Tc`/`RGB` | Bubble Tea v2 + colorprofile downsample automatically | Declare hex once (theme); verify the 256/16 table in DESIGN.md renders distinguishable; `--color truecolor|256|16|none` override |
| Kitty keyboard protocol vs legacy | Bubble Tea v2 requests enhancements, falls back silently; inside tmux without `extended-keys` the request fails and legacy keys work | Never bind keys that only exist with the protocol (no `shift-enter`, no key-release); the exit chord is a plain C0 byte |
| Mouse | Bubble Tea `WithMouseCellMotion` | `--no-mouse`; wheel only scrolls; INTERACT forwards only if the agent enabled reporting |
| Synchronized output (DEC 2026) | Bubble Tea renderer | nothing |
| OSC 8 hyperlinks | Lip Gloss v2 | Only on paths/ids; text identical without it |
| OSC 52 clipboard | Bubble Tea cmd | `y` in output pane; falls back to "copy not supported here" |
| Nerd Font glyphs | cannot be detected | `icons = nerd|unicode|ascii`, default unicode; ascii auto when no UTF-8 locale or `TERM=linux|dumb` |
| Light/dark background | `tea.BackgroundColorMsg` (terminal query), Apple Terminal/tmux may not answer | default dark; `theme: auto|dark|light` |
| tmux / zellij | pass-through with `TERM=tmux-256color`/`screen-256color`; tmux ≥ 3.4 for OSC 8, `allow-passthrough` not needed (we send no DCS) | manual test matrix in CODING_RULES.md; screenshots in the PR for the two multiplexers |
| Apple Terminal | 256 colors (truecolor only on macOS 26+, UNCONFIRMED), no OSC 8, no kitty keys | must be fully usable there — this is the floor |
| Resize storms | Bubble Tea coalesces `WindowSizeMsg` | debounce PTY resize by one frame so agents don't get 30 SIGWINCH per drag |

Test matrix for every release: kitty, Ghostty, iTerm2, WezTerm, Alacritty,
Terminal.app, tmux 3.4+, zellij 0.4x — each: colors, `gg`/`ctrl-\`, mouse
wheel, resize, INTERACT with `claude` and `codex`, `NO_COLOR=1`, `--icons ascii`.
