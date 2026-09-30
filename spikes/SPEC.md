# M0 spike: embedded terminal pane, Go vs Rust

Goal: choose mc's foundation before M1. The riskiest part of mc is the
right pane: Claude Code / Codex run inside an embedded terminal emulator,
and the user types into them directly (prompts, slash commands, accepting
edits, shift+enter, paste, Japanese text). Both prototypes are built to
the same spec and measured by the same harness.

| Prototype | Directory | Stack |
|-----------|-----------|-------|
| Go        | `spikes/go/`   | Bubble Tea v2 + `charmbracelet/x/vt` + `creack/pty` (versions as in bungkus-cli / docs/TECH_STACK.md) |
| Rust      | `spikes/rust/` | ratatui + crossterm + `portable-pty` + the best-fit emulator crate (`alacritty_terminal`, `wezterm-term`/`termwiz`, or `vt100`), chosen and justified in its README |

Throwaway code: keep it small and readable. No tests beyond what the
harness exercises. Each prototype must build with one command
(`go build -o bin/proto .` / `cargo build --release`).

## 1. Interactive mode (for the human test)

`proto` with no flags opens a full-screen TUI:

- Left pane (24 cols): a list of two sessions, `claude` and `codex`. Both
  are started in the current directory when the app starts. `j/k` or
  arrows select a session.
- Right pane: the selected session's live screen, rendered from the
  emulator with colours, bold, reverse and wide characters. A border around
  it.
- Focus: `l`, `→`, `enter` or `tab` on the list focuses the right pane, and
  **every key is then forwarded to the agent**. That includes esc, tab,
  arrows, ctrl keys, shift+enter and bracketed paste. `ctrl-\` returns to
  the list. The border is double while focused.
- The pane size tracks the terminal size. Resize all emulators and PTYs on
  window resize.
- `q` on the list quits and kills both children.
- Mouse wheel over the right pane scrolls scrollback, if the emulator
  supports it cheaply. Otherwise skip it and note that in the README.

## 2. Headless mode (for the harness)

```
proto --headless --cols C --rows R --case spikes/cases/NAME.json --out OUT_DIR
```

Runs one case with no UI. Spawn the case's command in a PTY of C×R with the
emulator, and feed the PTY's output into the emulator continuously in the
background, so the agent's terminal queries (DSR, DA, OSC 10/11) are
answered while steps run. Execute the steps in order, then exit 0.

Case file format (JSON):

```json
{
  "name": "claude-slash",
  "cmd": ["claude"],
  "cwd": "/abs/path",
  "env": {"TERM": "xterm-256color", "COLORTERM": "truecolor"},
  "steps": [
    {"wait": 3000},
    {"bytes": "/"},
    {"key": "shift+enter"},
    {"paste": "line one\n日本語の行"},
    {"resize": [80, 24]},
    {"dump": "slash-menu"},
    {"waitexit": 5000}
  ]
}
```

Step semantics:

- `wait` (ms)
- `bytes`: a raw UTF-8 string written to the PTY as-is.
- `key`: a named key, encoded through the prototype's own key-encoding path,
  the same one interactive mode uses. Names: `enter`, `shift+enter`, `esc`,
  `tab`, `shift+tab`, `up`, `down`, `left`, `right`, `ctrl+c`, `ctrl+d`,
  `backspace`.
- `paste`: bracketed paste when the child has enabled it, raw otherwise.
- `resize`: `[cols, rows]`. Resize the emulator and the PTY.
- `dump`: writes `OUT_DIR/<label>.txt`. That is the visible screen as plain
  text, exactly R lines, each right-trimmed. Wide characters count once
  (don't emit a padding space for the spacer cell). Also writes
  `OUT_DIR/<label>.cursor` with `row col` (0-based) and
  `OUT_DIR/<label>.meta.json` with `{"alt_screen": bool}`.
- `waitexit` (ms): wait for the child to exit. Kill it after the timeout.

Timing output: write `OUT_DIR/timing.json` with `{"wall_ms": …}`, measured
from spawn until the last step finishes. Also record the ms at which the
emulator had consumed all PTY output after the child exited.

## 3. Report (each prototype's README.md)

- The emulator crate or package chosen, and why. Include its version,
  maintenance status and licence.
- Anything the emulator doesn't do that mc needs, and how much code it
  took or would take. Cover:
  - key encoding, including the kitty protocol / shift+enter;
  - scrollback viewport;
  - OSC 10/11 colour replies;
  - wide chars;
  - query replies;
  - mouse.
- Lines of code in the prototype (excluding generated), direct dependency
  count, release binary size, clean release build time.
- Rough notes from running the cases.

## Safety rules for the cases

- Never submit a prompt to an agent (no bare `enter` after typing text),
  except where a case explicitly says so.
- Run agents only with `cwd` = this repo, which is already trusted by
  Claude Code.
- Don't modify `~/.claude`, `~/.codex` or any user config.
- Exit agents with `ctrl+c` twice, or `/exit`.
