# Security

bungkus-mcc is a local, single-user terminal application. It runs with the
invoking user's own OS privileges, spawns AI coding agents (`claude`,
`codex`) as child processes with those same privileges, listens on a
private unix socket for events from those children, and stores a small
amount of state under the user's home directory. It exposes no network
service and sends nothing anywhere except the release check described
below. This document records the security-relevant aspects of the tool
itself, in the same format as bungkus-cli's SECURITY.md.

## Reporting a vulnerability

Open a private security advisory on the GitHub repository
(`osbrjp/bungkus-mcc`), or email the maintainer. Please do not file public
issues for undisclosed vulnerabilities.

## Threat model

Trust boundaries, most to least trusted:

1. The user and their shell environment.
2. bungkus-mcc's own binary and config.
3. The agent processes it spawns — they run *as the user* and are driven by
   an LLM reading untrusted repository content. mcc does not and cannot
   sandbox them; that is the agents' job (Codex sandbox, Claude permissions).
4. Bytes the agents write to their PTYs, JSON their hooks and status line
   send to mcc's socket, prompt text, and directory names in the workspace —
   **untrusted input**.
5. Other local users on the machine — must not read mcc's socket or state.

**Out of scope, explicitly:** an attacker running as the *same uid*. The
socket has no peer-credential check beyond directory/file permissions and
no HMAC; a same-uid process can already read the user's `~/.claude` and
`~/.codex`, which is strictly more than it could learn from mcc.

Secrets goal, restated: **mcc's state on disk is a 0600 subset of what the
agents already store.** No `tool_input` beyond a 200-char description, no
`tool_response`, no transcript contents; titles and messages truncated.

### Assets and threats

