# bungkus-mcc — Architecture

Status: proposal, revision 2 (after critic round 1). Decisions are for stage
1 unless marked "later". Facts about Claude Code / Codex were checked on
2026-09-30 against Claude Code 2.1.285 and Codex CLI 0.153.4, the official
docs, and live experiments recorded in the review scratchpad (`sub/ev.log`:
real Claude hook payloads incl. subagents; `cx/ev.log`: Codex `-c` hook
injection; `merge/`: `--settings` hook merge; `sl/`: `--settings` statusLine
injection; `vt/`: x/vt behaviour; `dd/`: `--` prompt handling). "VERIFIED"
below means one of those. Anything marked ASSUMPTION or UNCONFIRMED is
re-verified at implementation time.

## 1. One-paragraph summary

bungkus-mcc is a single Go binary, a Bubble Tea v2 full-screen app. It
spawns each AI agent (`claude`, `codex`) as a child process in a PTY it
owns and renders that PTY through an embedded VT emulator in the right pane,
so the user gets the agent's real interactive TUI. It learns *structure*
(session state, subagents, current tool, waiting-for-input) and *usage*
(tokens, cost, context %, plan limits) not by parsing anything on screen or
on disk but from the agents' own **hook** and **status-line** extension
points: every hook runs `bungkus-mcc hook`, Claude's status line runs
`bungkus-mcc statusline`, and both forward a trimmed JSON line over a
per-process unix socket back into the TUI. mcc never parses agent
transcripts in stage 1.

## 2. How mcc sees subagents — options evaluated

| # | Option | Gives | Costs / breaks | Verdict |
|---|--------|-------|----------------|---------|
| 1 | **PTY + embedded VT emulator** (creack/pty + charmbracelet/x/vt) | The agent's real UI, permission prompts, colours, interactive input. Works for any CLI agent. | Zero structure. Emulator dependency, key re-encoding. x/vt has no semver tag. | **Use — live pane.** |
| 2 | **Tail transcripts** (`~/.claude/projects/<slug>/<sid>.jsonl` + `<sid>/subagents/agent-<id>.jsonl`; `~/.codex/sessions/…/rollout-*.jsonl`) | Full history, sessions mcc did not start. | Both formats are explicitly internal (Claude docs: "changes between versions … can break on any release"; Codex undocumented and mid-migration to SQLite via `codex migrate-rollouts`). Transcripts hold every secret the agent saw. | **Not in stage 1.** One narrowly scoped exception is *proposed* for Codex usage numbers (§6.3) and is an open question. |
| 3 | **Hooks → unix socket** (Claude `--settings`; Codex `-c hooks.*`) | Documented, structured, push-based; identical JSON shape in both agents (VERIFIED: `session_id, transcript_path, cwd, hook_event_name, agent_id, agent_type, tool_name, tool_input, tool_use_id, last_assistant_message, background_tasks`). | Only sessions mcc launched. Codex needs one-time hook trust. One process spawn per event. | **Use — structure.** |
| 4 | **Headless structured mode** (`claude -p --output-format stream-json`, `codex exec --json`, Codex app-server JSON-RPC) | Cleanest structured stream incl. usage. | Not the agent's interactive UI: mcc would have to build chat + permission UIs — a different product. | **Not for stage 1.** Later: optional "task" session kind. |
| 5 | **tmux/zellij panes** | Detach for free. | Two code paths, requires a multiplexer, mcc cannot draw inside another pane. | **No.** Run mcc *inside* tmux instead. |

