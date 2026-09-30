# bungkus-mcc — Coding Rules

bungkus-mcc is Rust (decided 2026-09-30). The one-line rule stays: **the
lazy solution that works and is tested is the right one.** These rules
cover behaviour, tests, process and simplicity; **Rust style is defined
once, in `.claude/skills/rust-best-practices/SKILL.md`, which is
mandatory for every `.rs` and `Cargo.toml` change** and is not repeated
here. Where the two disagree, this file and SECURITY.md win for
behaviour, the skill wins for style.

## 1. Conventions

- Toolchain and lint set: TECH_STACK.md. The four commands at the top of
  the skill pass before any Rust change is "done".
- Module layout as in ARCHITECTURE.md §9: one binary crate, one module per
  concern (`app`, `ui`, `term`, `agent`, `ipc`, `proc`, `route`, `store`,
  `update`). Files ≤ ~400 lines, split by topic. `pub(crate)` by default.
- Errors: a `thiserror` enum per module boundary; `anyhow` only in
  `main.rs` and the subcommand entry points. Parsing untrusted input (hook
  payloads, `ps` output, Codex records, TypeSafe responses) never panics —
  it returns an error or skips the record.
- The TUI always restores the terminal: the raw-mode/alt-screen guard is a
  struct whose `Drop` restores; `main` also installs a panic hook that
  restores before printing. The `hook`/`statusline` subcommands catch
  every error and exit 0.
- Concurrency: ARCHITECTURE.md §8 is law. The UI thread owns all state;
  every other thread sends `AppEvent`s over `std::sync::mpsc`. No
  `Arc<Mutex<_>>` unless a channel is clearly worse (document the lock
  order). No async runtime.
