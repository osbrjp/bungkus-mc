package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// caseFile is the harness case format from spikes/SPEC.md §2.
type caseFile struct {
	Name  string
	Cmd   []string
	Cwd   string
	Env   map[string]string
	Steps []step
}

// step holds exactly one action; the non-nil field says which.
type step struct {
	Wait     *int
	Bytes    *string
	Key      *string
	Paste    *string
	Resize   []int
	Dump     *string
	Waitexit *int
}

// runHeadless runs the case at casePath on a cols×rows PTY and writes dumps
// and timing.json into outDir. It returns an error for an unreadable or
// invalid case, an unknown key name, a spawn failure or a write failure. A
// child that outlives the steps is killed.
func runHeadless(casePath, outDir string, cols, rows int) error {
	raw, err := os.ReadFile(casePath)
	if err != nil {
		return err
	}
	var c caseFile
	if err := json.Unmarshal(raw, &c); err != nil {
		return fmt.Errorf("%s: %w", casePath, err)
	}
	if len(c.Cmd) == 0 {
		return fmt.Errorf("%s: empty cmd", casePath)
	}
	if err := os.MkdirAll(outDir, 0o755); err != nil {
		return err
	}
	env := os.Environ()
	for k, v := range c.Env {
		env = append(env, k+"="+v)
	}

	s, err := startSession(c.Name, c.Cmd, c.Cwd, env, cols, rows, nil)
	if err != nil {
		return err
	}
	defer s.kill()

	for i, st := range c.Steps {
		switch {
		case st.Wait != nil:
			time.Sleep(time.Duration(*st.Wait) * time.Millisecond)
		case st.Bytes != nil:
			s.emu.SendText(*st.Bytes)
		case st.Key != nil:
			k, ok := namedKeys[*st.Key]
			if !ok {
				return fmt.Errorf("step %d: unknown key %q", i, *st.Key)
			}
			sendKey(s.emu, k)
		case st.Paste != nil:
			s.emu.Paste(*st.Paste)
		case len(st.Resize) == 2:
			s.resize(st.Resize[0], st.Resize[1])
		case st.Dump != nil:
			if err := dump(s, filepath.Join(outDir, *st.Dump)); err != nil {
				return err
			}
		case st.Waitexit != nil:
			select {
			case <-s.exited:
			case <-time.After(time.Duration(*st.Waitexit) * time.Millisecond):
				_ = s.cmd.Process.Kill()
				<-s.exited
			}
		default:
			return fmt.Errorf("step %d: unrecognised step", i)
		}
	}
	wall := time.Since(s.start)

	timing := map[string]any{"wall_ms": wall.Milliseconds(), "child_exit_ms": nil, "drained_ms": nil}
	if !s.alive() {
		timing["child_exit_ms"] = s.exitAt.Sub(s.start).Milliseconds()
		select {
		case <-s.drained:
			timing["drained_ms"] = s.drainAt.Sub(s.start).Milliseconds()
		case <-time.After(time.Second):
		}
	}
	return writeJSON(filepath.Join(outDir, "timing.json"), timing)
}

// dump writes base.txt (screen text), base.cursor ("row col", 0-based) and
// base.meta.json ({"alt_screen": bool}) for the current emulator state.
func dump(s *session, base string) error {
	text := strings.Join(screenLines(s.emu), "\n") + "\n"
	if err := os.WriteFile(base+".txt", []byte(text), 0o644); err != nil {
		return err
	}
	pos := s.emu.CursorPosition()
	if err := os.WriteFile(base+".cursor", fmt.Appendf(nil, "%d %d\n", pos.Y, pos.X), 0o644); err != nil {
		return err
	}
	return writeJSON(base+".meta.json", map[string]bool{"alt_screen": s.emu.IsAltScreen()})
}

// writeJSON marshals v and writes it to path.
func writeJSON(path string, v any) error {
	b, err := json.Marshal(v)
	if err != nil {
		return err
	}
	return os.WriteFile(path, append(b, '\n'), 0o644)
}
