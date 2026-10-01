// Command proto is the M0 Go spike for mc's embedded terminal pane: two
// agent sessions (claude, codex) running in PTYs, parsed by charmbracelet/x/vt
// and shown in a Bubble Tea v2 TUI. With --headless it instead runs one case
// file for the comparison harness (see spikes/SPEC.md).
package main

import (
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strings"
	"sync/atomic"
	"time"

	tea "charm.land/bubbletea/v2"
	uv "github.com/charmbracelet/ultraviolet"
	"github.com/charmbracelet/x/vt"
	"github.com/creack/pty"
)

// session is one child process on a PTY with its emulator. The emulator is a
// SafeEmulator because the reader goroutine writes to it while the UI (or the
// headless step runner) reads from it.
type session struct {
	name    string
	emu     *vt.SafeEmulator
	ptmx    *os.File
	cmd     *exec.Cmd
	start   time.Time
	exited  chan struct{} // closed once the child has been reaped
	drained chan struct{} // closed once the PTY reader hit EOF
	exitAt  time.Time
	drainAt time.Time
}

// startSession spawns argv in dir with env on a cols×rows PTY and wires up
// three goroutines: the reader (PTY → emulator), the pump (emulator replies
// and our input → PTY) and the reaper (cmd.Wait). onOutput, if non-nil, is
// called after every chunk the emulator consumes. It returns an error only
// if the process cannot be started.
//
// The pump is mandatory: x/vt writes query replies (DSR, DA, OSC 10/11) into
// an unbuffered pipe, so Write blocks until someone reads it.
func startSession(name string, argv []string, dir string, env []string, cols, rows int, onOutput func()) (*session, error) {
	cmd := exec.Command(argv[0], argv[1:]...)
	cmd.Dir, cmd.Env = dir, env
	s := &session{name: name, cmd: cmd, start: time.Now(), exited: make(chan struct{}), drained: make(chan struct{})}
	ptmx, err := pty.StartWithSize(cmd, &pty.Winsize{Cols: uint16(cols), Rows: uint16(rows)})
	if err != nil {
		return nil, err
	}
	s.ptmx, s.emu = ptmx, vt.NewSafeEmulator(cols, rows)

	go func() { _, _ = io.Copy(ptmx, s.emu) }()
	go func() {
		buf := make([]byte, 32*1024)
		for {
			n, err := ptmx.Read(buf)
			if n > 0 {
				_, _ = s.emu.Write(buf[:n])
				if onOutput != nil {
					onOutput()
				}
			}
			if err != nil {
				break
			}
		}
		s.drainAt = time.Now()
		close(s.drained)
	}()
	go func() {
		_ = cmd.Wait()
		s.exitAt = time.Now()
		close(s.exited)
	}()
	return s, nil
}

// resize resizes the emulator and the PTY (the kernel sends SIGWINCH).
func (s *session) resize(cols, rows int) {
	if cols < 1 || rows < 1 {
		return
	}
	s.emu.Resize(cols, rows)
	_ = pty.Setsize(s.ptmx, &pty.Winsize{Cols: uint16(cols), Rows: uint16(rows)})
}

// alive reports whether the child has not been reaped yet.
func (s *session) alive() bool {
	select {
	case <-s.exited:
		return false
	default:
		return true
	}
}

// kill terminates the child and releases the PTY and emulator.
func (s *session) kill() {
	if s.alive() {
		_ = s.cmd.Process.Kill()
	}
	_ = s.ptmx.Close()
	_ = s.emu.Close()
}

// sendKey encodes k for the child and queues it on the emulator's input pipe.
// This is the single key-encoding path for both modes. x/vt's SendKey covers
// legacy xterm sequences (C0 ctrl chords, DECCKM-aware arrows, F-keys,
// shift+tab, alt as an ESC prefix); on top of it:
//   - shift+enter → ESC CR, the "newline" Claude Code accepts without kitty;
//   - printable text (incl. shifted and IME-composed input) → its UTF-8,
//     because SendKey drops any key that carries a modifier it doesn't know.
//
// Limits: no kitty/CSI-u or modifyOtherKeys encoding, so modified arrows and
// ctrl+enter etc. are dropped silently.
func sendKey(emu *vt.SafeEmulator, k tea.Key) {
	switch {
	case k.Code == tea.KeyEnter && k.Mod == tea.ModShift:
		emu.SendText("\x1b\r")
	case k.Text != "" && k.Mod&^tea.ModShift == 0:
		emu.SendText(k.Text)
	default:
		emu.SendKey(uv.KeyPressEvent{Code: k.Code, Mod: k.Mod})
	}
}

// namedKeys maps the case-file key names to the keys Bubble Tea would deliver.
var namedKeys = map[string]tea.Key{
	"enter":       {Code: tea.KeyEnter},
	"shift+enter": {Code: tea.KeyEnter, Mod: tea.ModShift},
	"esc":         {Code: tea.KeyEscape},
	"tab":         {Code: tea.KeyTab},
	"shift+tab":   {Code: tea.KeyTab, Mod: tea.ModShift},
	"up":          {Code: tea.KeyUp},
	"down":        {Code: tea.KeyDown},
	"left":        {Code: tea.KeyLeft},
	"right":       {Code: tea.KeyRight},
	"ctrl+c":      {Code: 'c', Mod: tea.ModCtrl},
	"ctrl+d":      {Code: 'd', Mod: tea.ModCtrl},
	"backspace":   {Code: tea.KeyBackspace},
}

// screenLines returns the visible screen as plain text, one right-trimmed
// string per row. Wide characters occupy one rune; their spacer cell is
// skipped by uv.Line.String.
func screenLines(emu *vt.SafeEmulator) []string {
	w, h := emu.Width(), emu.Height()
	out := make([]string, h)
	for y := range h {
		line := make(uv.Line, w)
		for x := range w {
			if c := emu.CellAt(x, y); c != nil {
				line[x] = *c
			}
		}
		out[y] = strings.TrimRight(line.String(), " ")
	}
	return out
}

func main() {
	headless := flag.Bool("headless", false, "run one case file without UI")
	cols := flag.Int("cols", 120, "headless: PTY columns")
	rows := flag.Int("rows", 34, "headless: PTY rows")
	casePath := flag.String("case", "", "headless: case JSON file")
	outDir := flag.String("out", "", "headless: output directory")
	flag.Parse()

	if *headless {
		if err := runHeadless(*casePath, *outDir, *cols, *rows); err != nil {
			fmt.Fprintln(os.Stderr, "proto:", err)
			os.Exit(1)
		}
		return
	}
	if err := runInteractive(); err != nil {
		fmt.Fprintln(os.Stderr, "proto:", err)
		os.Exit(1)
	}
}

// program is set once the Bubble Tea program exists, so reader goroutines
// can request a redraw.
var program atomic.Pointer[tea.Program]
