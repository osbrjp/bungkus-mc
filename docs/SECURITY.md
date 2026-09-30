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
3. The agent processes it spawns — they run *as the user*, can do anything
   the user can, and are driven by an LLM that reads untrusted repository
   content. mcc does not and cannot sandbox them; that is the agents' own
   job (Codex sandbox, Claude Code permissions).
4. Bytes the agents write to their PTYs (rendered by mcc) and JSON the
   agents' hooks send to mcc's socket — **untrusted input**.
5. Other local users on the machine (multi-user hosts) — must not read
   mcc's socket or state.

What we protect: the user's terminal (no escape-sequence injection into the
*host* terminal), the user's files (no writes outside mcc's own dirs and
nothing the user did not ask for), the user's secrets (never copied into
mcc state), and the mcc process itself (no code paths where agent output
controls what mcc executes).

### Assets and threats

| Asset / surface | Threat | Control |
|-----------------|--------|---------|
| Spawning agents | Command/argument injection via project name or prompt | `exec.Command` with an argv slice, never a shell. Agent command names come from config (`agents.<kind>.command`) and are resolved with `exec.LookPath`; the prompt is a single positional argument. Project directories are only ever passed as `cwd`, never interpolated |
| Workspace / project selection | Path traversal, symlink escape, launching in an unintended directory | Workspace is an absolute, cleaned path chosen through the picker or config. Projects are *direct* children of the workspace: `filepath.Join(workspace, name)` where `name` passed `ValidateProjectName` (single path segment, no `..`, no separators — same rule as bungkus-cli's `pkg/validate.go`). Symlinked children are shown but resolved with `filepath.EvalSymlinks` and rejected if they leave the workspace |
| Agent output rendering | Terminal escape-sequence injection: an agent (or a repo file it prints) emits sequences intended for the *host* terminal — OSC 52 clipboard writes, title changes, DCS/APC payloads, `\e[?1049` alt-screen toggles, hyperlinks with `file:`/`javascript:` schemes | All PTY bytes go through the VT emulator (`x/vt`), which interprets them into a cell grid. mcc renders cells, never raw bytes. The emulator's callbacks for OSC 52 (clipboard), OSC 0/2 (title), OSC 8 (links), bell and DCS are **not** forwarded to the host in stage 1 (clipboard/title from the agent are dropped; the pane title shows mcc's own text). A table test feeds known-hostile sequences and asserts the rendered output contains no ESC/OSC/DCS bytes other than those lipgloss emits |
| Hook socket | Another local process injects fake events (spoof "wrapped", hide "needs you"); DoS with large payloads; symlink races on the socket path | Socket directory `${XDG_RUNTIME_DIR:-$TMPDIR}/bungkus-mcc-<uid>/` created 0700 and verified (`Lstat`: owned by uid, mode 0700, not a symlink) before listening; socket file 0600 (umask 0177 around `Listen`); per-process name `<pid>.sock`; the path is handed to children via env, not discoverable through a fixed well-known name. Listener: read ≤ 1 MiB per connection with a 2 s deadline, one line, then close; malformed JSON dropped and counted. Events only *change display state*; nothing mcc does on an event executes anything |
| Hook payload contents | Payloads carry `tool_input` (may include command lines, file contents) and `last_assistant_message` — may contain secrets the agent saw | Only the fields listed in ARCHITECTURE.md §5 are kept in memory; `Raw` is discarded after parsing (kept only in `--debug` logs, see below). The per-session event log on disk stores `Name, SessionID, AgentID, AgentType, ToolName, Notify, timestamps` and a 200-char truncated `LastMessage` — never `ToolInput`. Files 0600 |
| Transcript files | Reading them would put every secret the agent ever saw through mcc | **mcc does not read transcripts in stage 1.** It stores `transcript_path` as an opaque string for the user's `o` key. A later history adapter must be reviewed against this document before merging |
| Environment passthrough | mcc leaks its own vars, or strips ones the agents need (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, proxies) | Children inherit `os.Environ()` unchanged plus `BUNGKUS_MCC_SOCK` and `BUNGKUS_MCC_SESSION`. mcc never reads, logs or displays the environment. `--debug` logs never include env |
| Config and state files | Tampering, world-readable state | JSON under `~/.config/bungkus/mcc` and `~/.local/state/bungkus/mcc`, dirs 0700, files 0600, atomic writes (temp + rename). Config values are validated on load (paths absolute, enum fields exact); a bad config is reported and replaced by defaults in memory, never auto-rewritten |
| Codex `setup codex` | Silently modifying the user's `~/.codex/hooks.json` | Prints the exact JSON block it will add, asks y/N, writes atomically, never removes or reorders existing entries. Nothing in mcc modifies `~/.claude/settings.json` (Claude hooks are session-scoped via `--settings`) |
| Update check / self-update | MITM, unverified binary | Identical to bungkus-cli: HTTPS to `api.github.com` (release tag only, 3 s timeout, once per day, `BUNGKUS_NO_UPDATE_CHECK` disables); `bungkus-mcc update` re-runs the checksum-verified `install.sh` |
| Debug log | Secrets in logs | Off by default; `--debug` writes to a 0600 file; documented as "may contain hook payloads — delete after sharing"; never written to the terminal |
| Signals / child lifetime | Orphaned agents keep running invisibly after a crash | Children are in their own session (setsid) and receive SIGHUP when the PTY master closes; on normal quit mcc sends SIGTERM then SIGKILL after 3 s. A crash of mcc still closes the PTY (kernel), so agents get SIGHUP. Nothing is left running without a terminal by design (stage 1 has no detach) |

