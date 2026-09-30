# Security

bungkus-mcc is a local, single-user terminal application. It runs with the
invoking user's own OS privileges, spawns AI coding agents (`claude`,
`codex`) as child processes with those same privileges, listens on a
private unix socket for events from those children, reads one kind of
record from Codex's session log, stops the agents' descendant processes on
quit, and stores a small amount of state under the user's home directory.
It exposes no network service and sends nothing anywhere except the
release check and — only when the user opts in — the model-routing
request described below. This document records the security-relevant
aspects of the tool itself, in the same format as bungkus-cli's SECURITY.md.

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
   sandbox them; that is the agents' job.
4. Bytes the agents write to their PTYs, JSON their hooks and status line
   send to mcc's socket, lines in Codex's session log, prompt text,
   directory names in the workspace, and process names in the process
   table — **untrusted input**.
5. Other local users on the machine — must not read mcc's socket or state.

**Out of scope, explicitly:** an attacker running as the *same uid*. The
socket has no peer-credential check beyond directory/file permissions and
no HMAC; a same-uid process can already read the user's `~/.claude` and
`~/.codex`, which is strictly more than it could learn from mcc.

Secrets goal, restated: **mcc's state on disk is a 0600 subset of what the
agents already store.** No `tool_input` beyond a 200-char description, no
`tool_response`, no transcript text; names and messages truncated.

### Assets and threats

