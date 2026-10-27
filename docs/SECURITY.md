# Security

bungkus-mc is a local, single-user terminal application. It runs with the
invoking user's own OS privileges, spawns AI coding agents (`claude`,
`codex`, `pi`) as children with those privileges, listens on a private
unix socket for bounded events from them, reads one kind of record from
Codex's session log, stops descendants on quit, and stores small state
under the user's home directory.
It exposes no network service and sends nothing anywhere except the
release check and — only when the user opts in — the model-routing
request described below. This document records the security-relevant
aspects of the tool itself, in the same format as bungkus-cli's SECURITY.md.

## Reporting a vulnerability

Open a private security advisory on the GitHub repository
(`osbrjp/bungkus-mc`), or email the maintainer. Please do not file public
issues for undisclosed vulnerabilities.

## Threat model

Trust boundaries, most to least trusted:

1. The user and their shell environment.
2. bungkus-mc's own binary and config.
3. The agent processes it spawns — they run *as the user* and are driven by
   an LLM reading untrusted repository content. mc does not and cannot
   sandbox them; that is the agents' job.
4. Bytes the agents write to their PTYs, JSON their hooks and status line
   send to mc's socket, lines in Codex's session log, prompt text,
   directory names in the workspace, and process names in the process
   table — **untrusted input**.
5. Other local users on the machine — must not read mc's socket or state.

**Out of scope, explicitly:** an attacker running as the *same uid*. The
socket has no peer-credential check beyond directory/file permissions and
no HMAC; a same-uid process can already read the user's agent data
(`~/.claude`, `~/.codex`, `~/.pi`), which is strictly more than it could
learn from mc.

Secrets goal, restated: **mc's state on disk is a 0600 subset of what the
agents already store.** No `tool_input` beyond a 200-char description, no
`tool_response`, no transcript text; names and messages truncated.

### Assets and threats

