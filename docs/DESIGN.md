# bungkus-mcc — Design Language ("Daun Pisang")

Status: proposal. This is the shared visual language for bungkus products;
bungkus-mcc is the first to use it in full, bungkus-cli adopts it lazily
(see "Relationship to the Lackluster palette").

## 1. The image

Nasi lemak, bungkus-style: coconut rice on a square of banana leaf, folded
into a pyramid, wrapped again in newspaper or brown paper, rubber band around
it. Open it: sambal (red, oily), half a boiled egg (yolk), cucumber slices,
fried ikan bilis and peanuts (golden brown), and the leaf's green underneath
everything.

What we take from it:

| Element            | What it becomes in the UI                                    |
|--------------------|--------------------------------------------------------------|
| Daun pisang (leaf) | The "surface". Focus, running, healthy. Green.               |
| Sambal             | Danger / failure. Red-orange, used sparingly, high signal.   |
| Kuning telur (yolk)| Attention: agent is waiting for you. Amber.                  |
| Nasi (rice)        | Primary text. Off-white.                                     |
| Surat khabar       | Secondary text, chrome. Newspaper gray.                      |
| Minyak sambal      | Brand accent (title, selection). The oil stain on the paper. |
| Getah (rubber band)| The status/mode bar that "holds the packet together".        |
| Bungkus            | Done. A finished session is "wrapped".                       |

Theme name: **Daun Pisang** (`daun-pisang-dark`, default) and
**Daun Pisang Pagi** (`daun-pisang-light`, "morning" variant).

## 2. Palette

We never paint the whole screen background. The terminal's own background is
kept (this is what users of kitty/Ghostty themes expect, and it avoids the
"black rectangle in a gray terminal" look). Contrast ratios below are
computed against a reference background (`#080808` dark, `#f3eee2` light);
on a user's mid-gray terminal background ratios will be lower — that is why
every state also has a glyph and a word, never color alone.

WCAG formula used: (L1 + 0.05) / (L2 + 0.05) with sRGB relative luminance.
Targets: AA normal text 4.5:1, AA large/bold 3:1, non-text UI 3:1.

### 2.1 Dark theme — `daun-pisang-dark` (default)

Reference bg `#080808` (Lackluster Gray1). Optional card surface `#121512`
(leaf-tinted, 1.09:1 against bg — a tint, not a contrast element).

| Token         | Nickname     | Hex       | Role                                      | vs bg  | AA  | 256 idx | 16-color |
|---------------|--------------|-----------|-------------------------------------------|--------|-----|---------|----------|
| `fg`          | Nasi         | `#deeeed` | primary text                              | 16.75  | yes | 254     | white (bright) |
| `fg-muted`    | Surat khabar | `#708090` | secondary text, hints, timestamps         | 4.94   | yes | 66      | white (normal) |
| `fg-dim`      | Dim          | `#555555` | decorative only (rules, inactive borders) | 2.69   | n/a | 240     | black (bright) |
| `border`      | Dark         | `#2a2a2a` | inactive pane border                      | 1.40   | n/a | 235     | black (bright) |
| `border-focus`| Daun         | `#7fb069` | focused pane border                       | 7.93   | yes | 107     | green |
| `accent`      | Minyak       | `#ffaa88` | app title, selected row, links in header  | 10.83  | yes | 216     | yellow (bright) |
| `ok`          | Daun         | `#7fb069` | running / healthy / done marker           | 7.93   | yes | 107     | green |
| `warn`        | Kuning       | `#f2c14e` | waiting for input, needs attention        | 11.93  | yes | 215     | yellow |
| `err`         | Sambal       | `#ff5c47` | failed, destructive confirm               | 6.56   | yes | 203     | red (bright) |
| `info`        | Biru         | `#7788aa` | hyperlinks, session ids, neutral badges   | 5.61   | yes | 103     | blue (bright) |

Inverse badges (bg = token, fg = `#080808`): black on Kuning 11.93, black on
Daun 7.93, black on Sambal 6.56 — all pass. Do not put Nasi on Sambal (2.55).