| Surface | Threat | Control |
|---------|--------|---------|
| Spawning agents | Argument injection via prompt or project name | `exec.Command` with an argv slice, never a shell. Agent commands come from config and are resolved with `exec.LookPath`. The prompt is one positional argument after `--` (verified accepted by both CLIs). Project directories are only ever `cwd`. Session ids used in argv match `^[0-9A-Za-z-]{8,64}$` |
| Workspace / project selection | Launching in an unintended directory | Workspace is an absolute cleaned path from CLI arg, config or the first-run input. Projects are the direct child directories (`os.ReadDir`), dot-dirs skipped, symlinks followed, `cwd = filepath.Join(workspace, entry.Name())`. Names are sanitised for display (below); no other validation is needed because a name is never used for anything but `Join` |
| **All strings not from the PTY** (hook fields, prompt/title, directory names, agent versions) | Terminal escape injection through a card, the header or a dialog | One `sanitise()` (`ansi.Strip`, then drop every rune < 0x20 except `\t`→space, 0x7F, 0x80–0x9F; truncate) applied in each adapter's `Parse`, in `workspace.Scan` and on the prompt before it becomes a title. The hostile-sequence test corpus is rendered through cards and the header, not only the output pane |
| Agent output rendering | Escape-sequence injection into the *host* terminal (OSC 52 clipboard, title, OSC 8 with `file:`/`javascript:`, OSC 1337, kitty APC, tmux DCS passthrough, XTWINOPS, DECRQSS echo, C1, alt-screen toggles) | All PTY bytes go through the VT emulator into a cell grid; the rendered string passes an **allowlist** (printable runes + `CSI…m`; everything else, including OSC 8 which the emulator re-emits, is stripped). Emulator callbacks for clipboard/title/bell/notifications are not forwarded. Corpus and oracle are in ARCHITECTURE.md §4.2 |
| Hook / status-line socket | Spoofed events from another process; oversized payloads; symlink races | Directory `filepath.Clean(${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}})/bungkus-mcc-<uid>/` created 0700; before listening: `Lstat` — must be a directory, owned by our uid, mode 0700, not a symlink; socket 0600 (umask 0177 around `Listen`); per-process name `<pid>.sock`. **If any check fails, or the path exceeds 104 bytes, mcc runs without a socket** (cards "output only") rather than listening somewhere weaker. Listener: ≤ 1 MiB per connection, 2 s deadline, one line, close; malformed JSON dropped. Events change display state only |
| Hook subcommands (`hook`, `statusline`) | Agents run hook commands through `sh -c`; a bad quoting of our path could execute something else; our output could corrupt the agent's UI | Command string = `os.Executable()` → `EvalSymlinks` → POSIX single-quoted (`'` → `'\''`) + ` hook`; settings JSON built with `encoding/json`, passed as one argv element; tested with a path containing a space and `'`. Both subcommands: `SilenceUsage`, `SilenceErrors`, no update check, `recover()` → exit 0, never write stdout/stderr (the `statusline` wrapper writes only the user's command's output). stdin capped at 8 MiB (hook) / 1 MiB (statusline) and drained |
| Hook payload contents | `tool_input`, `tool_response`, `last_assistant_message` may contain secrets the agent saw | The `hook` subcommand forwards only: event name, session/prompt/agent ids, agent type, tool name/use id, `tool_input.description` (≤ 200 chars), notification type, `last_assistant_message` (≤ 200), `background_tasks` (id/type/agent_type/status/description ≤ 200), `transcript_path`, `cwd`. Nothing else leaves the hook process. Debug log records event name + session + byte count only |
| Status-line payload (new data flow) | Account usage/limit figures and cost pass through mcc; the user's own status-line command is run by our wrapper | Forwarded fields: `session_id, cost, context_window, rate_limits` only (`model.id` dropped); stdin over 1 MiB is passed through and not forwarded; the forward has a 200 ms budget and never delays the user's line. The user's command is resolved from *their* settings files (`CLAUDE_CONFIG_DIR` honoured; malformed JSON = none; a command that is our own `statusline` is dropped — recursion guard), never from the agent or the socket, and run with `exec.Command("sh","-c",cmd)` with the buffered stdin, stdout inherited, stderr discarded — the same command Claude Code would have run. Project/local settings files are repo-controlled; running their command is acceptable because Claude's workspace-trust gate precedes any hook or status-line execution, so mcc never runs a repo command Claude would not. Persisted: a per-session usage subset (`in, out, costUsd, ctxPct`) in `sessions.json` (0600); limits in memory only |
| Transcript files | Reading them would put every secret the agent ever saw through mcc | **Not read in stage 1.** `transcript_path` is stored as an opaque string. The proposed Codex `token_count` exception (ARCHITECTURE.md §6.1) is off by default, reads only lines of one type from one file named by the agent's own hook, and must be reviewed against this document before merging |
| Environment passthrough | Leaking mcc vars; stripping vars the agents need | Children inherit `os.Environ()` plus `BUNGKUS_MCC_*`, with `TERM`/`COLORTERM` set and the host-terminal identity vars unset (ARCHITECTURE.md §3.1). mcc never reads, logs or displays the environment |
| Config and state files | Tampering; world-readable | JSON under `~/.config/bungkus/mcc` and `~/.local/state/bungkus/mcc`, dirs 0700, files 0600, atomic writes (temp + rename). Values validated on load; a bad config is reported and replaced by defaults in memory, never auto-rewritten |
| `setup codex` (only if M6 proves it necessary) | Silent modification of `~/.codex/hooks.json` | Round-trips `map[string]any`, aborts on malformed JSON, writes `.bak`, keeps the file mode, `EvalSymlinks` and writes the temp file next to the target, idempotent on the exact command string, prints the block and asks y/N, documents manual removal. Nothing in mcc modifies `~/.claude/settings.json` |
| Hook trust bypass | Running untrusted hooks | **Rule: never pass `--dangerously-bypass-hook-trust`** (or any `--dangerously-*` flag) to Codex; never `--dangerously-skip-permissions` to Claude. Trust is the user's decision inside the agent |
| Update check / self-update | MITM, tampered binary | HTTPS to `api.github.com` (release tag only; 3 s timeout; once a day; `BUNGKUS_NO_UPDATE_CHECK` disables). The cached tag is validated as semver before it is displayed (cache file 0600). `bungkus-mcc update` fetches `install.sh` **at the resolved release tag** (`raw.githubusercontent.com/osbrjp/bungkus-mcc/<tag>/install.sh`), which downloads the asset and `checksums.txt` for that tag and verifies SHA-256. Control: TLS + integrity checksum; **no signature** (same as bungkus-cli). Updater code is copied from bungkus-cli; fixes to it apply to both repos |
| Debug log | Secrets in logs | Off by default; `--debug` writes a 0600 file with event names, sizes and errors — no payload bodies, no env |
| Signals / child lifetime | Orphaned agents after a crash; signalling the wrong process | Children are in their own session and get SIGHUP when the PTY master closes (also on a mcc crash). On quit: SIGTERM then SIGKILL after 3 s to the process group we created, best effort, only before the waiter reports exit. No detach in stage 1, so nothing runs without a terminal |

