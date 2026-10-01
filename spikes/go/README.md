# M0 spike: Go prototype

Bubble Tea v2 + `charmbracelet/x/vt` + `creack/pty`, built to `spikes/SPEC.md`.

```sh
cd spikes/go
go build -o bin/proto .              # Go 1.26 (GOTOOLCHAIN=auto fetches it)
bin/proto                            # interactive: claude + codex in $PWD
bin/proto --headless --cols 120 --rows 34 --case ../cases/NAME.json --out OUT_DIR
```

Files: `main.go` (session = PTY + emulator + goroutines, key encoding, screen
dump), `tui.go` (interactive model), `headless.go` (case runner).

## Emulator: `github.com/charmbracelet/x/vt`

- Version `v0.0.0-20260927004216-9c77d672503d` (pseudo-version; the module
  has no tagged releases). MIT. Actively maintained by Charm (commits in the
  last week); it is the emulator Charm builds its own tools on and it shares
  the `ultraviolet` cell/screen types that Bubble Tea v2 uses, so it builds
  against the `ultraviolet` version Bubble Tea v2.0.9 pins with no
  replace directives.
- Why: same vendor and types as the TUI stack mc already uses (bungkus-cli),
  `Render()` gives a styled string that drops straight into `tea.View.Content`,
  built-in scrollback (10 000 lines default), and it answers terminal queries
  itself. Rejected alternatives are in `docs/TECH_STACK.md`.
- `SafeEmulator` (a mutex wrapper shipped with the package) is used because the
  PTY reader goroutine writes while the UI reads.

## Gaps vs what mc needs

| Area | x/vt provides | What we added / would add |
|------|---------------|---------------------------|
| Key encoding | Legacy xterm only: C0 ctrl chords, DECCKM-aware arrows, F-keys, keypad, shift+tab, alt as ESC prefix. **No kitty / CSI-u / modifyOtherKeys** (TODO in its source). It also silently drops any key with a modifier it doesn't list, e.g. shift+letter. | `sendKey` (~10 lines): shift+enter → `ESC CR`; any key with `Text` and no ctrl/alt → its UTF-8 (covers shifted chars and IME input); else `SendKey`. Modified arrows/home/end (`CSI 1;m A`) and ctrl+enter are still dropped: ~15 more lines. A kitty encoder for agents that ask for it: ~150 lines. |
| Scrollback viewport | `Scrollback().Lines()`, `ScrollbackLen()`, each `uv.Line` has `Render()`. No viewport. | Wheel offset + slice of scrollback+screen lines: ~20 lines (`paneContent`). The scrollback read is unlocked (race, marked `ponytail:`); a real build copies lines under the lock. No scrollback on the alt screen (Codex), as expected. |
| OSC 10/11 | Answers queries, but with its defaults (white on black) unless `SetDefault{Fore,Back}groundColor` is called. | Not done. mc should request the host colours (`tea.RequestBackgroundColor`) and forward them into every emulator: ~10 lines. `SafeEmulator` doesn't wrap `SetDefault*`, so call them on the embedded `Emulator` before the reader starts or under our own lock. |
| Wide chars | Correct: wide cells + zero spacer; grapheme clustering; `Line.String()` skips the spacer. | Nothing. Japanese renders and dumps correctly. |
| Query replies | DSR (CPR), DA, OSC 10/11/12 are answered into an internal `io.Pipe`. `Write` **blocks** until that pipe is read. | One pump goroutine per session, `io.Copy(ptmx, emu)` (1 line). All our input (keys, paste, `SendText`) goes through the same pipe, so the pump is the only PTY writer. Verified: DSR case returns `ESC[1;1R` and doesn't hang. |
| Mouse | `SendMouse` encodes X10/normal/button/any-event in SGR or X10 format based on the modes the child set. | Not forwarded in the spike (the wheel always scrolls our viewport). Forwarding when the child enabled mouse modes: ~15 lines. |
| Bracketed paste | `Paste()` wraps in `ESC[200~ … ESC[201~` only if the child set mode 2004. | Nothing; `tea.PasteMsg` → `Paste()`. |
| Sanitising | `Render()` re-emits OSC 8. | Not done in the spike (ARCHITECTURE §4.2 allowlist). |

## Numbers

| Metric | Value |
|--------|-------|
| Lines of Go | 503 total, 405 excluding comments and blanks (`main.go` 188, `tui.go` 187, `headless.go` 128) |
| Direct dependencies | 5 (`bubbletea/v2`, `lipgloss/v2`, `x/vt`, `ultraviolet` for `uv.Line`/`uv.KeyPressEvent`, `creack/pty`); 25 modules in the build graph (`go list -m all`) |
| Release binary (`-ldflags '-s -w'`, darwin/arm64) | 4.3 MB (6.1 MB unstripped) |
| Clean release build (`go clean -cache`, modules already downloaded, M-series Mac) | 2.2 s wall |

## Notes from running cases

The shared `spikes/cases/` harness wasn't present when this was built, so these
are local cases (JSON in the same format):

- `printf 'hi\n日本語\n'` at 30×6, dump, resize to 40×10, dump, waitexit: both
  dumps are exactly R lines, `日本語` is 3 runes, cursor `3 0`. Timing
  `{"wall_ms":317,"child_exit_ms":317,"drained_ms":317}`.
- DSR `printf '\e[6n'; read -rs -d R pos`: `got\e[1;1` in ~9 ms, no hang. OSC 11
  query returns `rgb:0000/0000/0000` (x/vt default, see gap above). `CSI c`
  (DA1) also answered.
- Key path (child in raw mode dumping bytes with `od`): shift+enter `033 \r`,
  shift+tab `033 [ Z`, up `033 [ A`, ctrl+c `003`, backspace `177`, paste with
  mode 2004 on `033 [ 200 ~ a b \n 日 033 [ 201 ~`. All as expected.
- `drained_ms` equals `child_exit_ms` for these short children: the reader
  hits EOF on the master as soon as the last slave fd closes.
- Interactive, under `tmux -x 120 -y 34`: Claude Code's welcome screen and
  prompt render with box-drawing, colour and the status line; `j` shows Codex's
  trust prompt (not answered); `l` switches to the double border; `ctrl-\`
  returns; resizing the tmux window to 100×28 resizes both panes and Claude
  redraws; `q` exits 0 and no claude/codex process is left behind.
- Not driven by a human yet: typing into the agents, shift+enter in a
  kitty-capable terminal, IME, the mouse wheel.
