# bungkus-mcc — Coding Rules

Aligned with bungkus-cli; differences are called out. The one-line rule:
**the lazy solution that works and is tested is the right one.**

## 1. Go conventions

- Go 1.26, `gofmt`, `go vet` clean. No linters beyond that in stage 1.
- Package layout as in ARCHITECTURE.md §9. `internal/` only; no `pkg/`.
- One package = one concern; files ≤ ~400 lines, split by topic not by type.
- Exported identifiers only where another package uses them. `internal/theme`
  is the exception: everything there is exported because it is meant to be
  lifted into a shared module later.
- Errors: `fmt.Errorf("launch %s: %w", kind, err)` — lowercase, context first,
  wrap with `%w`. No custom error types until two call sites need to branch
  on one (`errors.Is`/`As` with sentinel vars at that point).
- No `panic` outside `main`/`init` for programmer errors. The TUI must always
  restore the terminal: `defer` the release in `cmd/root.go`, and recover in
  `Update` only to log + quit cleanly, never to continue.
- Context: subprocesses and the update check take a `context.Context`;
  nothing else needs one yet.
- Concurrency: the rules in ARCHITECTURE.md §8 are law. Grep for `go func`
  in review; each must end in `Program.Send` or a channel owned by a
  `tea.Cmd`. No mutexes in `internal/tui`. No `time.Sleep` in the model.
- Config/state: `encoding/json` with explicit struct tags, `omitempty` for
  optional, atomic write helper in `internal/store`. Unknown fields are
  ignored on read, preserved never (we don't round-trip user comments;
  JSON has none).
- Logging: `log/slog`, one logger created in `main`, nil-safe wrapper so the
  default is "no log file". Never log to stdout/stderr while the TUI runs.
- Env vars: `BUNGKUS_MCC_*` for ours; `BUNGKUS_NO_UPDATE_CHECK` shared with
  bungkus-cli. Read them in one place (`internal/store/paths.go` and
  `cmd/hook.go`), not scattered.
- Comments explain *why*, not what. A deliberate simplification with a known
  ceiling gets a `// ponytail:` comment naming the ceiling and the upgrade
  path (e.g. `// ponytail: FIFO pairing of PreToolUse→SubagentStart; use
  meta.json toolUseId if descriptions land on the wrong sibling`).

## 2. Testing

- Tests live next to the code, table-driven, `t.Run` per case, as in
  bungkus-cli's `pkg/*_test.go`.
- **Adapters**: `internal/agent/testdata/<agent>/<event>.json` are real hook
  payloads recorded from the agent (secrets scrubbed by hand — grep for
  `sk-`, `key`, home paths before committing). Tests: `Parse` of each file,
  and `Reduce` sequences that walk the state machine (idle → running →
  waiting → running → wrapped; subagent start/stop; unknown event is a
  no-op). A test asserts every event name in the hook config we generate
  has at least one recorded payload.
- **Views**: two layers.
  1. Unit: build the model, call `View()`, assert substrings (bungkus-cli
     style; fast, no goroutines).
  2. Golden: `teatest/v2` drives the program at 120×40 and 80×24, with
     `NO_COLOR=1` + `--icons ascii` so goldens are plain text and stable
     across terminals; keys pressed via `Type`/`Send`; compare with
     `RequireEqualOutput`. Goldens are updated only with `-update` and the
     diff is reviewed like code. Scenarios: empty workspace, three sessions
     in five states, INTERACT mode banner, help overlay, quit confirm,
     narrow mode.