Recommendation: **1 + 3** (plus Claude's status line for usage, §6).

## 3. Process model

```
 terminal (kitty / ghostty / tmux ...)
   │ raw mode, alt screen
   ▼
 ┌──────────────────────────────── bungkus-mcc (one process) ─────────────────────────────┐
 │  Bubble Tea program: the model owns all state                                            │
 │   ├─ projects pane   ├─ sessions pane   ├─ output pane (one vt.Emulator per session)    │
 │                                                                                          │
 │  goroutines (each only Program.Send()s a tea.Msg, except the pump):                      │
 │   • pty reader ×N   → ptyDataMsg{sess, []byte}                                            │
 │   • reply pump ×N   : io.Copy(ptmx, emu)   ← the one goroutine that touches an emulator  │
 │   • proc waiter ×N  → sessionExitedMsg{sess, code}  (after the reader has drained)        │
 │   • ipc accept loop → hookEventMsg / usageMsg                                              │
 │   • spinner tick    → tickMsg (only while something is running)                           │
 │   • update check    → updateAvailableMsg (copied from bungkus-cli)                         │
 │                                                                                          │
 │  unix socket  $BUNGKUS_MCC_SOCK = ${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/bungkus-mcc-<uid>/<pid>.sock │
 └───────────┬──────────────────────────────────────────┬───────────────────────────────────┘
             │ PTY; env += BUNGKUS_MCC_SOCK,             │ PTY; same env
             │        BUNGKUS_MCC_SESSION=<mcc id>        │
             ▼                                            ▼
   claude --session-id <uuid> --settings '{…}' -- <prompt>    codex -c 'hooks.PreToolUse=[…]' … -- <prompt>
             │ runs hooks + statusLine via `sh -c`          │ runs hooks via `sh -c`
             ▼                                              ▼
   bungkus-mcc hook | bungkus-mcc statusline ──one JSON line──▶ socket ──▶ ipc accept loop
   (exit 0 always; never blocks the agent beyond its own runtime)
```

- One mcc process, no daemon, no background service. Socket per pid so two
  mcc instances never cross-talk. If the socket directory cannot be created
  or verified (§7, SECURITY.md), or the path exceeds the 104-byte `sun_path`
  limit, mcc runs **without a socket**: every card is "output only" with the
  hint from DESIGN.md §10. The live pane never depends on the socket.
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
  host terminal. Advertising the host's `TERM` (e.g. `xterm-kitty`) would
  make agents emit kitty-only sequences the emulator does not implement;
- unset `TERM_PROGRAM`, `TERM_PROGRAM_VERSION`, `KITTY_WINDOW_ID`, `TMUX`,
  `TMUX_PANE`, `WEZTERM_*`, `ITERM_*` — same reason (agents probe these to
  pick notification and hyperlink strategies);
- add `BUNGKUS_MCC_SOCK`, `BUNGKUS_MCC_SESSION`, and for Claude
  `BUNGKUS_MCC_USER_STATUSLINE` (§6.2).

The emulator's default foreground/background come from
`tea.BackgroundColorMsg`/`ForegroundColor` (or the theme's reference values
when the host does not answer), so the agent's "default colour" text looks
like the host's.

### 3.2 What happens when mcc quits while agents run (stage 1)

**Stop them, remember them, offer resume.**

- `q` with running sessions → confirm dialog (DESIGN.md §5.5).
- Confirmed: SIGTERM to each session's process group (best effort — the
  agent may have re-parented helpers; we only signal the group we created,
  and only while the waiter has not yet reported exit), wait ≤ 3 s, SIGKILL,
  close PTYs. The socket stays open until the last child has exited so
  `SessionEnd` hooks are delivered and do not error. Claude runs SessionEnd
  on SIGTERM and persists its transcript; Codex persists continuously.
- Each session's `agentSessionId` is in `sessions.json`; `r` runs the resume
  argv (§5) in the stored `cwd`.
- SIGHUP/terminal close: same, without the dialog.

Why not detach: both agents now have daemons (`claude --bg` + `attach` /
`codex app-server daemon` + `codex agents`), which are the right long-term
answer and would make mcc a viewer. But `--bg` is weeks old, `attach` has
its own Ctrl+Z semantics inside a PTY we'd have to test, and Codex parity
means a JSON-RPC client. A mcc "session holder" daemon is a second process
with its own lifecycle — real complexity for a convenience. Kill + resume
loses only the in-flight turn. **Detach vs stop-on-quit remains an open
question for the product owner.**

## 4. Data flow

### 4.1 Live output

```
pty fd ──read(32 KiB)──▶ reader goroutine ──Program.Send(ptyDataMsg)──▶ Update(): emu.Write(data); return nil
                                                                            (Bubble Tea coalesces repaints)
emu (replies: DSR, DA, XTVERSION …) ──io.Copy(ptmx, emu)── pump goroutine   ← REQUIRED: x/vt blocks Write until
                                                                               its reply pipe is drained (VERIFIED:
                                                                               Write hung on CSI 6n without a reader)
View(): output pane = sanitise(emu.Render()) or, in NORMAL with scroll offset, sanitise(scrollback[off:] + screen)
```

