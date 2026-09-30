# bungkus-mcc — Coding Rules

Aligned with bungkus-cli; differences are called out. The one-line rule:
**the lazy solution that works and is tested is the right one.**

## 1. Go conventions

- Go 1.26, `gofmt`, `go vet` clean. No linters beyond that in stage 1.
- Package layout as in ARCHITECTURE.md §9. `internal/` only; no `pkg/`.
- One package = one concern; files ≤ ~400 lines, split by topic.
- Exported identifiers only where another package uses them. `internal/theme`
  is the exception: it is written to be copied into other bungkus products.
- Errors: `fmt.Errorf("launch %s: %w", kind, err)` — lowercase, context
  first, `%w`. No custom error types until two call sites branch on one.
- No `panic` outside `main`/`init`. The TUI always restores the terminal
  (`defer` in `cmd/root.go`; `recover` in `Update` only to log and quit).
  The `hook`/`statusline` subcommands `recover()` and exit 0.
- Context for subprocesses and the update check; nothing else needs one.
- Concurrency: ARCHITECTURE.md §8 is law. Every `go func` ends in
  `Program.Send` or is the per-session reply pump. No mutexes in
  `internal/tui`. No `time.Sleep` in the model.
- Config/state: `encoding/json`, explicit tags, `omitempty`, atomic write
  helper. Unknown fields ignored on read.
- Logging: `log/slog`, one logger from `main`, nil-safe wrapper; never to
  stdout/stderr while the TUI runs; never payload bodies.
- Env vars: `BUNGKUS_MCC_*`; `BUNGKUS_NO_UPDATE_CHECK` shared with
  bungkus-cli. Read in one place each (`store/paths.go`, `cmd/hook.go`,
  `cmd/statusline.go`).
- **Copied code** from bungkus-cli starts with
  `// copied from osbrjp/bungkus-cli@<sha> <path>` and is changed as little
  as possible so fixes can be mirrored.
- Comments explain *why*. A deliberate simplification with a known ceiling
  gets a `// ponytail:` comment naming the ceiling and the upgrade path
  (e.g. `// ponytail: FIFO pairing of PreToolUse→SubagentStart; corrected
  by background_tasks on the next Stop`).

## 2. Testing

Tests live next to the code, table-driven, `t.Run` per case, as in
bungkus-cli. Every branch that decides state or security has a test; view
cosmetics have goldens; glue has none. Coverage is not a target.

- **Adapters** (`internal/agent`): `testdata/<agent>/*.json` are real hook
  payloads. The Claude set is the recorded sequence from the review round
  (`sub/ev.log`: PreToolUse(Agent) ×2, SubagentStart ×2, PreToolUse/
  PostToolUse with `agent_id`, SubagentStop ×2 with `background_tasks`,
  Stop ×3), scrubbed by hand (grep for `sk-`, `key`, tokens, home paths)
  before committing; the Codex set is recorded in M6. Tests: `Parse` of
  every file; `Reduce` over the recorded sequence asserting the decided
  states at each step (running → running with children → your turn),
  plus synthetic sequences for needs-you (permission_prompt /
  PermissionRequest), ignored `idle_prompt`, events with a foreign
  `session_id` (ignored), unknown events (no-op). A test asserts every event
  name in the hook config we generate has at least one recorded payload.
- **Launch argv**: new (`--session-id` + `--settings` + `--` + prompt) and
  resume (`--resume` + `--settings`, no `--session-id`) for Claude; new and
  resume with `-c hooks.*` for Codex; a prompt starting with `-`; session
  id validation rejects `..`, `/`, spaces.
- **Hook command quoting**: executable paths with a space and a `'`; the
  generated settings JSON parses and contains exactly the expected events.
- **Hook subcommand silence**: run `bungkus-mcc hook` with stdin from each
  testdata file and no socket → stdout and stderr are empty, exit 0; with
  a 9 MiB stdin → still exit 0 within the budget.
- **Views**: (1) unit — `View()` substring asserts; (2) golden — `teatest/v2`
  at 120×40 and 80×24 with `NO_COLOR=1` and `--icons ascii`, scenarios:
  empty workspace, first run, three sessions in five states, needs-you and
  failed gutters, expanded usage card, INTERACT banner, zoom, help overlay,
  `n` picker, quit confirm, narrow stack. Goldens are updated only with
  `-update` and reviewed line by line.