### Explicitly out of scope

- Sandboxing or restricting what the agents do — configure the agents.
- Protecting against a malicious *user* or same-uid process — mcc runs as them.
- Multi-tenant or remote use.

## Rules for contributors

- No shell in `exec.Command` for anything mcc decides. Exceptions, each
  fixed and reviewed: `update.go` (copied installer pipeline) and the
  `statusline` wrapper exec'ing the user's own status-line command.
- No raw agent bytes reach `os.Stdout`: emulator → cells → allowlist →
  lipgloss. No string from a hook, prompt or directory name reaches the
  screen without `sanitise()`.
- New file paths: absolute, `filepath.Clean`, under one of mcc's three
  directories or the workspace.
- New socket fields: optional, size-bounded, no side effects beyond UI state.
- New persisted field: "could this contain a secret?" — if yes, don't
  persist, or truncate and document.
- No new network calls without updating "External communication".
- No `--dangerously-*` flags to agents. No telemetry, ever.
- Copied code carries `// copied from osbrjp/bungkus-cli@<sha> <path>` so
  security fixes can be mirrored.

## External communication

- **Host:** `https://api.github.com/repos/osbrjp/bungkus-mcc/releases/latest`
  (hardcoded, HTTPS only). **Trigger:** once a day on start when stderr is a
  terminal and `BUNGKUS_NO_UPDATE_CHECK` is unset; `bungkus-mcc update
  --check`. **Auth:** none. **Client policy:** 3 s (background) / 10 s
  (explicit) timeout, no retries, no redirects followed, Go TLS defaults.
- `bungkus-mcc update` runs `curl -fsSL <install.sh at tag> | bash`, which
  downloads the release asset and `checksums.txt` from `github.com` for that
  tag and verifies SHA-256 before installing.
- The spawned agents make their own network calls under their own
  configuration; outside this tool's control.

## Dependency management & remediation policy

Same policy as bungkus-cli:

- **Inventory.** `go.mod` + `go.sum` are authoritative; hashes verified
  against the Go checksum database; all module paths fully-qualified public
  repositories.
- **Scanning.** `govulncheck ./...` in CI on every push, failing on any
  advisory affecting called code.
- **Updates.** Bumped when `govulncheck` flags an advisory or during periodic
  review; Charm modules in lockstep with bungkus-cli.
- **Remediation windows:** Critical ≤ 7 days; High ≤ 30 days; Moderate/Low
  ≤ 90 days. Uncalled modules: next routine bump.
- **Untagged dependency.** `charmbracelet/x/vt` is pinned to a
  pseudo-version and its diff is read on every bump, because it parses
  untrusted bytes.

## Dangerous functionality (summary for reviewers)

- **Subprocess execution** — `internal/term/session.go` (agents, argv);
  `cmd/update.go` (fixed installer pipeline via `bash -c`);
  `cmd/statusline.go` (the user's own status-line command via `sh -c`).
  **Agents themselves run our `hook`/`statusline` commands via `sh -c`**,
  which is why the executable path is single-quoted and tested.
  Stage 2: `exec.LookPath("bungkus-cli")` for "new project".
- **PTY / raw terminal** — `internal/term`, Bubble Tea. Restored on exit and
  on panic.
- **Unix socket server** — `internal/ipc/server.go`; permissions, limits and
  the fail-closed-to-no-socket rule as above.
- **Filesystem writes** — `internal/store` (own dirs only);
  `cmd/setup.go` (`~/.codex/hooks.json`, opt-in, only if it survives M6).
- **Signals to other processes** — `internal/term`, own process groups only.

## Risky components

To be reviewed against `go.mod` at the first release and quarterly
thereafter; record the date and `govulncheck` result here, as bungkus-cli does.
