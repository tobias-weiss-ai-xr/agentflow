package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

type TaskState string

const (
	StateReady   TaskState = "ready"
	StateRunning TaskState = "running"
	StateDone    TaskState = "done"
	StateFailed  TaskState = "failed"
)

type TaskStatus struct {
	State     TaskState `json:"state"`
	Attempts  int       `json:"attempts"`
	LastError string    `json:"last_error,omitempty"`
}

// Store: .af-state/ persistence. run-state.json is written atomically
// (tmp file + rename) so a torn write never leaves corrupt JSON.
type Store struct{ Dir string }

func (s Store) StatusFile() string   { return filepath.Join(s.Dir, "run-state.json") }
func (s Store) LogDir() string       { return filepath.Join(s.Dir, "logs") }
func (s Store) PromptDir() string    { return filepath.Join(s.Dir, "prompts") }
func (s Store) WorktreeRoot() string { return filepath.Join(s.Dir, "worktrees") }

func (s Store) Load() map[string]TaskStatus {
	m := map[string]TaskStatus{}
	data, err := os.ReadFile(s.StatusFile())
	if err != nil {
		return m
	}
	// tmp+rename makes torn reads near-impossible; a corrupt file falls
	// back to a fresh state rather than crashing the loop.
	_ = json.Unmarshal(data, &m)
	return m
}

func (s Store) Save(m map[string]TaskStatus) error {
	if err := os.MkdirAll(s.Dir, 0o755); err != nil {
		return err
	}
	data, err := json.MarshalIndent(m, "", "  ")
	if err != nil {
		return err
	}
	tmp := filepath.Join(s.Dir, fmt.Sprintf("run-state.json.tmp%d", os.Getpid()))
	if err := os.WriteFile(tmp, data, 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, s.StatusFile())
}

// Board: human-readable status table (rune-safe error truncation).
func Board(tasks []Task, status map[string]TaskStatus) string {
	lines := []string{fmt.Sprintf("%-12s %-9s %-8s %s", "TASK", "STATE", "ATTEMPTS", "LAST ERROR")}
	for _, t := range tasks {
		s := status[t.ID]
		lines = append(lines, fmt.Sprintf("%-12s %-9s %-8d %s",
			t.ID, s.State, s.Attempts, truncate(s.LastError, 48)))
	}
	return strings.Join(lines, "\n")
}

func truncate(s string, n int) string {
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	return string(r[:n]) + "..."
}