- **Keymap**: walks every `key.Binding`: help text non-empty; no key bound
  twice **within the same pane/mode**; the exit chord bound in INTERACT
  only; every vim motion has an arrow twin; `!` has `ctrl-]`.
- **Key translation table** (`internal/term/keys.go`): both directions —
  each `tea.Key` produces the expected bytes in normal and DECCKM mode,
  `shift-enter` → ESC CR, `ctrl-z` → nothing; bracketed paste only when
  mode 2004 is on.
- **Theme**: contrast ≥ 4.5 for `fg-muted, accent, ok, warn, err` against
  the reference backgrounds; `ok/warn/err/accent` pairwise distinct at 256
  and 16 in both themes; `fg-muted ≠ info` at 256 and 16; every glyph in
  every icon set is exactly 1 cell (`lipgloss.Width`) and East Asian width
  Narrow for state/marker glyphs.
- **Sanitiser** (`internal/term/sanitise.go`): the hostile corpus from
  ARCHITECTURE.md §4.2 through (a) the emulator + allowlist and (b) the
  string sanitiser as used by cards, header and dialogs; the oracle is
  "output contains only printable runes, `\n`, and `CSI…m`".
- **Emulator** (M3): feeding `CSI 6n`, `CSI c`, `CSI > c`, `OSC 10/11 ?`
  with the pump running → `Write` returns within 100 ms and the replies
  arrive on the PTY side; a recorded stream with CJK, emoji, `⏺ ⎿` and
  combining marks → cursor column after each line equals the wcwidth sum
  (documents the ambiguous-width risk).
- **PTY**: spawn `/bin/sh -c 'printf …; exit 3'` → exit code 3 reported
  after the reader drained (skipped when no PTY is available).
- **ipc**: round-trip over a temp dir; oversize and malformed payloads;
  permission bits and the fail-to-no-socket path (bad dir mode, symlink,
  >104-byte path).
- **workspace**: dot-dirs skipped, symlinked dirs followed, names with
  control characters sanitised for display.
- **CLI smoke** (CI; skipped if binaries absent): `claude --help` and
  `codex --help` contain the flags `Launch` uses.

## 3. Error handling and UX of failure

- User-facing errors are one line in the voice of DESIGN.md §5.6, shown in
  the getah bar for 5 s or in a dialog when action is needed. Never a stack
  trace.
- Recoverable I/O errors (socket unavailable, bad payload, unwritable state)
  degrade a feature and log; they never quit the app.
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
- CI green: build, `go test ./...`, `govulncheck`, `gofmt -l`.

## 5. Review checklist

- [ ] Does this need to exist? Request or bug behind it?
- [ ] Reuses an existing helper instead of a parallel one?
- [ ] No new dependency; if one, TECH_STACK.md updated.
- [ ] No interface with one implementation; no config for a constant.
- [ ] Goroutines only `Program.Send` (or are the pump); no state outside the model.
- [ ] Every new key in `keymap.go` with help text and an arrow/vim twin; uniqueness test still passes.
- [ ] Every new state/badge: glyph + word + colour, an ascii glyph, Narrow width.
- [ ] Every new string from outside the PTY goes through `sanitise()`.
- [ ] Tested with `NO_COLOR=1`, `--icons ascii`, inside tmux, at 80×24.
- [ ] Security rules (SECURITY.md): argv not shell; paths under mcc dirs or workspace; no transcript reads; no secrets persisted; socket limits; no `--dangerously-*`.
- [ ] Hook/statusline fields read optionally; recorded payload added to testdata.
- [ ] Golden diffs reviewed, not rubber-stamped.
- [ ] Docs touched if behaviour, keys, files or deps changed.

## 6. Simplicity rules (YAGNI)

1. Build for Claude Code and Codex. A third agent gets a file when requested.
2. No daemon until detach is a confirmed requirement.
3. No transcript parsing. The one proposed exception (Codex `token_count`)
   ships only with the owner's yes, off by default, reviewed against SECURITY.md.
4. One workspace, one socket, one process, one config file, one state file.
5. Stdlib first: `encoding/json`, `net` (unix), `os/exec`, `log/slog`,
   `time.Ticker`.
6. Delete before adding. Removing code needs less justification than adding it.
7. Ship the lazy version and ask: implement the smallest reading, note the
   larger one in the PR.
8. Speculative code is marked as such in the PR and usually rejected.
