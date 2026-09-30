# bungkus-mcc — Tech Stack

Status: proposal, revision 2. Rule: every dependency must earn its line
here. Versions match bungkus-cli where the module is shared; others are the
latest release as of 2026-09-30 (re-check at `go mod init`).

## Language and toolchain

| Item | Choice | Why |
|------|--------|-----|
| Go | 1.26 (`go 1.26.0` in go.mod; CI `1.26.x` + `check-latest`) | Same as bungkus-cli; single static binary; stdlib covers JSON, unix sockets, signals, atomic writes |
| Targets | darwin/linux × arm64/amd64 | Same release matrix as bungkus-cli; Windows out of scope (no ConPTY) |
| Build flags | `-s -w -X main.Version=<tag>` | Same as bungkus-cli |

## Direct dependencies (8)

| Module | Version | Purpose | Alternatives rejected |
|--------|---------|---------|-----------------------|
| `charm.land/bubbletea/v2` | v2.0.9 (match bungkus-cli; bump both together) | TUI runtime: raw mode, alt screen, kitty keyboard negotiation for *our* input, mouse, synchronized output, colour-profile detection, `BackgroundColorMsg` | tview/tcell (second style system next to Lip Gloss); own renderer (no) |
| `charm.land/lipgloss/v2` | v2.0.6 (match) | Styles, borders, joins, light/dark helpers, OSC 8 in the header | — |
| `charm.land/bubbles/v2` | v2.2.1 (match) | `key` (keymap + generated help), `help`, `spinner`, `textinput` (prompt, filter, workspace path) | hand-rolled widgets. The `filepicker` widget was considered and cut (a text input is enough) |
| `github.com/spf13/cobra` | v1.10.2 (match) | Subcommands `hook`, `statusline`, `setup`, `update`; `SilenceUsage/SilenceErrors` for the silent ones | stdlib `flag` would do, but the copied `update.go` is cobra and the team knows it |
| `github.com/creack/pty` | v1.1.24 | Open PTY, start child with size, `Setsize` on resize | raw `x/sys/unix` ioctls (~80 platform lines we'd own) |
| `github.com/charmbracelet/x/vt` | pseudo-version, pin the newest at implementation (built and tested with `v0.0.0-20260927004216-…` against bubbletea v2.0.9) | VT emulator: feeds PTY bytes, screen + scrollback, `Render()`; terminal-side replies via its `Read` side | `vito/midterm` v0.2.5 (maintained; own cell type; no reply pipe design) — the fallback if x/vt fails M3; `hinshun/vt10x` (dead since 2022) |
| `golang.org/x/mod` | v0.41.0 (match) | `semver` for the update check (copied code; also validates the cached tag) | hand-written compare |
| `github.com/charmbracelet/x/ansi` | v0.11.8 (match, already transitive via lipgloss) | `ansi.Strip` in `sanitise()`; `Convert256/Convert16` in the theme tests | own escape parser — no |

Verified behaviours of `x/vt` that shape the design (scratchpad `vt/`):
`Write` **blocks** until its reply pipe is drained (→ pump goroutine);
`Render()` re-emits OSC 8 (→ stripped by the allowlist); it has **no
kitty-keyboard client encoder** (→ our key table); it builds against the
`ultraviolet` version Bubble Tea pins. Contained in one package
(`internal/term`) behind a 6-method wrapper.

East Asian Width for the glyph-width test: a **hard-coded table** for our
~40 glyphs (generated once from `unicodedata`, checked into the test),
not `rivo/uniseg` — the set is tiny and the table doubles as documentation.

Transitive (already in bungkus-cli's go.sum): `charmbracelet/ultraviolet`,
`colorprofile`, `x/ansi`, `x/term`, `x/termios`, `lucasb-eyer/go-colorful`,
`rivo/uniseg`, `clipperhouse/displaywidth`, `muesli/cancelreader`,
`spf13/pflag`, `golang.org/x/sys`, `golang.org/x/sync`.

## Test-only dependencies (1)

| Module | Version | Purpose | Alternatives rejected |
|--------|---------|---------|-----------------------|
| `github.com/charmbracelet/x/exp/teatest/v2` | pseudo-version (Sep 2026) | Drive the model with `Send`/`Type`, golden-file compare (`RequireEqualOutput`, `-update`) | `View()` string asserts only (kept for unit views) |

## Deliberately not used

| Thing | Why not |
|-------|---------|
| `fsnotify` | No transcript tailing in stage 1. If the gated Codex `token_count` exception is approved, a 500 ms `os.Stat` + `ReadAt` poll (stdlib) is enough for one file |
| TOML/YAML libs | `encoding/json`; Claude Code and Codex users edit JSON already |
| SQLite / bbolt | State is one small JSON array |
| HTTP framework, gRPC | IPC is one JSON line per unix-socket connection (`net.Listen("unix")`) |
| `charmbracelet/log`, zap | `log/slog` to a file, only under `--debug` |
| Clipboard libs | OSC 52 via Bubble Tea; no `pbcopy`/`xclip` |
| tmux control mode / zellij plugin | ARCHITECTURE.md §2 option 5 |
| Claude Agent SDK / Codex app-server client | Stage 1 renders the agents' own UIs |
| `curl` in hooks | `bungkus-mcc hook` is our own binary: no PATH dependency, controlled silence, timeouts |
| Nerd-font detection libs | Impossible to do reliably; `icons` setting instead |

## Runtime prerequisites (not dependencies)

- `claude` and/or `codex` on PATH (detected at start; shown on the first-run screen).
- `bash`, `curl` for `bungkus-mcc update` (same as bungkus-cli).
- A UTF-8 locale for the default icon set (ASCII otherwise).

## Tooling

Same as bungkus-cli: `go test ./...`, `go vet`, `govulncheck` in CI,
semantic-release with conventional commits, GitHub Actions matrix build,
checksums.txt, `install.sh`. Added: `gofmt -l` check in CI. No
golangci-lint in stage 1.