`fg-dim` and `border` deliberately fail text contrast: they are never used
for text. bungkus-cli currently uses Dim for footer text (`FooterBarStyle`);
mcc uses `fg-muted` for that role instead.

### 2.2 Light theme — `daun-pisang-light`

Reference bg `#f3eee2` (Santan, coconut cream). Card surface `#e9e3d3`.

| Token         | Hex       | vs bg | AA  | 256 idx |
|---------------|-----------|-------|-----|---------|
| `fg`          | `#1f2321` | 13.74 | yes | 234 |
| `fg-muted`    | `#5a6470` | 5.20  | yes | 241 |
| `fg-dim`      | `#8a8f8a` | 2.84  | n/a | 245 |
| `border`      | `#cfd2c8` | 1.32  | n/a | 252 |
| `border-focus`| `#2f6b25` | 5.58  | yes | 22  |
| `accent`      | `#b8461f` | 4.61  | yes | 130 |
| `ok`          | `#2f6b25` | 5.58  | yes | 22  |
| `warn`        | `#8a5b00` | 5.07  | yes | 94  |
| `err`         | `#b3261e` | 5.65  | yes | 124 |
| `info`        | `#3f5f8a` | 5.64  | yes | 60  |

Light is chosen by `lipgloss` background detection (terminal query, falls
back to dark) and can be forced with `--theme light|dark` or config.

### 2.3 Fallback ladder

Detection is Bubble Tea v2 / colorprofile's job (`COLORTERM`, `TERM`,
`NO_COLOR`, `CLICOLOR_FORCE`, tmux/screen quirks). We only declare colors
once, as truecolor hex; downsampling to 256/16 is automatic. The 256 and
16-color columns above are what that downsampling produces — they were
checked so that the four state colors stay distinguishable (green / yellow /
red / blue) at 16 colors, and so that `fg-muted` does not collapse into
`fg-dim` (66 vs 240; white vs bright-black).

At 16 colors and in `NO_COLOR` mode the UI must still be fully usable: every
state carries a glyph + word, focus is shown by a border style change (single
→ heavy/double) not only a color change, and selection uses reverse video.

### 2.4 Relationship to the Lackluster palette (bungkus-cli)

bungkus-cli's `internal/tui/styles.go` uses the Lackluster palette. Decision:
**evolve, keep neutrals verbatim, replace the four semantic colors.**

| Lackluster   | Hex       | In Daun Pisang                                            |
|--------------|-----------|-----------------------------------------------------------|
| Luster       | `#deeeed` | kept → `fg` (Nasi)                                         |
| Lack         | `#708090` | kept → `fg-muted` (Surat khabar)                           |
| Dim          | `#555555` | kept → `fg-dim` (decorative only, no longer for text)      |
| Dark, Gray1  | `#2a2a2a`, `#080808` | kept → `border`, reference bg                   |
| Orange       | `#ffaa88` | kept → `accent` (Minyak)                                   |
| Green        | `#789978` | replaced by Daun `#7fb069` (6.32 → 7.93; reads as green at 256, Lackluster's collapses to gray 244) |
| Yellow       | `#abab77` | replaced by Kuning `#f2c14e` (a yolk, and clearly "attention" rather than olive) |
| Blue         | `#7788aa` | kept → `info`                                              |
| Error        | `#d70000` | replaced by Sambal `#ff5c47` (3.71 fails AA on dark; Sambal 6.56) |

Why not replace everything: bungkus-cli users already see Luster/Lack/Orange;
five of ten values are identical, so bungkus-cli can adopt the shared names
in a one-file PR with no visible change except greener greens and a legible
error red. Why not keep everything: two Lackluster colors fail contrast or
collapse at 256 colors, and none of them say "nasi lemak".

Sharing mechanism: in stage 1 the palette is a ~60-line Go file copied into
each product (`internal/theme/theme.go`) — see ARCHITECTURE.md "Relationship
with bungkus-cli" for why not a shared module yet.

## 3. Typography and glyphs (honest section)

A TUI does not choose the font. What we control: which code points we emit.

- **Recommended fonts** (documented in README, not enforced): any Nerd Font
  patched monospace — JetBrainsMono Nerd Font, Hack Nerd Font, or the
  terminal's default with Nerd Font symbol fallback (kitty `symbol_map`,
  Ghostty `font-family` fallback list). Two widths only: 1 cell and 2 cells.
