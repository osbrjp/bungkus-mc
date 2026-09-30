# M0 spike: Rust prototype

ratatui 0.30 + crossterm 0.29 + portable-pty 0.9 + **alacritty_terminal 0.26**.

```sh
cargo build --release                      # binary: target/release/proto
./target/release/proto                     # interactive (starts claude + codex in $PWD)
./target/release/proto --headless --cols 120 --rows 34 \
    --case ../cases/NAME.json --out OUT_DIR
```

Headless writes one extra timing field: `drained_ms` is the time from spawn until
the pump thread reached PTY EOF, which is when the emulator had consumed all output
after the child exited. It is `null` if EOF did not arrive within 2 s of the last step.

## Emulator choice

| | alacritty_terminal | wezterm-term (+ termwiz) | vt100 |
|---|---|---|---|
| crates.io | 0.26.0 (2026-04-06) | **not published**; git dependency only. termwiz 0.23.3 (2025-03) is published | 0.16.2 (2025-07) |
| Maintenance | Active; ships with every Alacritty release | Active in the wezterm monorepo, but no crates.io releases | One maintainer, slow |
| Licence | Apache-2.0 | MIT | MIT |
| Query replies | DSR, DA1/DA2, OSC 10/11/12 colour, XTWINOPS size, kitty-mode query. All arrive as `Event`s that the embedder writes back to the PTY | Yes, through a writer | **None**: it parses queries and drops them |
| Key encoding | None (the encoder lives in the alacritty binary). It does track DECCKM, 2004 and the kitty keyboard mode stack | termwiz `KeyCode::encode`, including kitty | None |
| Scrollback | `Config::scrolling_history`, `scroll_display(Scroll::Delta/Bottom)`, `display_offset()` | Yes | `set_scrollback(n)` |
| Wide chars | `WIDE_CHAR` / `WIDE_CHAR_SPACER` cell flags, reflow on resize | Yes | Yes |

**Pick: alacritty_terminal.** It is the only candidate that is on crates.io,
actively maintained, *and* answers terminal queries. wezterm-term would add key
encoding for free, but a git-only dependency on a monorepo is a supply-chain and
pinning problem (no `cargo deny`/`cargo audit` story for it). vt100 would need all
the query handling written by hand. Missing key encoding is the cheaper gap to
fill: about 100 lines, below.

## What mc needs that the emulator does not do

- **Key encoding and shift+enter.** Our own table in `src/keys.rs` is **98 lines**
  of code, excluding doc comments. About 40 of those are the headless key-name
  parser and the unit tests. It covers:
  - printable keys → UTF-8, ctrl → C0, alt → ESC prefix;
  - arrows and Home/End in CSI or SS3 form, depending on DECCKM, with xterm
    `;m` modifier params;
  - PgUp/PgDn/Ins/Del/F1–F12;
  - `shift+tab` → `CSI Z`, `shift+enter`/`alt+enter` → `ESC CR`.

  **No kitty protocol output.** `Config::kitty_keyboard` is left `false`, so the
  emulator does not advertise kitty support and agents stay on legacy input.
  Kitty output would take another ~80 lines: CSI-u encoding driven by the
  already-tracked mode flags. On the host side, crossterm pushes
  `DISAMBIGUATE_ESCAPE_CODES` when the outer terminal supports it, so shift+enter
  can be told apart from enter. On a legacy outer terminal it can't be, and the
  agent receives plain CR.
- **Scrollback viewport.** Built in; the prototype adds 0 extra lines of scrollback
  logic. The mouse wheel over the right pane calls `scroll_display(±3)`, and any
  forwarded key snaps back to the bottom. On the alternate screen there is no
  history, so the wheel does nothing. The "scroll inside the agent" hint is not
  implemented.