- **Keymap**: a test walks every `key.Binding` and asserts (a) help text
  non-empty, (b) no key is bound twice within a mode, (c) `ctrl-\` is bound
  in INTERACT only, (d) every vim motion has an arrow twin.
- **Theme**: contrast test computes WCAG ratios for every `fg-*`, `accent`,
  `ok/warn/err/info` token against both reference backgrounds and fails
  below 4.5; icon width test asserts every glyph is exactly 1 cell (nerd set
  may be 1 or 2, but consistent per glyph).
- **term**: the emulator wrapper is tested with recorded byte streams
  (`testdata/*.bin`) including the hostile-sequence set from SECURITY.md;
  PTY spawning is tested with `/bin/sh -c 'printf ...; exit 3'` (skipped
  when no PTY is available, e.g. some CI sandboxes).
- **ipc**: server + client round-trip over a temp socket dir; oversize and
  malformed payload cases; permission bits asserted.
- **CLI smoke** (CI only, non-fatal skip if binaries absent): `claude --help`
  and `codex --help` contain the flags `Launch` uses.
- Coverage is not a target. Every branch that decides state or security has
  a test; view cosmetics have goldens; glue has none.

## 3. Error handling and UX of failure

- User-facing errors are one line, in the voice of DESIGN.md §8, shown in
  the getah bar for 5 s or in a dialog if action is needed. Never a stack
  trace on screen.
- Recoverable I/O errors (socket gone, hook payload bad, state file
  unwritable) degrade a feature and log; they never quit the app.
- Fatal at start only: terminal not a TTY, terminal too small is *not*
  fatal (message screen), config unparsable is not fatal (defaults + banner).

## 4. Commits, branches, PRs (same as bungkus-cli)

- Conventional commits: `feat:`, `fix:`, `test:`, `chore:`, `docs:`,
  `refactor:`. semantic-release reads them: `feat` → minor, `fix` → patch,
  `BREAKING CHANGE:` footer → major. `main` publishes canary pre-releases,
  `release` publishes stable; the promotion PR is merged with a merge
  commit, never squashed.
- Branch: `i{issue#}-{date}-{seq}` (e.g. `i12-20261007-0930`), created by
  the "Start Pull Request" workflow on issue assignment.
- PR body template: Target / Specification & Test Plan / Notes / Checklist /
  Evidence. Evidence for TUI changes = a screenshot or short recording in
  at least one terminal, two if the change touches rendering (one of them
  tmux or Apple Terminal).
- CI must be green: build, `go test ./...`, `govulncheck`, `gofmt -l`.

## 5. Review checklist

- [ ] Does this need to exist? Is there a request or a bug behind it?
- [ ] Reuses an existing helper/type instead of adding a parallel one?
- [ ] No new dependency; if one, TECH_STACK.md updated with alternatives.
- [ ] No interface with one implementation; no config for a constant.
- [ ] Goroutines only talk via `Program.Send`; no state outside the model.
- [ ] Every new key is in `keymap.go` with help text and an arrow/vim twin.
- [ ] Every new state/badge has glyph + word + color, and an ascii glyph.
- [ ] Tested with `NO_COLOR=1`, `--icons ascii`, inside tmux, at 80×24.
- [ ] Security: argv not shell; paths validated; no transcript reads; no
      secrets persisted; socket limits intact (SECURITY.md rules).
- [ ] Hook payload fields read optionally; recorded payload added to testdata.
- [ ] Golden diffs reviewed line by line, not rubber-stamped.
- [ ] Docs touched if behavior, keys, files or deps changed.

## 6. Simplicity rules (YAGNI)

1. Build for Claude Code and Codex. A third agent gets a file when it is
   requested, not a plugin system now.
2. No daemon until detach is a confirmed requirement.
3. No transcript parsing. If someone wants history, it is a separate adapter
   behind the same `Event` type, reviewed against SECURITY.md.
4. One workspace, one socket, one process, one config file.
5. Prefer stdlib. `encoding/json`, `net` (unix), `os/exec`, `log/slog`,
   `time.Ticker` before any library.
6. Delete before adding. A PR that removes code needs less justification
   than one that adds it.
7. Ship the lazy version and ask. If a request is ambiguous, implement the
   smallest reading, note the larger one in the PR, and let the owner say
   "need full X".
8. Speculative code is marked as such in the PR and usually rejected.
