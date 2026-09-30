# bungkus-mcc — Tech Stack

Status: proposal. Rule: every dependency must earn its line here. Versions
match bungkus-cli where the module is shared; others are the latest tagged
release as of 2026-09-30 (re-check at `go mod init` time).

## Language and toolchain

| Item | Choice | Why |
|------|--------|-----|
| Go | 1.26 (`go 1.26.0` in go.mod, CI uses `1.26.x` + `check-latest`) | Same as bungkus-cli; single static binary; stdlib has everything needed for JSON, unix sockets, signals, atomic file writes |
| Targets | darwin/linux × arm64/amd64 | Same release matrix as bungkus-cli; Windows explicitly out of scope (no ConPTY work) |
| Build flags | `-s -w -X main.Version=<tag>` | Same as bungkus-cli |

## Direct dependencies (7)

| Module | Version | Purpose | Alternatives rejected |
|--------|---------|---------|-----------------------|
| `charm.land/bubbletea/v2` | v2.0.9 (match bungkus-cli; bump both together — v2.0.10 exists) | TUI runtime: raw mode, alt screen, kitty keyboard, mouse, sync output, color-profile detection, resize | tview/tcell (imperative, no v2 color pipeline, second style system next to Lip Gloss); gocui (unmaintained feel); writing our own renderer (no) |
| `charm.land/lipgloss/v2` | v2.0.6 (match) | Styles, borders, layout joins, light/dark helpers, OSC 8 | none needed |
| `charm.land/bubbles/v2` | v2.2.1 (match) | `key` (keymap + generated help), `help`, `spinner`, `viewport`, `textinput` (prompt, filter), `filepicker` (workspace picker) | hand-rolled widgets — bubbles is already in the tree via bungkus-cli |
| `github.com/spf13/cobra` | v1.10.2 (match) | Subcommands `hook`, `setup`, `update`, `--version`; same CLI feel as bungkus-cli | stdlib `flag` — would work (3 subcommands), but cobra is what the copied `update.go` uses and what the team knows; not worth diverging |
| `github.com/creack/pty` | v1.1.24 | Open PTY, start child with size, `Setsize` on resize | `golang.org/x/sys/unix` ioctls by hand (~80 lines of platform-specific code we'd own; pty is the de-facto lib and tiny) |
| `github.com/charmbracelet/x/vt` | pseudo-version, pin the newest at implementation (e.g. `v0.0.0-20260927004216-9c77d672503d`) | VT emulator: feeds PTY bytes, keeps screen + scrollback, `Render()`/`Draw()`, `SendKeys`/`Paste` encode input per the agent's terminal modes | `vito/midterm` v0.2.5 (maintained, but cells are its own type — we would convert to ultraviolet cells ourselves and lose the input encoder); `hinshun/vt10x` (dead since 2022) |
| `golang.org/x/mod` | v0.41.0 (match) | `semver` compare for the update check (copied code) | hand-written compare — bungkus-cli already uses this |

Transitive (pulled by the above, already in bungkus-cli's go.sum): `charmbracelet/ultraviolet`, `charmbracelet/colorprofile`, `charmbracelet/x/ansi`, `x/term`, `x/termios`, `lucasb-eyer/go-colorful`, `rivo/uniseg`, `clipperhouse/displaywidth`, `muesli/cancelreader`, `spf13/pflag`, `golang.org/x/sys`, `golang.org/x/sync`.

Risk note on `x/vt`: no semver tags, API may move. Containment: it is used
in exactly one package (`internal/term`), behind a 6-method wrapper
(`Write, Resize, Render, SendKey, Paste, Scroll`). Swapping to midterm is a
one-package change.

## Test-only dependencies (1)

| Module | Version | Purpose | Alternatives rejected |
|--------|---------|---------|-----------------------|
| `github.com/charmbracelet/x/exp/teatest/v2` | pseudo-version (Sep 2026) | Drive the model with `Send`/`Type`, wait for output, golden-file compare (`RequireEqualOutput`, `-update`) | Calling `View()` directly (bungkus-cli style) — still used for unit views; teatest adds the end-to-end "press keys, compare screen" tests we want for the modal keymap |

## Deliberately not used

| Thing | Why not |
|-------|---------|
| `fsnotify` | No transcript tailing in stage 1. If a later history adapter needs it, a 500 ms `os.Stat` + `ReadAt` poll (stdlib) is enough for a handful of files; fsnotify on macOS is kqueue (one fd per file) and still needs the poll fallback for atomic-rename writers |
| TOML/YAML config libs | `encoding/json` for config; Claude Code and Codex users edit JSON already |
| SQLite / bbolt | State is one small JSON array |
| HTTP framework, gRPC, protobuf | IPC is one JSON line per unix-socket connection (`net.Listen("unix", …)`, stdlib) |
| `charmbracelet/log`, zap, zerolog | `log/slog` to a file, only under `--debug` |
| Clipboard libs | OSC 52 via Bubble Tea; no `pbcopy`/`xclip` shelling |
| tmux control mode / zellij plugin API | See ARCHITECTURE.md §2, option 5 |
| Claude Agent SDK / Codex app-server client | Stage 1 renders the agents' own UIs; a structured client is a different product |
| `curl` in hooks | `bungkus-mcc hook` is our own binary: no dependency on the user's PATH, handles the socket path, timeouts, and the 0-exit contract |

## Runtime prerequisites (not dependencies)

- `claude` and/or `codex` on PATH (detected at start; missing ones just hide from the "new session" picker with a hint).
- `bash`, `curl` for `bungkus-mcc update` (same as bungkus-cli; the installer is re-run).
- A UTF-8 locale for the default icon set (ASCII otherwise).

## Tooling

Same as bungkus-cli: `go test ./...`, `go vet`, `govulncheck` in CI,
semantic-release with conventional commits, GitHub Actions matrix build,
checksums.txt, `install.sh`. Add `gofmt -l` check in CI (cheap, catches
noise in reviews). No golangci-lint in stage 1 (one more config to maintain;
add if reviews keep finding the same lint-class issues).