| Surface | Threat | Control |
|---------|--------|---------|
| Spawning agents | Argument injection via prompt, name or project | `exec.Command` with an argv slice, never a shell. Agent commands come from config and are resolved with `exec.LookPath`. The prompt is one positional argument after `--`; the name is one `--name` value; model ids come from config and match `^[A-Za-z0-9._:-]{1,64}$`. Project directories are only ever `cwd`. Session ids used in argv match `^[0-9A-Za-z-]{8,64}$` |
| Workspace / project selection | Launching in an unintended directory | Workspace is an absolute cleaned path from CLI arg, config or the first-run input. Projects are direct child directories containing `CLAUDE.md`, `AGENTS.md` or `.git` (`os.ReadDir` + three `Lstat`s), dot-dirs skipped, symlinks followed, `cwd = filepath.Join(workspace, entry.Name())` |
| **All strings not from the PTY** (hook fields, prompt/name, directory names, process names, agent versions) | Terminal escape injection through a card, the header or a dialog | One `sanitise()` (`ansi.Strip`, then drop every rune < 0x20 except `\t`→space, 0x7F, 0x80–0x9F; truncate) applied in each adapter's `Parse`, in `workspace.Scan`, in `proc.Scan`, and on the prompt/name before display. The hostile-sequence corpus is rendered through cards, header and dialogs, not only the output pane |
| Agent output rendering | Escape-sequence injection into the *host* terminal | All PTY bytes go through the VT emulator into a cell grid; the rendered string passes an **allowlist** (printable runes + `CSI…m`; everything else, including OSC 8 which the emulator re-emits, is stripped). Emulator callbacks for clipboard/title/bell/notifications are not forwarded. Corpus and oracle in ARCHITECTURE.md §4.2 |
| Hook / status-line socket | Spoofed events; oversized payloads; symlink races | Directory `filepath.Clean(${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}})/bungkus-mcc-<uid>/` created 0700; before listening: `Lstat` — a directory, owned by our uid, mode 0700, not a symlink; socket 0600 (umask 0177 around `Listen`); per-process name `<pid>.sock`. **If any check fails, or the path exceeds 104 bytes, mcc runs without a socket** (cards "output only"). Listener: ≤ 1 MiB per connection, 2 s deadline, one line, close; malformed JSON dropped. Events change display state only |
| Hook subcommands (`hook`, `statusline`) | Agents run hook commands through `sh -c`; bad quoting of our path; our output corrupting the agent's UI | Command string = `os.Executable()` → `EvalSymlinks` → POSIX single-quoted + ` hook`; settings JSON built with `encoding/json`; tested with a path containing a space and `'`. Both subcommands: `SilenceUsage`, `SilenceErrors`, no update check, `recover()` → exit 0, never write stdout/stderr of their own. stdin capped at 8 MiB (hook) / 1 MiB (statusline) and drained |
| Hook payload contents | `tool_input`, `tool_response`, `last_assistant_message` may contain secrets | The `hook` subcommand forwards only: event name, session/prompt/agent ids, agent type, tool name/use id, `tool_input.description` (≤ 200), notification type, `last_assistant_message` (≤ 200), `session_title` (≤ 80), `background_tasks` (id/type/agent_type/status/description ≤ 200), `transcript_path`, `cwd`. Debug log records event name + session + byte count only |
| Status-line payload | Account usage/limit figures pass through mcc; the user's own status-line command is run by our wrapper | Forwarded fields: `session_id, session_name, cost, context_window, rate_limits` only (`model.id` dropped); stdin over 1 MiB is passed through and not forwarded; the forward has a 200 ms budget and never delays the user's line. The user's command is resolved from *their* settings files (`CLAUDE_CONFIG_DIR` honoured; malformed JSON = none; a command that is our own `statusline` is dropped — recursion guard), never from the agent or the socket, and run with `exec.Command("sh","-c",cmd)` with the buffered stdin, stdout inherited, stderr discarded. Project/local settings files are repo-controlled; running their command is acceptable because Claude's workspace-trust gate precedes any hook or status-line execution, so mcc never runs a repo command Claude would not. Persisted: a per-session usage subset in `sessions.json` (0600); limits in memory only |
| **Codex session log reader** (the one transcript exception, approved) | Reading a file that holds everything the agent saw; a crafted path from a hook; a huge or malicious file | `internal/agent/codexusage.go` only. Path from Codex's own `SessionStart` hook, `EvalSymlinks`-resolved, must be under `${CODEX_HOME:-~/.codex}` (also resolved), `.jsonl`, regular file — else disabled. Read-only, tail-only, ≤ 256 KiB per 1 s poll; a line is decoded only far enough to see `type`/`payload.type`; only `token_count` records are parsed further, with a fixed struct; nothing else is decoded, retained, displayed or logged. Numbers only leave the function. Any error → reader stops, card shows `-` |
| **Process-tree scan and cleanup** | Killing a process that is not ours (pid reuse, wrong parent chain, a port-holder that happens to be listening); parsing attacker-named processes | Only processes observed as descendants of an mcc-launched agent's pid are ever recorded (`ppid` chain from one snapshot; set grows over time; nothing is inferred from ports or names). Identity is `{pid, startTime}` and is re-read and compared immediately before each signal; mismatch = skip. Signals: SIGTERM, 3 s grace, SIGKILL, to individual pids after the agent's own process group. Discovery runs as the user with fixed argv (`ps -axo pid=,ppid=,lstart=,comm=` on macOS, `/proc` on Linux; `lsof -nP -iTCP -sTCP:LISTEN -a -p …` for port annotation only). Process names go through `sanitise()` before display. The confirm dialog lists every pid before anything is signalled; `esc`/`n` stops nothing |
| Transcript files (all other) | Reading them would put every secret the agent ever saw through mcc | **Not read.** `transcript_path` is stored as an opaque string for the Codex usage reader's path check and nothing else. Claude transcripts are never opened |
| **Model routing (opt-in)** | Prompt text leaves the machine; API key leakage; a compromised or spoofed routing service influencing which model runs | Off by default; requires `routing.enabled` **and** a recorded consent (`routing.consented`, written only after the dialog). Sent: `{"prompt": <sanitised, ≤ 4 KiB>}`, the Jev model id and the question — never cwd, project name, env, history or anything typed inside a session (INTERACT bytes never touch this code path). Response can only select one of the configured model ids (any other `choice` → fallback); it cannot change argv beyond the `--model`/`-m` value. HTTPS only, Go TLS defaults, no redirects, 1.5 s timeout, no retries. API key from `TYPESAFE_API_KEY` or `routing.apiKeyCommand` (argv, no shell; stdout trimmed), held in memory for the request only, never written to config, state or logs; the debug log records "routed: tier/confidence" only |
| Environment passthrough | Leaking mcc vars; stripping vars the agents need | Children inherit `os.Environ()` plus `BUNGKUS_MCC_*`, with `TERM`/`COLORTERM` set and host-terminal identity vars unset. mcc never reads, logs or displays the environment |
| Config and state files | Tampering; world-readable | JSON under `~/.config/bungkus/mcc` and `~/.local/state/bungkus/mcc`, dirs 0700, files 0600, atomic writes. Values validated on load; a bad config is reported and replaced by defaults in memory. The only key mcc writes back into `config.json` is `routing.consented` |
| `setup codex` (only if M6 proves it necessary) | Silent modification of `~/.codex/hooks.json` | Round-trips `map[string]any`, aborts on malformed JSON, writes `.bak`, keeps the file mode, `EvalSymlinks` and writes next to the target, idempotent, prints the block and asks y/N, manual removal documented. Nothing in mcc modifies `~/.claude/settings.json` |
| Hook trust bypass | Running untrusted hooks | **Never pass `--dangerously-bypass-hook-trust`** or any `--dangerously-*` flag to either agent |
| Update check / self-update | MITM, tampered binary | HTTPS to `api.github.com` (release tag only; 3 s; daily; `BUNGKUS_NO_UPDATE_CHECK` disables). Cached tag validated as semver before display (cache 0600). `bungkus-mcc update` fetches `install.sh` at the resolved release tag, which verifies SHA-256 from that tag's `checksums.txt`. Control: TLS + integrity checksum; no signature (as bungkus-cli). Updater code copied from bungkus-cli; fixes apply to both repos |
| Debug log | Secrets in logs | Off by default; `--debug` writes a 0600 file with event names, sizes, errors, routing tier — no payload bodies, no prompts, no env, no keys |
| Signals / child lifetime | Orphaned agents after a crash | Children are in their own session and get SIGHUP when the PTY master closes (also on a mcc crash); descendants that survive a *crash* are not cleaned (no dialog, no scan) — a normal quit is required for cleanup |

### Explicitly out of scope

- Sandboxing or restricting what the agents do — configure the agents.
- Protecting against a malicious *user* or same-uid process.
- Multi-tenant or remote use.
- What TypeSafe does with a prompt after receiving it — governed by their
  DPA; the consent dialog says so.

## Rules for contributors

- No shell in `exec.Command` for anything mcc decides. Exceptions, each
  fixed and reviewed: `update.go` (copied installer pipeline) and the
  `statusline` wrapper running the user's own command.
- No raw agent bytes reach `os.Stdout`; no string from a hook, prompt,
  directory name, process table or session log reaches the screen without
  `sanitise()`.
- No transcript reads outside `codexusage.go`, and none there beyond
  `token_count` records. Extending it is a SECURITY.md review.
- No process is signalled unless it is in a session's observed-descendant
  set with a matching start time, or is the agent's own process group.
- New file paths: absolute, `filepath.Clean`, under mcc's dirs, the
  workspace, or (read-only) `CODEX_HOME`.
- New socket fields: optional, size-bounded, no side effects beyond UI state.
- New persisted field: "could this contain a secret?" — if yes, don't.
- No new network calls without updating "External communication"; no
  network call that sends user text without an opt-in and a consent dialog.
- No `--dangerously-*` flags to agents. No telemetry, ever.
- Copied code carries `// copied from osbrjp/bungkus-cli@<sha> <path>`.