### Explicitly out of scope

- Sandboxing or restricting what the agents do — configure the agents.
- Protecting against a malicious *user* — mcc runs as them.
- Multi-tenant or remote use.

## Rules for contributors

- No shell. `exec.Command(name, args...)` only; never `sh -c` with user or
  agent-derived strings. (Exception: `update.go`, copied from bungkus-cli,
  runs a fixed hardcoded pipeline.)
- No raw agent bytes reach `os.Stdout`. Everything goes through the emulator
  → cells → lipgloss.
- New file paths: absolute, `filepath.Clean`, under one of the three mcc
  directories or the workspace; add a `ValidateProjectName`-style test.
- New socket messages: fields optional, size-bounded, no side effects beyond
  UI state.
- New persisted field: ask "could this contain a secret?" — if yes, don't
  persist it, or truncate and document.
- No new network calls without updating "External communication" below.
- No telemetry, ever.

## External communication

- **Host:** `https://api.github.com/repos/osbrjp/bungkus-mcc/releases/latest`
  (hardcoded, HTTPS only). **Trigger:** once per day on start when stderr is a
  terminal and `BUNGKUS_NO_UPDATE_CHECK` is unset; and `bungkus-mcc update
  --check`. **Auth:** none. **Client policy:** 3 s (background) / 10 s
  (explicit) timeout, no retries, Go TLS defaults.
- `bungkus-mcc update` runs `curl -fsSL <install.sh> | bash`, which downloads
  the release asset and `checksums.txt` from `github.com` and verifies SHA-256
  before installing — same script family as bungkus-cli.
- The spawned agents make their own network calls under their own
  configuration; that is outside this tool's control.

## Dependency management & remediation policy

Same policy as bungkus-cli:

- **Inventory.** `go.mod` + `go.sum` are authoritative; `go.sum` hashes are
  verified against the Go checksum database; all module paths are
  fully-qualified public repositories.
- **Scanning.** `govulncheck ./...` runs in CI on every push and fails the
  build on any advisory affecting called code.
- **Updates.** Bumped when `govulncheck` flags an advisory or during periodic
  review; Charm modules are bumped in lockstep with bungkus-cli.
- **Remediation windows** (advisory severity, called code): Critical ≤ 7 days;
  High ≤ 30 days; Moderate/Low ≤ 90 days. Uncalled modules: next routine bump.
- **Untagged dependency.** `charmbracelet/x/vt` is pinned to a pseudo-version;
  it is reviewed (diff read) on every bump because it parses untrusted bytes.

## Dangerous functionality (summary for reviewers)

- **Subprocess execution** — `internal/term/session.go` (agents, `bungkus-cli`
  for "new project"), `cmd/update.go` (installer). All argv-based except the
  fixed installer pipeline.
- **PTY / raw terminal** — `internal/term`, Bubble Tea. Restored on exit and
  on panic (deferred `Program.Kill`/`ReleaseTerminal`).
- **Unix socket server** — `internal/ipc/server.go`. Permissions and limits
  as above.
- **Filesystem writes** — `internal/store` (config/state/log, own dirs only),
  `cmd/setup.go` (`~/.codex/hooks.json`, opt-in).
- **Signals to other processes** — `internal/term` sends SIGTERM/SIGKILL only
  to process groups it created.

## Risky components

To be reviewed against `go.mod` at the first release and quarterly
thereafter; record the date and `govulncheck` result here, as bungkus-cli does.