- **OSC 10/11 colour replies.** The emulator emits `Event::ColorRequest` with a
  formatter. We answer with fixed defaults (fg `#d8d8d8`, bg `#1c1c1c`), not the
  host terminal's real colours: about 5 lines. Proxying the host's colours would
  need a startup query to the outer terminal, estimated at ~30 lines.
- **Wide chars.** Built in. The renderer skips `WIDE_CHAR_SPACER` cells, both in
  the headless dump and in the ratatui buffer, where ratatui skips the trailing
  column itself.
- **Query replies (DSR/DA).** Built in, as `Event::PtyWrite`. `Replier`
  (about 15 lines) writes them to the PTY from the pump thread, so replies go out
  without waiting on the UI. `TextAreaSizeRequest` (CSI 14/18 t) is ignored.
- **Mouse.** Not forwarded to the agent. SGR mouse encoding driven by
  `TermMode::MOUSE_MODE`/`SGR_MOUSE` would take ~30 lines. Only the wheel is used,
  and only for scrollback.
- **Bracketed paste.** crossterm's `Event::Paste` becomes `ESC[200~ … ESC[201~`
  when mode 2004 is on (ESC is stripped inside), otherwise raw text with LF → CR.

## Numbers

| | |
|---|---|
| Lines of Rust | 990 total; **727** excluding comments, doc comments and blank lines (5 files) |
| Direct dependencies | 7: alacritty_terminal, anyhow, crossterm, portable-pty, ratatui, serde, serde_json |
| Transitive crates (normal) | 118 |
| Release binary (stripped) | 1.24 MB (1 237 216 bytes) |
| Clean release build | 7.3 s wall, 58.7 s CPU (Apple Silicon, dependencies already downloaded) |

Lint gate: all of these pass with the `rust-best-practices` lint set in
`Cargo.toml`:
- `cargo fmt --all --check`;
- `cargo clippy --all-targets --all-features -- -D warnings` (pedantic,
  `unwrap_used` deny);
- `cargo test` (2 table tests for keys/paste);
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`.

`rustfmt.toml` omits `imports_granularity` and `group_imports`, because both are
nightly-only and stable rustfmt warns about them.

## Notes from running

- **Headless: text and wide chars.** Case
  `bash -c 'printf "hi\n日本語\n"; sleep 0.3'` at 80×4 dumps:
  - 4 lines: `hi`, `日本語` (no padding spaces), then two empty lines;
  - cursor `2 0`;
  - `wall_ms` 315, `drained_ms` 310.
- **Headless: DSR.** Case
  `bash -c 'printf "\e[6n"; IFS= read -rs -d R pos; echo "got${pos}"'`:
  - returns in 13 ms (`drained_ms` 4), with no hang;
  - a variant printing `${pos:1}` after `ab` shows `got[1;3R`, which is the correct
    position;
  - OSC 10 and DA1 (`ESC[?6c`) replies were also checked.
  - The replies were echoed on screen in some variants. That is because they
    arrive before bash's `read -s` turns off echo: a test-script race, not a
    prototype bug.
- **Resize.** A `resize` step to 40×10 reflows the text: alacritty rewraps long
  lines.
- **Interactive.** Tested under `tmux new-session -d -x 120 -y 34`:
  - Claude Code's welcome screen rendered correctly in the right pane: logo block
    characters, prompt box, status line;
  - `j` switched to Codex, which showed its update prompt;
  - `l` made the right border double; `ctrl-\` returned focus to the list;
  - `tmux resize-window` to 100×20 resized the pane and both PTYs;
  - `q` exited 0 and left no orphaned `claude`/`codex` processes.
- **Not verified by hand:**
  - typing into the agents: shift+enter, paste and Japanese IME;
  - colour fidelity;
  - wheel scrolling.

  These are for the human test. No prompts were submitted to either agent.
- **Redraw.** The UI redraws every 16 ms tick whether or not anything changed,
  which is simple but burns a little CPU when idle. A wakeup channel from the pump
  thread would fix this.