- **Glyph sets**: two, selected by `icons = "nerd" | "unicode" | "ascii"`
  (config or `--icons`). Default is `unicode` — a font cannot be detected, so
  we default to glyphs every modern monospace font has (box drawing, ✓ ✗ ○
  ● ▸ ⚠ braille spinner) and let the user opt into Nerd Font icons. `ascii`
  is auto-selected when `TERM=linux`/`dumb` or `LANG` has no UTF-8.

| Meaning        | nerd   | unicode | ascii |
|----------------|--------|---------|-------|
| project        | `` | `▪`     | `#`   |
| session        | `` | `●`     | `@`   |
| subagent       | `` | `◦`     | `-`   |
| running        | spinner| braille spinner `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` | `\|/-\` |
| waiting        | `` | `?`     | `?`   |
| error          | `` | `✗`     | `x`   |
| done (wrapped) | `` | `✓`     | `ok`  |
| idle           | `` | `○`     | `.`   |
| focused pane   | `` | `▸`     | `>`   |
| interact mode  | `` | `⌨`     | `>>`  |
| claude / codex | `` `` | `C` / `X` letter badge | same |

Every glyph is emitted through one `theme.Icon(name)` function; nothing in
the views hardcodes a symbol. Widths are asserted in a table test (`uniseg`
via lipgloss `Width`) so a 2-cell icon can never shift a column.

- **Box drawing**: rounded corners `╭╮╰╯` for the focused pane, plain
  `┌┐└┘` for inactive, `═` double top rule for the interact-mode pane.
  ASCII mode: `+-|` everywhere (this is bungkus-cli's current default; mcc
  changes the default to unicode because it is a full-screen app that lives
  on screen for hours, and ASCII borders at 120 columns look like a spreadsheet).
- **No italics, no underline for meaning** (tmux and Apple Terminal render
  them unreliably). Bold is allowed for emphasis only, never as the sole
  carrier of state.
- **Hyperlinks**: OSC 8 for session ids and project paths when the terminal
  supports it (kitty, Ghostty, iTerm2, WezTerm; not Apple Terminal; tmux ≥ 3.4
  passes through). Text is identical either way; the link is a bonus.

## 4. Layout

Three columns, bento style. Widths at ≥ 100 columns: sidebar 22 / sessions
flexible min 34 / output flexible ≥ 40. The output pane gets all remaining
width because a terminal emulator inside it needs columns.

### 4.1 Main screen, 120×40 (dark theme rendering shown as text)

```
 bungkus-mcc  ▪ ~/Works/OSBR                                                        2 running · 1 needs you · v0.1.0