## External communication

- **Release check.** Host `https://api.github.com/repos/osbrjp/bungkus-mcc/releases/latest`
  (hardcoded, HTTPS). Trigger: once a day on start when stderr is a
  terminal and `BUNGKUS_NO_UPDATE_CHECK` is unset; `bungkus-mcc update
  --check`. Auth: none. Policy: 3 s / 10 s timeout, no retries, no
  redirects, Go TLS defaults.
- **Self-update.** `curl -fsSL <install.sh at tag> | bash` → asset +
  `checksums.txt` from `github.com`, SHA-256 verified.
- **Model routing (opt-in, off by default).** Host
  `https://api.typesafe.ai/v1/systemone` (hardcoded, HTTPS, POST). Trigger:
  only when `routing.enabled` and `routing.consented` are true and the
  user starts or resumes a session *with a prompt* from the `n`/`r`
  picker with `model = auto`. Auth: `Authorization: Bearer <key>` from
  `TYPESAFE_API_KEY` or `routing.apiKeyCommand`. Sent: the sanitised start
  prompt (≤ 4 KiB), the Jev model id, one fixed Choice question; nothing
  else. Received: a tier choice, probabilities, confidence, token usage.
  Policy: 1.5 s timeout, no retries, no redirects, Go TLS defaults.
  Retention: TypeSafe states it does not train on requests; zero data
  retention is an enterprise option; standard retention is per their DPA
  (docs.typesafe.ai/legal) — surfaced verbatim in the consent dialog.
- The spawned agents make their own network calls under their own
  configuration; outside this tool's control.

## Dependency management & remediation policy

Same policy as bungkus-cli:

- **Inventory.** `go.mod` + `go.sum` are authoritative; hashes verified
  against the Go checksum database; all module paths fully-qualified.
- **Scanning.** `govulncheck ./...` in CI on every push.
- **Updates.** On advisories or periodic review; Charm modules in lockstep
  with bungkus-cli.
- **Remediation windows:** Critical ≤ 7 days; High ≤ 30 days; Moderate/Low
  ≤ 90 days. Uncalled modules: next routine bump.
- **Untagged dependency.** `charmbracelet/x/vt` pinned to a pseudo-version,
  diff read on every bump (it parses untrusted bytes).

## Dangerous functionality (summary for reviewers)

- **Subprocess execution** — `internal/term/session.go` (agents, argv);
  `internal/proc` (`ps`, `lsof`, fixed argv); `cmd/update.go` (installer
  via `bash -c`); `cmd/statusline.go` (user's status line via `sh -c`).
  Agents themselves run our `hook`/`statusline` commands via `sh -c`, hence
  the quoted executable path. Stage 2: `exec.LookPath("bungkus-cli")`.
- **Signals to other processes** — `internal/proc/kill.go`: the agent's
  process group, then observed descendants by pid + start time only.
- **PTY / raw terminal** — `internal/term`, Bubble Tea; restored on exit and panic.
- **Unix socket server** — `internal/ipc/server.go`; permissions, limits,
  fail-to-no-socket.
- **File reads outside mcc's dirs** — `internal/agent/codexusage.go`
  (Codex rollout, validated path, `token_count` only); `cmd/statusline.go`
  (Claude settings files, read-only, for the status-line command).
- **Outbound HTTP** — `pkg/update` (GitHub) and `internal/route`
  (TypeSafe, opt-in).
- **Filesystem writes** — `internal/store` (own dirs; `routing.consented`
  into config); `cmd/setup.go` (`~/.codex/hooks.json`, opt-in, if it
  survives M6).

## Risky components

To be reviewed against `go.mod` at the first release and quarterly
thereafter; record the date and `govulncheck` result here, as bungkus-cli does.