| Surface | Threat | Control |
|---------|--------|---------|
| Spawning agents | Argument injection via prompt, name or project | `std::process::Command` (through `portable-pty`) with an argv vector, never a shell. Agent commands come from config and are resolved on `PATH`. Prompt and name are one value each; prompts follow `--`; model ids match `^[A-Za-z0-9._:-]{1,64}$`; project directories are only `cwd`. Claude/Codex resume ids must parse as UUIDs. Pi resume ids must match its documented 1–64 character `A-Za-z0-9._-` alphabet with alphanumeric ends. Pi's extension is one path under mc's verified runtime dir. A cloud start is one `--cloud=<prompt>` argument. Claude teleport ids match only `session_`/`cse_` plus ASCII alphanumerics and never become a flag or path |
| Workspace / project selection | Launching in an unintended directory | Workspace is an absolute cleaned path from CLI arg, config or the first-run input. Projects are direct child directories containing `CLAUDE.md`, `AGENTS.md` or `.git` (`read_dir` + `fs::metadata` of the three markers, symlinks followed), dot-dirs skipped, `cwd = workspace.join(entry.file_name())`. The card's git line comes from `git --no-optional-locks status --porcelain=v2 --branch` (fixed argv, read-only, branch name sanitised) in each session folder. Worktree cleanup (`d` on a session with a worktree, `c` on a project) runs `git worktree unlock|remove|prune|list` with fixed argv in the project folder, never with `--force`, so git refuses to drop uncommitted work. The one exception: a project's linked git worktrees elsewhere on disk are listed from `<repo>/.git/worktrees/*/gitdir`, and only when the folder's own `.git` file points back at that repository; mc refuses to move them to the Trash |
| **All strings not from the PTY** (hook fields, prompt/name, directory names, process names, agent versions) | Terminal escape injection through a card, the header or a dialog | One `ui::sanitise::sanitise()` (char-based: strip every `ESC`-led sequence to its terminator, then drop every `char` < U+0020 except `\t`→space, U+007F, U+0080–U+009F; truncate) applied in `Card::reduce` / `Card::report` to names, titles, failure reasons and subagent descriptions (`ipc::trim` caps every hook field's length first), in `workspace::scan`, in `proc::scan`, and on the prompt/name before display. The hostile-sequence corpus is rendered through cards, header and dialogs, not only the output pane |
| Agent output rendering | Escape-sequence injection into the *host* terminal | All PTY bytes go through the VT emulator into a cell grid; the cell-to-buffer mapping is an **allowlist** (printable graphemes + SGR attributes are copied into the ratatui buffer; nothing else exists in a cell, and OSC 8 hyperlink data is dropped in stage 1). Emulator events for clipboard/title/bell/notifications are not forwarded. Corpus and oracle in ARCHITECTURE.md §4.2 |
| Hook / status-line socket | Spoofed events; oversized payloads; symlink races | Directory `clean(${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}})/bungkus-mc-<uid>/` created 0700; before listening: `symlink_metadata` — a directory, owned by our uid, mode 0700, not a symlink; socket `set_permissions(0o600)` immediately after `UnixListener::bind` (no process-wide umask change; the 0700 directory covers the gap); per-process name `<pid>.sock`. **If any check fails, or the path exceeds 104 bytes, mc runs without a socket** (cards "output only"). Listener: ≤ 1 MiB per connection, 2 s deadline, one line, close; malformed JSON dropped. **Plainly: a prompt-injected agent — same uid, holding `$BUNGKUS_MC_SOCK` and `$BUNGKUS_MC_SESSION` — can forge any event.** The effect is bounded to display state (a card can lie about its own status or usage) plus one file read, the Codex usage reader, whose path is validated under `CODEX_HOME` and whose parser only yields numbers. No event can start, stop, signal or route anything |
| Hook bridge commands (`hook`, `pi-hook`, `statusline`) | Bad command quoting; bridge output corrupting an agent UI; a stuck bridge delaying the agent | Claude/Codex hook command strings use the canonical executable with POSIX single-quoting and are tested with spaces and `'`. Pi's bundled extension calls the canonical executable as fixed argv `["pi-hook"]`, `shell: false`, and kills it after 2 s. All bridge subcommands catch errors/panics, exit 0, and write no stdout/stderr. stdin is capped and drained: 8 MiB for `hook`, 1 MiB for `pi-hook`/`statusline` |
| Hook / extension payload contents | Tool inputs, messages or results may contain secrets | `hook` keeps only bounded lifecycle ids/type/name fields, `tool_input.description`/Codex `task_name` (≤ 200), notification type, last message (≤ 200), session title (≤ 80), bounded background-task metadata, opaque transcript path and cwd. The Pi extension forwards lifecycle ids, tool name/id, cwd and session title; its usage report contains only numeric token/cache/cost/context values and the bounded session name. It never forwards Pi messages, tool arguments/results, prompts or session files. Debug logs record event name/session/byte count, never payload bodies |
| Status-line payload | Account usage/limit figures pass through mc; the user's own status-line command is run by our wrapper | Forwarded fields: `session_id, session_name, cost, context_window, rate_limits` only (`model.id` dropped); stdin over 1 MiB is passed through and not forwarded; the forward has a 200 ms budget and never delays the user's line. The user's command is resolved from *their* settings files (`CLAUDE_CONFIG_DIR` honoured; malformed JSON = none; a command that is our own `statusline` is dropped — recursion guard), never from the agent or the socket, and run with `Command::new("sh").args(["-c", cmd])` with the buffered stdin, stdout inherited, stderr null. Project/local settings files are repo-controlled; running their command is acceptable because Claude's workspace-trust gate precedes any hook or status-line execution, so mc never runs a repo command Claude would not. Persisted: a per-session usage subset in `sessions.json` (0600); limits in memory only |
| **Issue / pull request links** (`gh`) | A hostile title reaching the screen; a URL from the API reaching the desktop's opener as an option or a `file:` address; a token passing through mc | `src/app/links.rs` only. `gh` is run with fixed argv (the variable arguments are an issue number mc parsed as `u64`, a validated `https://` URL, and a `gh api` path built from a `github.com` URL whose owner and repository hold only `A-Za-z0-9._-` and whose number is a `u64`), no shell, null stdin, in the session's folder; mc never reads, stores or passes a token (`gh` uses the user's own login). Output is parsed with `serde_json` into number, title, state and URL; anything else is dropped. Titles, states and every line of an issue's or pull request's description and comments, and comment authors' names (the popup's text view, capped at 2000 lines) go through `sanitise()` before the Markdown reader (`src/app/markdown.rs`) sees them; the reader shows a link's text and never its address. A URL is kept only when it starts with `https://` and holds no whitespace or control character, and is then one argument of `open` / `xdg-open`. Read-only: mc never writes to GitHub. Nothing is persisted |
| **MCP server names** | A forged hook event names a server | The only source is the `tool_name` of hook events (`mcp__<server>__…`), already size-bounded and sanitised by the socket reader; `src/agent/mcp.rs` cuts a name to 24 characters, ≤ 12 per session. A forged event can only add a name to its own card. A name only picks text and a glyph on the card; nothing is run, and the agents' MCP config files are never read |
| **Codex session log reader** (the one transcript exception, approved) | Reading a file that holds everything the agent saw; a crafted path from a hook; a huge, growing or special file | `src/agent/codex_usage.rs` only. Path from Codex's own `SessionStart` hook: `Path::canonicalize` on it and on `${CODEX_HOME:-~/.codex}`, then `resolved.strip_prefix(codex_home)` must succeed component-wise (string-prefix checks are not enough), extension `jsonl`; opened read-only with `OFlags::NONBLOCK` and the open `File::metadata()` (an fstat on the fd) must be a regular file — else disabled. Read-only, tail-only, ≤ 256 KiB per 1 s poll, partial-line and carry-buffer limits; lines are prefiltered with a byte search for `"token_count"` and only those are deserialised into a fixed tolerant struct; nothing is retained, displayed or logged beyond the numbers. Any error → reader stops, card shows `-` |
| **Process-tree scan and cleanup** | Killing a process that is not ours (pid reuse, wrong parent chain, another user's process, a port-holder that happens to be listening, a long-lived agent the user relies on); parsing attacker-named processes; a locale-mangled `ps` | Only processes observed as descendants of an mc-launched agent's pid are recorded; uid and `{pid, startTime}` are checked, and Linux uses pidfds. **Default-keep:** application bundles, `claude`/`codex`/`pi`, listed infrastructure tools and `cleanup.keep`. Signals are SIGTERM, 3 s grace, SIGKILL. Ports annotate rows only; they never select a process. Discovery uses fixed argv and `LC_ALL=C`; argv of other processes is never read. A fresh scan and explicit list precede every stop |
| Transcript / session files (all other) | Reading them would put every secret the agent saw through mc | **Not read.** `transcript_path` stays opaque except for the validated Codex usage reader. Claude transcripts and Pi session files are never opened |
| **Model routing (opt-in)** | Prompt text leaves the machine; a prompt that contains a secret; API key leakage; a compromised or spoofed routing service influencing which model runs; a key command that prompts or hangs | Off by default; requires `routing.enabled` **and** a recorded consent (`consent.json` in the state dir, written only after the dialog). Only the `n` start prompt is ever sent — never resume, never INTERACT bytes, never cwd, project name, env or history — as `{"prompt": <sanitised, ≤ 4 KiB>}` plus the Jev model id and the fixed question. Prompts matching common secret shapes (`sk-`, `ghp_`, `github_pat_`, `AKIA`, `xox[bp]-`, `-----BEGIN`) are not sent at all (`default · not routed`). Response body capped at 64 KiB (ureq body limit / `Read::take`); `confidence` must be within `[0, 1]`; the `choice` can only select one of the configured model ids (anything else → fallback) and cannot change argv beyond the `--model`/`-m` value. HTTPS only, ureq/rustls defaults, no redirects, 1.5 s timeout, no retries. API key from `TYPESAFE_API_KEY` or `routing.apiKeyCommand` (argv, no shell) — the command runs once per mc process, lazily, in its own process group (`CommandExt::process_group(0)`, no `pre_exec`), `Stdio::null()` stdin, stderr discarded, a 10 s timeout after which the group is killed, stdout capped at 4 KiB and trimmed; the key is held in memory for the process lifetime, never written to config, state or logs, and `TYPESAFE_API_KEY` is unset in every child's environment; the debug log records "routed: tier/confidence" only |
| Environment passthrough | Leaking mc vars (the routing key); stripping vars the agents need | Children inherit `std::env::vars_os()` plus `BUNGKUS_MC_*`, with `TERM`/`COLORTERM` set, host-terminal identity vars unset, and **`TYPESAFE_API_KEY` unset** (test). mc never reads, logs or displays the environment |
| Per-workspace GitHub account (`ghConfigDirs`) | mc handling a GitHub token; a cloned workspace pointing `gh` at a folder it controls (its own account, or `gh` aliases that run commands) | mc sets `GH_CONFIG_DIR` to the configured folder for children under that workspace, and for its own read-only `gh` calls for issue / pull request links there, and does nothing else: it never reads the folder, and no token is read, held, passed or stored by mc (`gh` reads its own keychain entry). The key is read from the **global** `config.json` only; a workspace's `.bungkus-mc/config.json` cannot set it (`Overrides` has no such key). Entries that are not absolute paths after `~` expansion are skipped (test) |
| **Inherited agent session markers** | An agent started inside another agent inherits parent markers and misbehaves | Every child unsets the explicit Claude markers from ARCHITECTURE.md §3.1 plus Pi's `AI_AGENT`, `PI_CODING_AGENT`, `PI_SESSION_ID`, `PI_SESSION_FILE`, `PI_PROVIDER`, `PI_MODEL`, `PI_REASONING_LEVEL`. User configuration (`CLAUDE_CONFIG_DIR`, deliberate `CLAUDE_CODE_*` settings, `CODEX_HOME`, `PI_CODING_AGENT_DIR`) is kept. Values are never read or logged; tests assert both lists |
| Config and state files | Tampering; world-readable | JSON under `~/.config/bungkus/mc` and `~/.local/state/bungkus/mc`, dirs 0700, files 0600, atomic writes. The Pi extension is atomically written 0600 into mc's verified 0700 runtime dir and contains no executable path or secret; the path is passed only at launch. Invalid config is reported and replaced by in-memory defaults |
| Hook trust bypass | Running untrusted hooks | **Never pass `--dangerously-bypass-hook-trust`** or any `--dangerously-*` flag to an agent |
| Update check / self-update | MITM, tampered binary | The release tag comes from the public GitHub API through `curl` (fixed argv, 3 s, 64 KiB cap, hourly; `BUNGKUS_NO_UPDATE_CHECK` disables; skipped silently offline). No token is used, read or stored. Cached tag validated as semver before display (cache 0600). `bungkus-mc update` and `U` download `install.sh` from `main` over HTTPS and run it with `bash`; it verifies the binary's SHA-256 against the release's `checksums.txt`. Control: TLS + integrity checksum; no signature (as bungkus-cli) |
| Debug log | Secrets in logs | Off by default; `--debug` writes a 0600 file with event names, sizes, errors, routing tier — no payload bodies, no prompts, no env, no keys |
| Signals / child lifetime | Orphaned agents after a crash | Children are in their own session and get SIGHUP when the PTY master closes (also on a mc crash); descendants that survive a *crash* are not cleaned (no dialog, no scan) — a normal quit is required for cleanup |

### Explicitly out of scope

- Sandboxing or restricting what the agents do — configure the agents.
- Protecting against a malicious *user* or same-uid process.
- Multi-tenant or remote use.
- What TypeSafe does with a prompt after receiving it — governed by their
  DPA; the consent dialog says so.

## Rules for contributors

- No shell in `std::process::Command` for anything mc decides. Exceptions,
  each fixed and reviewed: `update` (the installer pipeline, ported from
  bungkus-cli) and the `statusline` wrapper running the user's own command.
  `o` splits the `editor` setting (global `config.json` only, never a
  workspace's) or `$VISUAL`/`$EDITOR` on whitespace into an argv (no shell), and
  `t` runs `$SHELL` itself as the interactive terminal the user asked for;
  both come from the user's own environment, the same trust as the user.
- A workspace's `.bungkus-mc/config.json` can come from elsewhere (a
  clone, a shared drive), so it may only set `defaultAgent`, `worktrees`,
  `notify` and `cleanup.keep`; agent commands and arguments are read from
  the user's own `config.json` only. Its `groups` name projects by folder
  name; a name that is no project directly inside that workspace is
  ignored, so the file cannot make mc pass an agent any other path with
  `--add-dir`. Grouping gives sessions the requested write access to the
  other members. `.bungkus-mc/CLAUDE.md` / `AGENTS.md` reach agents as
  instructions after mc's built-ins: one argv value for Claude and Pi,
  one TOML-quoted argv value for Codex, never a shell. argv is visible to
  the user's other processes, so instruction files must hold no secret.
- No raw agent bytes reach stdout (`print_stdout` is a denied lint; the
  emulator yields cells, and only printable cells + SGR reach the ratatui
  buffer); no string from a hook, prompt, directory name, process table or
  session log reaches the screen without `sanitise()`.
- No transcript reads outside `agent/codex_usage.rs`, and none there beyond
  `token_count` records. Extending it is a SECURITY.md review.
- No `unsafe`, no `pre_exec`. If a syscall truly needs it, see
  "Dependency management & remediation policy" below.
- No process is signalled unless it is in a session's observed-descendant
  set with a matching start time, or is the agent's own process group.
- New file paths: absolute, normalised (`canonicalize` where the file must exist), under mc's dirs, the
  workspace, or (read-only) `CODEX_HOME`.
- New socket fields: optional, size-bounded, no side effects beyond UI state.
- New persisted field: "could this contain a secret?" — if yes, don't.
- No new network calls without updating "External communication"; no
  network call that sends user text without an opt-in and a consent dialog.
- No `--dangerously-*` flags to agents. No telemetry, ever.
- Code ported from bungkus-cli (the updater) names its origin in the
  module's `//!` doc comment (`Port of osbrjp/bungkus-cli pkg/update.go @<sha>`)
  so security fixes can be mirrored in both directions.

## External communication

- **Release check.** mc runs `curl -fsSL --max-time 3` against
  `https://api.github.com/repos/osbrjp/bungkus-mc/releases/latest` (fixed
  argv, no shell, no token) and keeps only `tag_name`, validated as semver.
  Trigger: once an hour (on start and while running) when stderr is a terminal and
  `BUNGKUS_NO_UPDATE_CHECK` is unset; `bungkus-mc update --check`; `U`.
  Policy: 3 s timeout, 64 KiB cap, no retries, skipped silently on failure.
- **Issue and pull request links.** mc runs the user's `gh` CLI (fixed
  argv, no shell): `gh pr view` and `gh issue view` in each session folder
  when its branch changes and every 60 s for running sessions, and `gh pr
  list` / `gh issue list` when the `i` popup opens, and `gh issue view` /
  `gh pr view` for one description and its comments when `enter` asks,
  plus `gh api repos/…/pulls/<n>/comments` for a pull request's review
  comments on code. `gh` talks to GitHub
  with the user's own login; mc sends nothing of its own and handles no
  token. Nothing happens without `gh` on `PATH`. Policy: one read at a
  time, no retries, failures are silent (nothing is linked).
- **Self-update.** `install.sh` is downloaded from the repository's `main`
  with `curl` and run with `bash` (the one documented shell use for
  updates); it downloads the release asset and `checksums.txt` and
  verifies SHA-256. The installer's `bkmc` symlink is created only when no
  `bkmc` exists on `PATH` or in the install dir. It never replaces another
  program's command. It never asks for sudo unless the existing install's
  folder is not writable and `~/.local/bin` does not come first on `PATH`
  (ARCHITECTURE §11); the shadowed old copy is reported, never removed by
  the script. In the TUI (`U`) the installer gets no terminal, so a sudo
  prompt fails with a hint instead of hanging.
- **Uninstall.** `bungkus-mc uninstall` removes only its own binary, the
  `bkmc` link that points at it, and with `--purge` mc's own config, state
  and cache folders, after listing them and asking (`--yes` skips).
- **Model routing (opt-in, off by default).** Host
  `https://api.typesafe.ai/v1/systemone` (hardcoded, HTTPS, POST). Trigger:
  only when `routing.enabled` is true in config, `consent.json` records
  consent, and the user **starts** a session *with a prompt* from the `n`
  picker with `model = auto` — never on resume. Auth: `Authorization: Bearer <key>` from
  `TYPESAFE_API_KEY` or `routing.apiKeyCommand`. Sent: the sanitised start
  prompt (≤ 4 KiB), the Jev model id, one fixed Choice question; nothing
  else. Received: a tier choice, probabilities, confidence, token usage.
  Policy: 1.5 s timeout, no retries, no redirects, 64 KiB body cap,
  ureq/rustls defaults.
  Retention: TypeSafe states it does not train on requests; zero data
  retention is an enterprise option; standard retention is per their DPA
  (docs.typesafe.ai/legal) — surfaced verbatim in the consent dialog.
- The spawned agents make their own network calls under their own
  configuration; outside this tool's control.

## Dependency management & remediation policy

Same policy as bungkus-cli (which uses `govulncheck`), with the Rust tools:

- **Inventory.** The committed `Cargo.lock` pins everything; `Cargo.toml`
  minor-pins 0.x crates by caret; every crate comes from crates.io
  (`deny.toml` `sources` allows no git dependencies — the reason
  `wezterm-term` was rejected).
- **Scanning.** `cargo deny check` (RustSec advisories, licences, bans,
  sources; `[graph] targets` = the four unix triples) in CI on every push
  and PR.
- **Updates.** On advisories or periodic review; `alacritty_terminal`
  bumps are read as a diff (it parses untrusted bytes).
- **Remediation windows:** Critical ≤ 7 days; High ≤ 30 days; Moderate/Low
  ≤ 90 days. Advisories in crates whose affected code we do not call:
  next routine bump.
- **`unsafe` is denied crate-wide** (`unsafe_code = "deny"`); `rustix`
  provides safe wrappers for signals, pidfd, uid, `O_NONBLOCK` and
  `poll`; child process groups use std's safe `process_group(0)`, never
  `pre_exec`/`setsid`.
  Any future exception is one small function in one module that allows
  `unsafe_code`, with a `// SAFETY:` comment on every block, reviewed
  against this document.
- **Lints as a security control.** `unwrap_used`/`panic`/`todo` are denied
  in shipped code, so untrusted input (hook payloads, `ps` output, Codex
  records, TypeSafe responses) cannot crash mc; the skill's checklist is
  part of review.

## Dangerous functionality (summary for reviewers)

- **Subprocess execution** — `src/term/session.rs` (agents, argv, via
  `portable-pty`); `src/proc/` (`ps`, `lsof`, fixed argv, `LC_ALL=C`);
  `src/route/` (`routing.apiKeyCommand`, argv, once, own process group
  via `process_group(0)`, null stdin, 10 s then group kill);
  `src/update.rs` (`curl` for the release lookup and for `install.sh`,
  fixed argv; the downloaded installer via `bash <file>`);
  `src/app/links.rs` (`gh`, fixed argv, read-only; URLs to `open` /
  `xdg-open` only when plain `https://`);
  `src/app/diff.rs` (`git status` / `git diff`, fixed argv, read-only,
  `--no-ext-diff`; a path from `git status` goes back only after `--`
  with `--literal-pathspecs`; diff lines sanitised);
  `src/app/ding.rs` (the first of `afplay` / `pw-play` / `paplay` /
  `aplay` on `PATH`, fixed argv plus mc's own `ding.wav`, null stdio);
  `src/ipc/statusline.rs` (user's status line via `sh -c`). All are
  `std::process::Command`. Claude/Codex run quoted `hook`/`statusline`
  command strings through `sh -c`; Pi's JS extension uses Node/Bun
  `spawn` with fixed argv and `shell: false`.
- **Signals to other processes** — `src/proc/kill.rs` (`rustix::process::
  kill_process_group`, `pidfd_open`/`pidfd_send_signal` on Linux): the
  agent's process group, then observed descendants by pid + start time only.
- **PTY / raw terminal** — `src/term/`, `ratatui::crossterm` raw mode +
  alt screen behind a `Drop` guard; restored on exit and from the panic
  hook. Per session one writer thread owns the PTY writer; the UI thread
  never blocks on the PTY.
- **Unix socket server** — `src/ipc/server.rs` (`std::os::unix::net::
  UnixListener`); permissions, limits, fail-to-no-socket.
- **File reads outside mc's dirs** — `src/agent/codex_usage.rs` (Codex
  rollout, canonicalised path under `CODEX_HOME`, `File::metadata()` regular file,
  `token_count` only); `src/ipc/statusline.rs` (Claude settings files,
  read-only, for the status-line command).
- **Sessions outside mc** — `src/external.rs` lists `claude agents --json`
  and native own-uid `codex`/`pi` processes plus cwd from fixed-argv
  `lsof`. Node-hosted Pi is omitted rather than reading argv. Rows are
  sanitised and never persisted or read beyond that. `x`, then explicit
  `y`, sends one SIGTERM only after a fresh snapshot still identifies the
  pid as this user's `claude`, `codex`, `pi` or `node` with the same start
  time. Never by port. Takeover is Claude-only and resumes a UUID reported
  by `claude agents`.
- **Outbound HTTP** — none in-process before M9: `src/update.rs` shells
  out to `curl` (GitHub API and `install.sh`, fixed argv, `--max-time`).
  `src/route/` (TypeSafe, opt-in, M9): `ureq` with rustls, timeouts, no
  redirects, capped bodies.
- **Filesystem writes** — `src/store/` (own state/config/runtime dirs only,
  including the bundled Pi extension). mc never writes agent config:
  integrations are injected per launch (`--settings`, `-c`, `--extension`).

## Risky components

To be reviewed against `Cargo.lock` at the first release and quarterly
thereafter; record the date and the `cargo deny check` result here, as
bungkus-cli does with `govulncheck`.