╭ projects ────────────╮┌ sessions · bungkus-mcc ──────────────────┐┌ output · claude #a3f1 ───────────────────────╮
│▸ bungkus-mcc    ● 2  ││ ● claude  #a3f1  "write proposal"   12m  ││                                              │
│  bungkus-cli    ? 1  ││   ⠹ working · Bash                       ││  ● I'll start by reading the sibling repo,   │
│  miko-chan           ││   ├ ◦ research claude hooks   ⠹ 3m       ││    then inspect local agent session data.    │
│  osbr-v2             ││   ├ ◦ research codex         ⠹ 3m       ││                                              │
│  aab                 ││   └ ◦ research go tui libs    ✓ 1m       ││  ⏺ Bash(ls ~/.claude/projects | head)         │
│  playground          ││                                          ││    ⎿  -Users-spencer-osbr                    │
│                      ││ ○ codex   #77c0  "fix flaky test"   idle ││       -Users-spencer-osbr-Documents           │
│                      ││   last: turn complete 41m ago            ││       ...                                    │
│                      ││                                          ││                                              │
│                      ││ ✓ claude  #9be2  "bump deps"    wrapped  ││  ● Now the local agent session data — schema │
│                      ││   38 tool calls · 2 subagents · 22m      ││    only, no content.                         │
│                      ││                                          ││                                              │
│                      ││                                          ││  ⏺ Agent(Research Claude Code observability) │
│                      ││                                          ││    ⎿  Running…                               │
│                      ││                                          ││                                              │
│                      ││                                          ││                                              │
│                      ││                                          ││                                              │
│                      ││                                          ││                                              │
│                      ││                                          ││                                              │
│                      ││                                          ││                                              │
│                      ││                                          ││                                              │
│                      ││                                          ││                                              │
│                      ││                                          ││                                              │
│                      ││                                          ││                                              │
│                      ││                                          ││                                              │
│                      ││                                          ││                                              │
│                      ││                                          ││ > █                                          │
│                      ││                                          ││                                              │
│                      ││                                          ││                                              │
│                      ││                                          ││                                              │
│                      ││                                          ││                                              │
│                      ││                                          ││                                              │
│                      ││                                          ││                                              │
│                      ││                                          ││                                              │
╰──────────────────────╯└──────────────────────────────────────────┘╰──────────────────────────────────────────────╯
 NORMAL  j/k move · enter open · n new session · i interact · x stop · / filter · ? help · q quit
```

Notes:
- Top line: app name, workspace path (OSC 8 link), global tallies right-aligned.
- Focused pane (`projects` here) has rounded corners in `border-focus` and a
  `▸` before the selected row. Selected row is also reverse-video, so focus +
  selection survive 16 colors.
- Session card = 2–3 lines + one line per subagent (tree glyphs `├ └`).
  Cards collapse to their first line when the pane is shorter than the content;
  the selected card never collapses.
- Bottom "getah" bar: mode word first (`NORMAL`, `INTERACT`, `FILTER`),
  then the keys relevant to the focused pane, generated from the keymap.

### 4.2 Interact mode (keys go to the agent)

```
╔ output · claude #a3f1 · INTERACT · ctrl-\ to leave ══════════════════════════════╗
║ ...agent's own TUI, rendered by the VT emulator...                                ║
╚═══════════════════════════════════════════════════════════════════════════════════╝
 INTERACT  keys go to claude · ctrl-\ back to mcc
```

Double-line border + `INTERACT` in the title + Kuning reverse-video mode word
in the getah bar + the getah bar text changes. Four independent signals; any
one is enough to tell where keystrokes go.

### 4.3 Narrow mode, 80×24

Below 100 columns the sidebar hides; below 60 the sessions pane also hides
and the screen becomes a single pane with a breadcrumb. `h`/`l` (or `←`/`→`,
`[`/`]`) move between the three panes as a stack.

```
 bungkus-mcc ▪ bungkus-mcc › sessions                    2 running · 1 needs you
┌ sessions ────────────────────────────────────────────────────────────────────┐
│ ● claude  #a3f1  "write proposal"                                   12m      │
│   ⠹ working · Bash                                                           │
│   ├ ◦ research claude hooks                              ⠹ 3m                │
│   ├ ◦ research codex                                     ⠹ 3m                │
│   └ ◦ research go tui libs                               ✓ 1m                │
│                                                                              │
│ ○ codex   #77c0  "fix flaky test"                                   idle     │
│   last: turn complete 41m ago                                                │
│                                                                              │
│ ✓ claude  #9be2  "bump deps"                                        wrapped  │
│   38 tool calls · 2 subagents · 22m                                          │
│                                                                              │
│                                                                              │
│                                                                              │
│                                                                              │
│                                                                              │
│                                                                              │
│                                                                              │
│                                                                              │
│                                                                              │
└──────────────────────────────────────────────────────────────────────────────┘
 NORMAL  h projects · l output · j/k move · enter open · n new · ? help