- Rendering uses `Render()` (a string): `tea.View.Content` is a string
  (VERIFIED build with bubbletea v2.0.9 + x/vt pseudo-version 2026-09-27).
  Bubble Tea pins the `ultraviolet` version; x/vt must build against that
  same version — checked in CI by the build itself.
- **The pump is the single exception** to "an emulator is touched only in
  `Update`/`View`". x/vt's `Read`/`Write` sides are designed for concurrent
  use; nothing else about the emulator is shared.
- Resize: on `tea.WindowSizeMsg` (debounced one frame) **every** emulator is
  resized (`emu.Resize` + `pty.Setsize` → kernel sends SIGWINCH), not only
  the visible one, so a session you switch to is never stale.
- Scrollback: explicit cap of **10 000 lines per session** (x/vt
  scrollback). In NORMAL, `j/k/ctrl-d/ctrl-u/gg/G` change mcc's offset and
  the pane renders `scrollback[offset:] + screen`. When the agent is on the
  alternate screen (Codex's TUI; Claude Code is not), there is no scrollback
  to show and the hint `scroll inside the agent` appears. Whether to launch
  Codex with `--no-alt-screen` is tested in M3; default **off** (keep Codex's
  own behaviour).
- Input in INTERACT: Bubble Tea gives `tea.KeyPressMsg`. **x/vt has no kitty
  keyboard encoder** (it implements the terminal side, not the client side),
  so mcc translates keys itself with a small table: printable → UTF-8;
  `enter` → CR; `backspace` → DEL; arrows/home/end/pgup/pgdn → CSI or SS3
  depending on the emulator's DECCKM mode; `ctrl-x` → C0 byte; `alt-x` →
  ESC + x; `shift-enter` → ESC CR (what Claude Code accepts as "newline" —
  verify in M3); `tab`/`shift-tab` → HT / CSI Z; F-keys → CSI n~ / SS3.
  Paste → bracketed paste if the agent enabled mode 2004, else raw. Mouse →
  SGR encoding only if the agent enabled mouse reporting. `ctrl-z` is
  swallowed. A table test covers the table both ways.

### 4.2 Rendered-output sanitiser (allowlist)

`sanitise()` runs on everything that reaches the host terminal from an
untrusted source: the emulator's `Render()` output and (a simpler variant)
every string from hooks, prompts and directory names (SECURITY.md).

Allowed in emulator output: printable runes (incl. wide and combining),
`SGR` (`CSI … m`) only. Everything else is stripped: all other CSI, all OSC
— including OSC 8, which x/vt's `Render()` re-emits (VERIFIED) and which we
drop rather than filter by scheme in stage 1 — DCS, APC, PM, SOS, C1
controls, and raw C0 except `\n`. Hostile-sequence test corpus (the
emulator must neither leak them nor mis-render around them): OSC 52
clipboard, OSC 0/2 title, OSC 8 with `file:` and `javascript:`, OSC 1337
(iTerm2 file/image), OSC 9/99/777 notifications, kitty APC graphics
(`ESC _G … ESC \`), tmux DCS passthrough (`ESC P tmux; … ESC \`), XTWINOPS
(`CSI 21 t` title query), DECRQSS (`DCS $ q … ST`), DA/DSR/XTVERSION query
echoes, C1 bytes 0x80–0x9F, `ESC c` (RIS), `CSI ? 1049 h/l`.

Strings from hooks/prompts/dir names: `ansi.Strip`, then drop every rune
< 0x20 except `\t` (rendered as one space), 0x7F, and 0x80–0x9F; truncate.

### 4.3 Structure (hook events)

```
agent ──sh -c '<exe> hook'──▶ bungkus-mcc hook
                               reads stdin (cap 8 MiB, drains the rest), env BUNGKUS_MCC_SOCK/SESSION
                               keeps only the fields below, writes one JSON line to the socket, exit 0
ipc listener ──▶ Program.Send(hookEventMsg) ──▶ agent.Adapter.Reduce(session, event)
```

Session state machine (decided; the same for both agents, the adapter only
maps event names):

```
 launch/SessionStart ─► running? no: your turn (nothing happening yet is shown as "your turn")
 UserPromptSubmit, PreToolUse, PostToolUse, SubagentStart ─► running        (also clears needs-you)
 Notification{permission_prompt | agent_needs_input | elicitation_dialog}   ─► needs you
 PermissionRequest (codex)                                                  ─► needs you
 Notification{idle_prompt}                                                  ─► ignored
 Stop  with no running children (per background_tasks)                      ─► your turn
 Stop  with running children                                                ─► running
 SubagentStop ─► child wrapped; if it was the last child and the main turn already stopped ─► your turn
 process exit 0 ─► wrapped      exit ≠ 0 ─► failed (reason = last non-empty emulator line, sanitised)
 x ─► stopped
```

Subagents (flat list, no nesting in stage 1):

- `SubagentStart{agent_id, agent_type}` adds a child (VERIFIED fields; no
  description, no transcript path on Start).
- Description: the `PreToolUse` for `tool_name == "Agent"` (Claude) /
  `"spawn_agent"` (Codex, ASSUMPTION until M6 records it) carries
  `tool_input.description` and `tool_use_id`; the next unpaired
  `SubagentStart` takes it (FIFO). Parallel launches in one turn are the
  risky case for FIFO, which is why:
- **`background_tasks` is authoritative.** Every Claude `Stop` and
  `SubagentStop` carries `background_tasks: [{id, type: "subagent",
  agent_type, description, status}]` (VERIFIED). On each, the child list is
  reconciled to it: descriptions corrected, statuses set, unknown children
  added, children absent from the list and not `running` treated as done.
- `SubagentStop{agent_id, agent_transcript_path, last_assistant_message}`
  marks the child wrapped (`agent_transcript_path` exists only on Stop — VERIFIED).
- `PreToolUse`/`PostToolUse` carrying `agent_id` update that child's tool.
- meta.json enrichment: **dropped** (decided). Undocumented and unnecessary.

Session-id binding: Claude sessions get `--session-id <uuid>` chosen by mcc,
so `session_id` in every event is known in advance. Codex sessions bind on
the first event carrying their `BUNGKUS_MCC_SESSION`; after that the reducer
**ignores any event whose `session_id` differs** from the bound one (nested
`claude`/`codex` runs inherit the env and would otherwise pollute the card).
All ids are validated against `^[0-9A-Za-z-]{8,64}$` before use in argv or
file names.

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
    Transcript  string          // stored opaque for a later "open transcript" feature; never read
    AgentID     string
    AgentType   string
    AgentTranscript string      // SubagentStop only
    ToolName    string
    ToolUseID   string
    ToolDesc    string          // tool_input.description, ≤ 200 chars — the only tool_input field kept
    Notify      string          // notification_type (claude) | "permission_request" (codex)
    LastMessage string          // ≤ 200 chars
    BackgroundTasks []BackgroundTask // {ID, Type, AgentType, Description(≤200), Status}
}
// No Raw, no tool_response, no tool_input beyond description.

type Usage struct {             // from `bungkus-mcc statusline` (Claude) — see §6
    SessionID string
    In, Out, CacheRead, CacheWrite int64
    CostUSD   float64
    CtxUsedPct float64; CtxSize int64
    Limits    *Limits            // nil when the account has none (API key)
}
type Limits struct{ FiveHourPct, SevenDayPct float64; FiveHourResets, SevenDayResets time.Time }

type Adapter interface {
    Kind() Kind
    Launch(o LaunchOpts) (argv, env []string, err error) // new or resume; hooks/statusline wired to o.SockPath
    Parse(line []byte) (Event, error)                    // unknown events → Name only
    Reduce(s *Session, e Event)                          // state machine of §4.3
}
```

Two implementations, ~150 lines each; adding an agent = new file + one map
entry. No capability flags, no registry.

### 5.1 Claude (`claude.go`)

- **New:** `claude --session-id <uuid> --settings <json> -- <prompt?>`
- **Resume:** `claude --resume <id> --settings <json>` in the stored `cwd`
  (never together with `--session-id` — the combination errors; a unit test
  covers both argv forms).
- `<json>` is built with `encoding/json` and passed as one argv element. It
  contains only `hooks` (synchronous — `async` dropped: it would make
  ordering across events unreliable and the hook takes ~5 ms) and
  `statusLine` (§6.2). Hook command string =
  `'<os.Executable() → EvalSymlinks, POSIX single-quoted>' hook` — tested
  with a path containing a space and a `'`.
- Events subscribed: `SessionStart, UserPromptSubmit, PreToolUse,
  PostToolUse, SubagentStart, SubagentStop, Notification, Stop, SessionEnd`.
- `--settings` hooks **merge** with the user's own hooks (VERIFIED: the same
  `UserPromptSubmit` reached both a `--settings` hook and a project hook).
  Assumption closed.
- Claude runs hooks and the status line **only after the workspace-trust
  prompt has been answered** for that directory, and managed settings with
  `disableAllHooks` / `allowManagedHooksOnly` drop ours entirely. So: no
  event of any kind within 10 s → the card shows "output only" with a hint
  that names both causes ("answer the trust prompt in the pane, or hooks are
  disabled by policy"); the card leaves "output only" the moment *any* event
  arrives, however late.

### 5.2 Codex (`codex.go`)

- **New:** `codex -c 'hooks.<Event>=[{…}]' … -- <prompt?>` — per-launch
  hook injection via `-c` works (VERIFIED: `SessionStart` and `Stop`
  delivered). Events: `SessionStart, UserPromptSubmit, PreToolUse,
  PostToolUse, PermissionRequest, SubagentStart, SubagentStop, Stop,
  SessionEnd`.
- **Resume:** `codex resume <id>` with the same `-c` hooks, in the stored
  cwd. Never `resume --last`. If hooks are not trusted the card says
  `not resumable — hooks off` and `r` is disabled (we could resume blind,
  but a card with no state and a stale title is worse than a hint).
- Trust: Codex only runs hooks the user has approved once via `/hooks`
  (hash-based). **M6's first task** is to verify whether that approval
  persists for our (byte-stable) injected hooks across launches. If yes,
  `setup codex` is deleted and the hint "run `/hooks` once" is all we need.
  If not, `bungkus-mcc setup codex` merges an mcc block into
  `~/.codex/hooks.json` with these rules: round-trip via `map[string]any`
  (preserve unknown keys), abort on malformed JSON, write a `.bak`, keep the
  file mode, `EvalSymlinks` and write the temp file next to the target,
  idempotent on the exact command string, print the block and ask y/N,
  document manual removal in README. **Never pass
  `--dangerously-bypass-hook-trust`** (rule, SECURITY.md).
- Codex `notify` is not used (only `agent-turn-complete`, strictly less than
  `Stop`).
- **M6's second task**: record real `SubagentStart`/`SubagentStop`/
  `PreToolUse(spawn_agent)` payloads into `testdata/codex/` — the field set
  is documented but unverified here.

### 5.3 What is fragile and where it is contained

| Fragile | Blast radius | Containment |
|---------|--------------|-------------|
| Hook field renamed | Card shows less | every field optional; unknown events ignored; recorded-payload tests |
| FIFO description pairing | description on a sibling | corrected by `background_tasks` on the next Stop/SubagentStop |
| CLI flags | launch fails | flags exist only in `Launch`; CI smoke greps `--help` |
| Codex hook trust | Codex card "output only" | detection + hint; PTY unaffected |
| statusLine schema (usage) | usage shows `—` | all fields optional; Claude docs list the fields |
| x/vt rendering quirks | visual glitch | `ctrl-l`/`R` full repaint; the agent runs fine outside mcc |
| x/vt untagged API | build break on bump | one wrapper package; pinned pseudo-version; diff reviewed on bump |

## 6. Usage figures — sources, verified vs assumed

Requirement: tokens per session (and per subagent where obtainable), cost,
context fill %, plan rate limits. What exists without reading transcripts:

| Number | Claude Code 2.1.285 | Codex 0.153.4 |
|--------|---------------------|---------------|
| tokens (session) | **statusLine stdin** `context_window.total_input_tokens/total_output_tokens` (input **includes cache**; cumulative vs per-request UNCONFIRMED → M5), `current_usage{input, output, cache_creation_input, cache_read_input}` — VERIFIED present | interactive: only in the rollout `token_count` events (internal) or via app-server `thread/tokenUsage/updated` (not in stage 1); `codex exec --json turn.completed.usage` (VERIFIED) is headless only |
| tokens (per subagent) | not exposed by hooks or statusLine; only in the subagent transcript | same |
| cost | **statusLine** `cost.total_cost_usd` (client-side list price) — VERIFIED | none for non-Enterprise (Codex meters credits; USD only for eligible workspaces) |
| context % | **statusLine** `context_window.used_percentage` (+ `context_window_size`) — VERIFIED | rollout `token_count.info.model_context_window` (internal) / app-server |
| plan limits | **statusLine** `rate_limits.five_hour/seven_day{used_percentage, resets_at}` — VERIFIED on a subscription account; documented absent for API-key users | rollout `token_count.rate_limits{primary, secondary}` (internal, often null) / app-server `account/rateLimits/read` |
| hooks carry usage? | no (VERIFIED: Stop/PostToolUse/SubagentStop have none; only `SessionStart` on resume has `context_tokens`) | no |

### 6.1 Decision for stage 1

- **Claude: all four numbers from the status line**, delivered by the same
  socket. Per-subagent tokens: not shown (transcript-only).
- **Codex: `—`** on the card and no contribution to the global limits;
  Codex's own status line (which can show context and limits) is visible in
  the output pane because we render its TUI. A proper source is the
  app-server protocol (stage 2, with detach).
- **Proposed exception (open question, not planned):** tail only the
  `token_count` lines of the rollout file that Codex's own hook names in
  `transcript_path` — an internal format, so only with the owner's yes and a
  SECURITY.md review.

### 6.2 Claude status line delivery (`bungkus-mcc statusline`)

`--settings` sets `statusLine` for the session and **displaces the user's
own status line** (VERIFIED: with `--settings '{"statusLine":…}'` only our
command ran; precedence managed > `--settings` > local > project > user).
To keep the user's status line working:

1. **Resolve** the user's effective `statusLine` object at launch from
   `<cwd>/.claude/settings.local.json`, `<cwd>/.claude/settings.json`,
   `${CLAUDE_CONFIG_DIR:-~/.claude}/settings.json` (first hit wins).
   Edge cases: malformed JSON in a file = treated as no status line there;
   a resolved command that is itself our own `statusline` (a previous mcc
   session's `--settings` leaked into a file, or a user copying it) is
   dropped — recursion guard; managed settings are not readable by us and
   would override ours anyway (usage then shows `-`). Project/local
   settings are repo-controlled; running their command is acceptable
   because Claude's workspace-trust gate precedes any status-line execution
   and Claude would run the same command without mcc (SECURITY.md).
2. **Inject** `{"type":"command","command":"'<exe>' statusline",
   "padding": <user's>, "refreshInterval": <user's, or 30 when they had
   none>}`. 30 s keeps `rate_limits` fresh between assistant messages; it
   also means the user's own command now runs every 30 s where it may have
   run only on events before — documented, and `refreshInterval: 0` in their
   settings is honoured as "events only".
3. **Run**: `bungkus-mcc statusline` reads stdin up to 1 MiB into a buffer.
   Concurrently, with a **≤ 200 ms budget** that never delays the user's
   line, it forwards a `Usage` line (`session_id, cost, context_window,
   rate_limits` — nothing else; `model.id` dropped) to the socket. Then it
   runs the user's command with `exec.Command("sh", "-c", cmd)`,
   `Stdin = bytes.NewReader(buf)`, `Stdout = os.Stdout`, `Stderr` discarded,
   and exits with its status. It runs under `sh`; whether Claude itself
   uses `sh` or the user's `$SHELL` is an M5 check (match it if they
   differ). No user command → print nothing.
   If stdin exceeds 1 MiB it is passed through to the user's command
   unchanged and **not forwarded**.
4. Same silence rules as `hook` (no stdout of our own, exit 0).

Token semantics (M5 records a multi-turn session to settle them):
`context_window.total_input_tokens` **includes cache** reads/writes and may
be per-request rather than cumulative; `used_percentage`/`current_usage`
are `null` before the first reply (→ `-`); `context_window_size` varies
(200k and 1M observed). Limits whose `resets_at` has passed without a
fresher report are shown dimmed as stale (DESIGN.md §6.1).

## 7. State and config storage

Paths follow XDG on both macOS and Linux (dotdirs in `$HOME`, like the
agents; `os.UserConfigDir` on macOS would be `~/Library/Application
Support`, wrong for a terminal tool).

| What | Path | Mode |
|------|------|------|
| config | `${XDG_CONFIG_HOME:-~/.config}/bungkus/mcc/config.json` | 0600, dir 0700 |
| state | `${XDG_STATE_HOME:-~/.local/state}/bungkus/mcc/sessions.json` | 0600, dir 0700 |
| debug log (`--debug` / `BUNGKUS_MCC_DEBUG=1` only) | `${XDG_STATE_HOME:-~/.local/state}/bungkus/mcc/mcc.log` | 0600 |
| socket | `filepath.Clean(${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}})/bungkus-mcc-<uid>/<pid>.sock` | dir 0700, sock 0600 |
| update-check cache | `os.UserCacheDir()/bungkus-mcc/latest-release` (as bungkus-cli) | 0600 |

`config.json` (all keys optional; the workspace can also be given as the
first CLI argument, `bungkus-mcc ~/Works`):

```json
{
  "workspace": "/Users/me/Works/OSBR",
  "theme": "auto",
  "icons": "unicode",
  "mouse": true,
  "notify": "bell",
  "interactExit": "ctrl-\\",
  "agents": {
    "claude": { "command": "claude", "args": [] },
    "codex":  { "command": "codex",  "args": [] }
  }
}
```

`sessions.json` (array; atomic temp + rename; the only persisted state —
the per-session event log was cut in review):

```json
[{ "id": "m-7c5d", "agent": "claude", "project": "kedai-web",
   "cwd": "/Users/me/Works/OSBR/kedai-web", "agentSessionId": "5f1c…",
   "transcript": "/Users/me/.claude/projects/…/5f1c….jsonl",
   "title": "write proposal", "status": "wrapped",
   "startedAt": "2026-09-30T13:41:02+09:00", "endedAt": "…",
   "toolCalls": 38, "subagents": 2,
   "usage": { "in": 71000, "out": 13000, "costUsd": 1.42, "ctxPct": 37 } }]
```

Workspace = one directory; projects = its direct child directories
(dot-dirs skipped, symlinks followed, names sanitised for display), sorted
by name. No recursion, no project file. Deleting the state dir loses the
session list and nothing else.

## 8. Concurrency rules

1. The model is the only state. Goroutines own I/O and communicate through
   `Program.Send`.
2. `vt.Emulator` instances are owned by the model and touched only in
   `Update`/`View` — **except the reply pump** (`io.Copy(ptmx, emu)`) per
   session, which exists because x/vt blocks `Write` until its replies are
   read (VERIFIED). Plain `Emulator`, not `SafeEmulator`.
3. PTY readers block on `Send` when the UI is busy — that is the
   backpressure. 32 KiB chunks.
4. The waiter first waits for the reader goroutine to drain (EOF), then
   sends `sessionExitedMsg`, so the final screen is complete before the
   state flips to wrapped/failed.
5. Socket: one goroutine per connection, one line, 2 s deadline, then close.
6. No `time.Sleep` in the model; timers are `tea.Tick`; the spinner tick is
   scheduled only while something is running.
7. Shutdown (§3.2) runs as a `tea.Cmd` so the UI can show "stopping…";
   `tea.Quit` only after it completes.

## 9. Package layout

```
main.go, version.go            # Version via -ldflags (as bungkus-cli)
cmd/
  root.go                      # TUI; optional workspace arg; --debug, --icons, --no-mouse, --theme
  hook.go                      # `hook`: stdin → trimmed line → socket; silent; exit 0
  statusline.go                # `statusline`: same, plus exec of the user's status line
  setup.go                     # `setup codex` (only if M6 proves it necessary)
  update.go                    # copied from bungkus-cli (header: // copied from osbrjp/bungkus-cli@<sha> cmd/update.go)
internal/
  theme/    theme.go           # Daun Pisang tokens (hex + 256 + 16), Icon(), styles — the file future products copy
  tui/      app.go keymap.go projects.go sessions.go output.go dialogs.go firstrun.go
            testdata/          # goldens (120×40, 80×24), hostile streams
  agent/    agent.go claude.go codex.go   testdata/{claude,codex}/*.json
  term/     session.go keys.go sanitise.go  # pty + emulator + pump + key table + allowlist
  ipc/      server.go client.go
  store/    paths.go config.go state.go
  workspace/scan.go
```

## 10. Modes and keys

Two input modes, one `mode` field: NORMAL (keys are mcc's; DESIGN.md §8)
and INTERACT (keys go to the focused session's PTY; exit chord from config,
default `ctrl-\`; `ctrl-z` swallowed; a click outside the output pane
exits). FILTER is NORMAL with a text input capturing printable keys. `gg`
is the only sequence (a `pendingG` flag). The keymap is one struct of
`key.Binding`s grouped per pane and mode; help, the getah bar and the
per-pane uniqueness test are generated from it.

## 11. Relationship with bungkus-cli

| Option | Pros | Cons | Stage 1? |
|--------|------|------|----------|
| A. Separate repo + binary, **copy** the ~350 shareable lines (theme, update, install.sh, workflows) | zero coupling, own cadence, same tooling | two copies drift | **Yes** |
| B. Shared module `bungkus-kit` | one source of truth | third repo to tag/release; every palette tweak = release + two bumps | when a 3rd product exists |
| C. `bungkus-cli mc` subcommand | one install | PTY/VT/socket stack inside a scaffolder; entangled release risk | no |
| D. Monorepo | shared code | breaks bungkus-cli's repo/CI/semantic-release conventions | no |
| E. `bungkus-cli mcc …` exec shim (git/kubectl style) | discoverability, ~20 lines | it is a shim | after mcc's first stable release |

**Recommendation: A now, E later, B only with a third product.**

Copied code carries `// copied from osbrjp/bungkus-cli@<sha> <path>` and
both SECURITY.md files state that updater fixes apply to both repos
(bungkus-cli's SECURITY.md gets that line in a follow-up PR). Reused by
copy: `pkg/update.go`, `cmd/update.go`, `install.sh` (fetched at the
resolved release tag, see SECURITY.md), the four workflows,
`.releaserc.json`, conventions. Not reused: registry/templates/wizard.

Integration (stage 2, decided): `N` "new project" runs `bungkus-cli`'s
wizard in the output pane with cwd = workspace and rescans on exit 0 —
zero library coupling. Stage 1 ships without it.

## 12. Terminal compatibility strategy

| Concern | Handled by | Our part |
|---------|-----------|----------|
| colour depth, `NO_COLOR`, `CLICOLOR_FORCE`, tmux RGB | Bubble Tea v2 + colorprofile | declared 256/16 values per token (DESIGN.md §2) |
| kitty keyboard vs legacy | Bubble Tea negotiates for *our* input | never bind protocol-only keys; exit chord is a C0 byte; our own key table for the child (§4.1) |
| mouse | Bubble Tea | `mouse: false`; modifier-drag for selection documented |
| synchronized output | Bubble Tea | — |
| OSC 8 | Lip Gloss (header path only) | stripped from agent output |
| OSC 52 | Bubble Tea cmd | `y`; hint when unsupported |
| Nerd Font | undetectable | `icons` setting, default unicode |
| light/dark | `BackgroundColorMsg` | `theme` setting |
| notifications | OSC 9/99/777 by terminal, BEL fallback; title via OSC 2 with XTWINOPS push/pop | `notify` setting |
| tmux / zellij | `TERM=tmux-256color`; OSC 8 ≥ 3.4; kitty keys need `extended-keys` | test matrix; no DCS passthrough needed |
| Apple Terminal | 256 colours (truecolor from macOS 26 UNCONFIRMED), no OSC 8, no kitty keys | must be fully usable — this is the floor |
| resize storms | Bubble Tea coalesces | debounce PTY resize by one frame |

Release test matrix: kitty, Ghostty, iTerm2, WezTerm, Alacritty,
Terminal.app, tmux 3.4+ (plain **and** with vim-tmux-navigator), zellij
0.4x (default and lock mode), VS Code and Cursor integrated terminals, Warp;
keyboards: US, **JIS**, **German**. Each: colours, `gg`/exit chord, mouse
wheel, resize, INTERACT with `claude` and `codex`, `NO_COLOR=1`,
`--icons ascii`, BEL/desktop notification.
