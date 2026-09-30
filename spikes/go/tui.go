package main

import (
	"fmt"
	"os"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
)

const listWidth = 24

// redrawMsg tells the model that some emulator consumed new output.
type redrawMsg struct{}

// model is the interactive two-pane UI.
type model struct {
	sessions []*session
	sel      int
	focused  bool
	scroll   int // lines scrolled back in the selected session; 0 = live
	w, h     int
}

// runInteractive starts claude and codex in the current directory and runs
// the TUI until q. Both children are killed on exit. It returns an error if
// the cwd is unreadable or Bubble Tea fails; a failed spawn is shown in the
// pane instead of aborting.
func runInteractive() error {
	cwd, err := os.Getwd()
	if err != nil {
		return err
	}
	redraw := func() {
		if p := program.Load(); p != nil {
			p.Send(redrawMsg{})
		}
	}
	m := &model{}
	for _, name := range []string{"claude", "codex"} {
		s, err := startSession(name, []string{name}, cwd, os.Environ(), 80, 24, redraw)
		if err != nil {
			s = &session{name: name, exited: make(chan struct{})}
			close(s.exited)
			fmt.Fprintf(os.Stderr, "proto: %s: %v\n", name, err)
		}
		m.sessions = append(m.sessions, s)
	}
	defer func() {
		for _, s := range m.sessions {
			if s.emu != nil {
				s.kill()
			}
		}
	}()
	p := tea.NewProgram(m)
	program.Store(p)
	_, err = p.Run()
	return err
}

// Init implements tea.Model.
func (m *model) Init() tea.Cmd { return nil }

// paneSize is the emulator size that fits inside the bordered right pane.
func (m *model) paneSize() (int, int) { return m.w - listWidth - 2, m.h - 2 }

// cur returns the selected session.
func (m *model) cur() *session { return m.sessions[m.sel] }

// Update implements tea.Model: list navigation when the list is focused,
// raw forwarding of every key and paste to the agent when the pane is.
// ctrl+\ always returns to the list.
func (m *model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.w, m.h = msg.Width, msg.Height
		c, r := m.paneSize()
		for _, s := range m.sessions {
			if s.emu != nil {
				s.resize(c, r)
			}
		}
	case tea.MouseWheelMsg:
		if msg.X < listWidth || m.cur().emu == nil {
			break
		}
		switch msg.Button {
		case tea.MouseWheelUp:
			m.scroll = min(m.scroll+3, m.cur().emu.ScrollbackLen())
		case tea.MouseWheelDown:
			m.scroll = max(m.scroll-3, 0)
		}
	case tea.PasteMsg:
		if m.focused && m.cur().emu != nil {
			m.scroll = 0
			m.cur().emu.Paste(msg.Content)
		}
	case tea.KeyPressMsg:
		if m.focused {
			if msg.String() == "ctrl+\\" {
				m.focused = false
			} else if m.cur().emu != nil {
				m.scroll = 0
				sendKey(m.cur().emu, tea.Key(msg))
			}
			break
		}
		switch msg.String() {
		case "q", "ctrl+c":
			return m, tea.Quit
		case "j", "down":
			m.sel = (m.sel + 1) % len(m.sessions)
			m.scroll = 0
		case "k", "up":
			m.sel = (m.sel + len(m.sessions) - 1) % len(m.sessions)
			m.scroll = 0
		case "l", "right", "enter", "tab":
			m.focused = true
		}
	}
	return m, nil
}

// paneContent renders the selected session's screen, or a window into
// scrollback+screen when scrolled back.
func (m *model) paneContent() string {
	s := m.cur()
	if s.emu == nil {
		return "failed to start " + s.name
	}
	screen := s.emu.Render()
	if m.scroll == 0 {
		return screen
	}
	// ponytail: reads scrollback lines without the emulator lock; fine for a
	// spike, a real build copies them under the lock.
	var lines []string
	for _, l := range s.emu.Scrollback().Lines() {
		lines = append(lines, l.Render())
	}
	lines = append(lines, strings.Split(screen, "\n")...)
	_, rows := m.paneSize()
	end := max(len(lines)-m.scroll, rows)
	return strings.Join(lines[max(end-rows, 0):min(end, len(lines))], "\n")
}

// View implements tea.View: a 24-col session list and a bordered pane
// (double border while focused), with the agent's cursor shown when focused.
func (m *model) View() tea.View {
	if m.w == 0 {
		return tea.NewView("")
	}
	var list strings.Builder
	for i, s := range m.sessions {
		mark, state := "  ", ""
		if i == m.sel {
			mark = "> "
		}
		if !s.alive() {
			state = " (exited)"
		}
		fmt.Fprintf(&list, "%s%s%s\n", mark, s.name, state)
	}
	list.WriteString("\nj/k select  l focus\nctrl+\\ back  q quit")
	if m.scroll > 0 {
		fmt.Fprintf(&list, "\n\nscrolled back %d", m.scroll)
	}
	left := lipgloss.NewStyle().Width(listWidth).Height(m.h).Render(list.String())

	border := lipgloss.NormalBorder()
	if m.focused {
		border = lipgloss.DoubleBorder()
	}
	right := lipgloss.NewStyle().Border(border).Width(m.w - listWidth).Height(m.h).
		Render(m.paneContent())

	v := tea.NewView(lipgloss.JoinHorizontal(lipgloss.Top, left, right))
	v.AltScreen = true
	v.MouseMode = tea.MouseModeCellMotion
	if m.focused && m.scroll == 0 && m.cur().emu != nil && m.cur().alive() {
		pos := m.cur().emu.CursorPosition()
		v.Cursor = tea.NewCursor(listWidth+1+pos.X, 1+pos.Y)
	}
	return v
}