```

Minimum supported size is 60×16; below that we show a single centered line
"bungkus-mcc needs at least 60×16 (now 52×14)" and nothing else.

## 5. Card states

State is carried three ways at once: glyph, word, color. Border style adds a
fourth for `waiting` and `error`, the two states that need action.

| State     | Glyph (unicode/ascii) | Word       | Color   | Extra                                  |
|-----------|-----------------------|------------|---------|----------------------------------------|
| idle      | `○` / `.`             | idle       | fg-muted| whole card in fg-muted                 |
| running   | spinner / `\|/-\`     | working    | ok      | current tool name after `·`            |
| waiting   | `?` / `?`             | needs you  | warn    | card title bold; row also listed in header tally |
| error     | `✗` / `x`             | failed     | err     | last error line shown under the title  |
| done      | `✓` / `ok`            | wrapped    | fg      | summary line (tools, subagents, time)  |
| stopped   | `■` / `#`             | stopped    | fg-muted| "resume: r" hint                       |

Subagents use the same table, one indent level in, with `◦` instead of the
session glyph and no border. A subagent row never exceeds one line.

Project rows in the sidebar show the worst state among their sessions as a
single trailing badge: `? 1` (one needs you) beats `● 2` (two running) beats
nothing.

## 6. Motion

- One spinner for the whole app, ticked at 8 fps by a single `tea.Tick`; every
  running row reads the same frame. No spinner when no session is running
  (no wake-ups, no battery drain in the background).
- No animation on focus change or pane resize.
- Output pane repaints are coalesced by Bubble Tea's renderer (≤ 60 fps) and
  by our PTY reader chunking (32 KiB reads); synchronized-output (DEC 2026) is
  requested by Bubble Tea v2 where supported, which removes tearing on
  kitty/Ghostty/WezTerm/iTerm2.
- A newly wrapped session flashes its glyph once (two frames) in `accent`,
  then settles. That is the only "celebration".

## 7. Keybindings

Both vim motions and arrow keys, everywhere, always. The map lives in one
file (`internal/tui/keymap.go`, `bubbles/key.Binding`) and the help view is
generated from it; this table is the spec, the code is the source of truth.

### 7.1 Global (NORMAL mode)

| Keys                     | Action                                   |
|--------------------------|------------------------------------------|
| `h` `l` / `←` `→` / `[` `]` / `tab` `shift-tab` | focus previous/next pane |
| `1` `2` `3`              | focus projects / sessions / output       |
| `j` `k` / `↓` `↑`        | move within pane (rows, or scroll output)|
| `gg` / `G` / `home` `end`| first / last                             |
| `ctrl-d` `ctrl-u` / `pgdn` `pgup` | half page                       |
| `enter`                  | open: project → sessions pane; session → output pane |
| `/`                      | filter the focused list (FILTER mode; `esc` clears) |
| `?`                      | help overlay (generated from keymap)     |
| `w`                      | change workspace directory               |
| `t`                      | toggle dark / light theme                |
| `ctrl-l`                 | redraw                                   |
| `q` / `ctrl-c`           | quit (confirm if any session is running) |

`gg` is the only two-key sequence; a pending `g` is shown in the getah bar
and expires on the next key. `G` is uppercase, no chord.

### 7.2 Sessions pane

| Keys      | Action                                                  |
|-----------|---------------------------------------------------------|
| `n`       | new session (picker: claude / codex, optional prompt)   |
| `N`       | new project: runs `bungkus-cli` in the output pane      |
| `x`       | stop selected session (confirm)                         |
| `r`       | resume a stopped session (`claude --resume` / `codex resume`) |
| `d`       | forget a wrapped/stopped session (removes from the list only) |
| `space`   | collapse / expand subagents of the selected card        |
| `i` / `enter` on a live session | jump to output pane and enter INTERACT |

### 7.3 Output pane, NORMAL mode