- Processes: `std::process::Command` with an argv, never a shell. The two
  documented exceptions (`update` running the installer, `statusline`
  running the user's own command) each live in one function.
- Config/state: `serde` structs with `#[serde(default)]`; agent-facing
  payload structs never use `deny_unknown_fields`; atomic writes via a
  temp file + `rename` in `store`.
- Logging: the `debug_log!` macro in `store/debug.rs` (~20 lines), which
  writes to a 0600 file only under `--debug` and is a no-op otherwise;
  never to stdout/stderr while the TUI runs; never payload bodies,
  prompts, env or keys. No logging crate.
- Env vars: `BUNGKUS_MCC_*`; `BUNGKUS_NO_UPDATE_CHECK` shared with
  bungkus-cli. Read in one place each (`store::paths`, `ipc::hook`,
  `ipc::statusline`).
- Timeouts and caps are `const`s with a doc comment naming their source
  (`/// 1 MiB, per SECURITY.md "Hook subcommands".`).
- Comments: doc comments carry the documentation (skill §2). Inline `//`
  comments are rare and explain *why*. **No agent chatter, no edit
  narration.** A deliberate simplification with a known ceiling gets a
  `// ponytail:` comment naming the ceiling and the upgrade path.
- The **palette is a token spec** (DESIGN.md §2 tables). `ui/theme.rs`
  implements it; a table test compares every token's painted hex, fallback
  hex, 256 and 16 index with the spec, so the two implementations
  (bungkus-cli's `styles.go`, mcc's `theme.rs`) cannot drift unnoticed.

## 2. Testing

Unit tests live in `#[cfg(test)] mod tests` at the bottom of the file
they test, table-driven, named as sentences (`rejects_path_outside_codex_home`).
Fixtures live in `testdata/` next to the module. Goldens are plain text +
cursor files, updated only with `UPDATE_GOLDEN=1`. Every branch that
decides state or security has a test; view cosmetics have goldens; glue
has none. Coverage is not a target.

- **Adapters** (`agent`): `testdata/claude/*.json` are the real hook
  payloads recorded in the review round (PreToolUse(Agent), SubagentStart,
  Pre/PostToolUse with `agent_id`, SubagentStop with `background_tasks`,
  Stop), scrubbed; `testdata/codex/` is recorded in M6. Tests: parse every
  file; `reduce` over the recorded sequence asserting the decided states;
  synthetic sequences for needs-you, ignored `idle_prompt`, foreign
  `session_id` (ignored), unknown events (no-op); every event name in the
  generated hook config has a fixture.
- **Launch argv**: new (`--session-id` + `--settings` + `--name` + `--model`
  + `--` + prompt) and resume (`--resume` + `--settings`, no `--session-id`,
  no `--name`) for Claude; new/resume with `-c hooks.*` and `-m` for Codex;
  a prompt starting with `-`; strict-UUID validation rejects names, `..`,
  spaces.
- **Hook command quoting**: executable paths with a space and a `'`; the
  generated settings JSON parses and contains exactly the expected events.
- **Hook subcommand silence**: `hook` with stdin from each fixture and no
  socket → empty stdout/stderr, exit 0; 9 MiB stdin → exit 0 within budget.
- **Statusline wrapper**: resolver order and `CLAUDE_CONFIG_DIR`; malformed
  settings → none; recursion guard; the user's command gets the exact
  stdin bytes and its stdout passes through; > 1 MiB passed through and
  not forwarded; a socket that never answers costs ≤ 200 ms; forwarded
  line has `session_name` and no `model`.
- **Child environment**: `TERM`/`COLORTERM` set; host-terminal identity
  vars, `TYPESAFE_API_KEY` and every Claude/Codex **session-marker**
  variable (ARCHITECTURE.md §3.1 list) absent; `CLAUDE_CONFIG_DIR`,
  `CLAUDE_CODE_PROJECT_DIR_NAME`, `CODEX_HOME` preserved.
- **Views** (`ui`): (1) unit — render into a `ratatui::buffer::Buffer` and
  assert cell text; (2) golden — drive `app` with synthetic `AppEvent`s at
  120×40 and 80×24 with `NO_COLOR=1` and the ascii default; scenarios:
  empty workspace, first run, three sessions in five states, needs-you and
  failed gutters, expanded usage card, INTERACT banner, zoom, help overlay,
  `n` picker with name/model rows, quit confirm with descendants, routing
  consent, narrow stack, corner mascot (full/mini/title-bar fallback),
  error pair. The generated mockups in DESIGN.md are the first goldens.
- **Keymap**: walks every binding: help text non-empty; no key bound twice
  within the same pane/mode; the exit chord bound in INTERACT only; every
  vim motion has an arrow twin; `!` has `ctrl-]`.
- **INTERACT passthrough**: with the output pane focused, every
  `ratatui::crossterm::event::KeyEvent` in a generated set (printable, esc, tab,
  shift-tab, arrows, F-keys, every ctrl chord) reaches the PTY writer
  except the configured exit chord and `ctrl-z`; each focus route (`l`,
  `→`, `tab`, `enter`-on-session, click) flips the mode in the same
  update; the exit chord lands on the sessions pane; a child-exit event in
  INTERACT returns to NORMAL.
- **Key encoder** (`term/keys.rs`, from the spike): both directions — each
  key produces the expected bytes in normal and DECCKM mode with xterm
  modifier params, `shift+enter` → ESC CR, `ctrl-z` → nothing; bracketed
  paste only when mode 2004 is on; kitty CSI-u output when the child pushed
  kitty flags (M3).
- **Session names**: resolution order (`session_name` → `session_title` →
  picker name → prompt → `untitled`); later `session_name` overrides;
  sanitised, ≤ 80 chars.
- **Theme**: contrast ≥ 4.5 for text tokens against the painted backgrounds
  and for the fallback set against the reference backgrounds, with the
  exception list of DESIGN.md §2.1 (`info` and `fg-muted` on Nord);
  `ok/warn/err/accent` pairwise distinct at 256 and 16 in both themes;
  `fg-muted ≠ info`; painting only at TrueColor with `background: paint`
  (table over profile × config); emulator defaults equal the painted
  `bg`/`fg`; legs token per theme; **token table equals the spec**.
- **Mascot**: pixel tables equal DESIGN.md §5.7 (full 14×16, mini 6×8);
  every full frame is 7×16 cells and every mini frame 3×8; hop shifts up
  one pixel with longer legs and an intact tip; duck shifts down two;
  lookR's feet point right; only brand colours + legs token + transparent;
  died renders eye cells as bold `x`; mood and empty-state sequences equal
  the documented ones; the corner overlay is drawn only over blank cells
  (blank corner → sprite; one non-blank cell → `/..\` or `/xx\`); busy
  (PTY output within 1 s) selects the mini sprite; ticks only while an
  animated element is visible and motion is on.
- **Sidebar spinner**: a project with any running session shows the
  global-clock frame; the badge shows only needs-you / failed / your-turn;
  static under `motion: false`.
- **Sanitiser** (`ui/sanitise.rs`): the hostile corpus from
  ARCHITECTURE.md §4.2 through (a) the emulator + allowlist and (b) the
  string sanitiser as used by cards, header and dialogs; oracle: output
  contains only printable chars, `\n`, and `CSI…m`.
- **Emulator and session threads** (`term`): feeding `CSI 6n`, `CSI c`,
  `CSI > c`, `CSI 14 t`, `CSI 18 t`, `OSC 10/11 ?` → replies arrive on the
  PTY side within 100 ms through the writer thread, OSC 10/11 carrying
  the painted theme colours, CSI 14 t answered by mcc; the UI thread never
  blocks when the writer's PTY is stalled (fake writer test); the reader
  blocks on a full bounded channel instead of allocating; BSU with no ESU
  renders after 150 ms (`sync_timeout` deadline + `stop_sync`); child exit
  is reported on reader EOF/EIO **or** 500 ms after `wait` (test with a
  child that leaks the slave fd to a grandchild); `Config { kitty_keyboard:
  true }` is set and the encoder emits CSI-u while the agent has a flag
  pushed; a recorded stream with CJK, emoji, `⏺ ⎿` and combining marks →
  cursor column after each line equals the width sum; writes after child
  exit do not error (EIO ignored); the `spikes/cases/` corpus replays
  green, and the flood cases are timed interactively (M3 placement check).
- **PTY**: spawn `sh -c 'printf …; exit 3'` → exit code 3 reported after
  the reader drained (skipped when no PTY is available).
- **ipc**: round-trip over a temp dir; oversize and malformed payloads;
  permission bits (dir 0700, socket 0600 via `set_permissions` after
  bind, no umask change) and the fail-to-no-socket path (bad dir mode,
  symlink, > 104-byte path).
- **workspace**: dot-dirs skipped, symlinked dirs followed, marker files via
  `metadata()`, names with control characters sanitised.
- **Descendant tracking** (`proc`): fixtures of `ps -axo
  pid=,ppid=,uid=,lstart=,comm=` (macOS, including a ja_JP capture that
  must fail without `LC_ALL=C` and a `comm` with spaces) and
  `/proc/<pid>/stat` trees (Linux) → expected sets across three snapshots
  incl. reparent-to-init and a pruned entry; foreign-uid entries dropped;
  identity refuses a changed start time; default-keep rule and the `space`
  toggle; SIGHUP path applies keep; kill order (group first, `-pgid`
  SIGKILL only before the waiter reported; then `[stop]` descendants) with
  a fake signaller; EPERM → "could not stop"; dialog lists sessions then
  processes as `basename(comm) [:ports] pid n` with `… and N more` after
  8 rows; `lsof` parser and missing-`lsof` path; user stop yields
  `stopped` even with exit code 1; every name sanitised.
- **Codex usage reader** (`agent/codex_usage.rs`): fixtures with
  `token_count` records among decoys (incl. `token_usage_record`); path
  validation (outside `CODEX_HOME`, `..`, `~/.codex-evil` prefix trick,
  symlink escape, non-`.jsonl`, directory, FIFO via `File::metadata()`); tail from
  `len − 256 KiB` with the partial first line discarded; shrink; carried
  partial line; carry over 256 KiB dropped to the next `\n`; `null`
  `info`/`rate_limits`; malformed lines skipped; a line without
  `"token_count"` is never deserialised (counting decoder).
- **Routing** (`route`, a local `std::net::TcpListener` fake server): 200
  with each tier, `unclear`, low confidence, confidence outside `[0, 1]`,
  unknown choice, 401/429/529, malformed JSON, body over 64 KiB, slow
  server past the budget, empty tier map / missing key / secret-shaped
  prompt (no request made); resume never routes; golden request body with
  only `prompt`, `model` and the fixed question; sanitiser + 4 KiB
  truncation; key runner (once per process, own process group via
  `process_group(0)`, null stdin, 10 s then group kill, 4 KiB cap,
  trimmed); consent gate reads/writes `consent.json`.
- **Update**: semver compare table; cached tag validated; installer URL is
  at the resolved tag.
- **CLI smoke** (CI; skipped if binaries absent): `claude --help` and
  `codex --help` contain the flags `launch` uses. M3 manual check: the
  interactive entry points accept `--` before a dash-leading prompt.

## 3. Error handling and UX of failure

- User-facing errors are one line in the voice of DESIGN.md §5.6, shown in
  the getah bar for 5 s or in a dialog when action is needed (with the
  died mascot). Never a backtrace on screen.
- Recoverable I/O errors (socket unavailable, bad payload, unwritable
  state, EIO on a PTY after exit) degrade a feature and log; they never
  quit the app.
- Fatal at start only: not a TTY. Too small → message screen. Bad config →
  defaults + banner.

## 4. Commits, branches, PRs (same as bungkus-cli)

- Conventional commits (`feat:`, `fix:`, `test:`, `chore:`, `docs:`,
  `refactor:`); semantic-release: `main` = canary, `release` = stable; the
  promotion PR is merged with a merge commit, never squashed.
- Branch `i{issue#}-{date}-{seq}`, created by the "Start Pull Request"
  workflow on issue assignment.
- PR body: Target / Specification & Test Plan / Notes / Checklist /
  Evidence. TUI changes: screenshot or recording in at least one terminal,
  two if rendering changed (one of them tmux or Apple Terminal).
- CI green: `fmt`, `clippy -D warnings`, `test`, `doc -D warnings`,
  `cargo deny check` (TECH_STACK.md).

## 5. Review checklist

- [ ] Does this need to exist? Request or bug behind it?
- [ ] Reuses an existing function/type instead of a parallel one?
- [ ] No new crate; if one, TECH_STACK.md updated and `cargo deny` clean.
- [ ] The skill's checklist passes (lints, doc comments, no `unwrap`/`panic`/`todo` in shipped code, newtypes/enums, no chatter comments).
- [ ] Threads only send `AppEvent`s; no state outside the `app` model.
- [ ] Every new key in `ui/keymap.rs` with help text and an arrow/vim twin; per-pane uniqueness test passes.
- [ ] Every new state/badge: glyph + word + colour, an ascii glyph, Narrow width.
- [ ] Every new string from outside the PTY goes through `sanitise()`.
- [ ] Tested with `NO_COLOR=1`, the ascii default, inside tmux, at 80×24.
- [ ] Security rules (SECURITY.md): argv not shell; paths under mcc dirs or workspace; no transcript reads outside `codex_usage.rs`; no signal outside the observed-descendant set; no secrets persisted; socket limits; no `--dangerously-*`; no new egress without consent; no `unsafe`.
- [ ] Hook/statusline fields read optionally; recorded payload added to `testdata/`.
- [ ] Golden diffs reviewed line by line.
- [ ] Docs touched if behaviour, keys, files or crates changed.

## 6. Simplicity rules (YAGNI)

1. Build for Claude Code and Codex. A third agent gets a module when requested.
2. No daemon until detach is a confirmed requirement. No async runtime
   until a milestone needs one.
3. No transcript parsing, **except the one documented reader**
   (`agent/codex_usage.rs`, Codex `token_count` records only). Widening
   it, or adding another, is a SECURITY.md review.
4. One workspace, one socket, one process, one config file, one state file.
5. `std` first: `std::process`, `std::net::UnixListener`, `std::sync::mpsc`,
   `std::fs`, `std::time`; then an existing crate; a new crate last.
6. Delete before adding. Removing code needs less justification than adding it.
7. Ship the lazy version and ask: implement the smallest reading, note the
   larger one in the PR.
8. Speculative code is marked as such in the PR and usually rejected.