| Keys              | Action                                        |
|-------------------|-----------------------------------------------|
| `i` / `enter`     | enter INTERACT mode                           |
| `j` `k` `ctrl-d` `ctrl-u` `gg` `G` | scroll the emulator's scrollback |
| `y`               | copy visible output to clipboard (OSC 52 when available) |
| `o`               | open the session transcript path with `$EDITOR` in the pane (stage 2) |

### 7.4 INTERACT mode

Every key goes to the agent's PTY, including `q`, `?`, `ctrl-c`, arrows,
mouse (if enabled), and paste. The one exception:

| Keys      | Action                                    |
|-----------|-------------------------------------------|
| `ctrl-\`  | leave INTERACT, back to NORMAL            |

Why `ctrl-\`: it is not bound by Claude Code or Codex defaults (checked
against Claude Code 2.1.285 and Codex 0.153 default keymaps — re-verify at
implementation, ASSUMPTION), not used by tmux/zellij/screen defaults
(`ctrl-b`, `ctrl-g`, `ctrl-a`), not a vim motion, and is delivered as a plain
byte (0x1C) even by terminals without the kitty keyboard protocol and through
tmux without `extended-keys`. In raw mode it is not SIGQUIT. If an agent
ever needs a literal `ctrl-\`, `ctrl-\ ctrl-\` within 500 ms forwards one;
nothing in stage 1 needs that, so it is documented but implemented only if
asked. `esc` is deliberately not the exit chord: agents use it constantly.

### 7.5 Mouse

Click focuses a pane and selects a row; wheel scrolls; click inside the
output pane while in NORMAL does not enter INTERACT (accidental focus steals
are worse than one extra keypress). In INTERACT mode, mouse events are
forwarded to the agent only if the agent enabled mouse reporting in the
emulator; otherwise the wheel scrolls our scrollback. Off entirely with
`--no-mouse`, because text selection in tmux/Alacritty is nicer without it.

## 8. Empty states and microcopy

Voice: short, warm, hawker-stall casual. English UI; a few Malay words that
are already the brand (bungkus, daun pisang) and nothing that needs a
glossary. No exclamation marks except the one below. Never blame the user.

| Where                    | Copy                                                              |
|--------------------------|-------------------------------------------------------------------|
| no workspace set         | `No workspace yet. Press w to pick a folder — any folder with projects in it.` |
| workspace has no projects| `Nothing to wrap here. Press N to create a project with bungkus-cli, or w to pick another folder.` |
| project has no sessions  | `No sessions. Press n to start one.`                              |
| session running, no output yet | `Warming up the wok…`                                       |
| output pane, nothing selected | `Pick a session on the left. Press i there to talk to it.`   |
| agent not installed      | `claude not found on PATH. Install Claude Code, then press n again.` |
| quit with running sessions | `2 sessions are still working. They will be stopped (you can resume them later). Quit? [y/N]` |
| session wrapped          | `Bungkus! #a3f1 wrapped in 22m — 38 tool calls, 2 subagents.`   |
| session failed           | `#a3f1 failed: <last error line>. Press i to see what happened.` |
| hook socket unavailable  | `Live subagent view is off for this session (socket error). Output still works.` |
| terminal too small       | `bungkus-mcc needs at least 60×16 (now 52×14).`                  |

Labels: "projects", "sessions", "output" — lowercase in pane titles, sentence
case in dialogs. Session ids are shown as the first 4 hex chars of the
agent's own session id, prefixed with `#`.

## 9. Accessibility checklist (per screen, at review)

- Every state has glyph + word + color; test with `NO_COLOR=1`.
- Focus is visible without color (border style + `▸`).
- Nothing important is only in the title bar's right-aligned tallies; the
  same information exists in the sidebar badges.
- Minimum 4.5:1 for all text tokens on both reference backgrounds (table above).
- Works with `--icons ascii` and in tmux with `TERM=screen-256color`.
- `--ax-screen-reader`-style plain mode is out of scope for stage 1; noted as
  an open question in PROPOSAL.md.
