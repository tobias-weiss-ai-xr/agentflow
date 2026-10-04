// Package e2e proves the Go af port end-to-end (mirrors Rust tests/e2e.rs):
// it builds the af binary, runs it as a subprocess against a temp git repo
// fixture with a scripted fake agent (selected via the worker cli field),
// and asserts the lifecycle — dispatch, gate, ff-merge, retry, deadlock
// exit. No network, no LLM — deterministic CI.
package e2e

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

var afBin string

func TestMain(m *testing.M) {
	dir, err := os.MkdirTemp("", "af-go-e2e-bin")
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	defer os.RemoveAll(dir)
	afBin = filepath.Join(dir, "af")
	if runtime.GOOS == "windows" {
		afBin += ".exe"
	}
	out, err := exec.Command("go", "build", "-o", afBin, "..").CombinedOutput()
	if err != nil {
		fmt.Fprintf(os.Stderr, "build af: %v\n%s\n", err, out)
		os.Exit(1)
	}
	os.Exit(m.Run())
}

func git(t *testing.T, dir string, args ...string) {
	t.Helper()
	cmd := exec.Command("git", args...)
	cmd.Dir = dir
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git %v failed: %v\n%s", args, err, out)
	}
}

func writeFile(t *testing.T, path, content string, perm os.FileMode) {
	t.Helper()
	if err := os.WriteFile(path, []byte(content), perm); err != nil {
		t.Fatal(err)
	}
}

// agentScript returns the fake agent for the platform: it writes out.txt
// and commits (like a real agent), tolerating nothing-to-commit.
// kind "fail7" makes it exit 7 instead.
func agentScript(kind string) string {
	if runtime.GOOS == "windows" {
		if kind == "fail7" {
			return "@echo off\r\necho fake-agent failing 1>&2\r\nexit /b 7\r\n"
		}
		return "@echo off\r\n" +
			"echo fake-agent: task work > out.txt\r\n" +
			"git add -A\r\n" +
			"git commit -m \"fake agent: task work\"\r\n" +
			"exit /b 0\r\n"
	}
	if kind == "fail7" {
		return "#!/bin/sh\necho fake-agent failing >&2\nexit 7\n"
	}
	return "#!/bin/sh\necho 'fake-agent: task work' > out.txt\ngit add -A\ngit commit -m 'fake agent: task work' || true\nexit 0\n"
}

// gateCmd: exit 0 iff the named file exists in the worktree.
func gateCmd(name string) string {
	if runtime.GOOS == "windows" {
		return fmt.Sprintf("if exist %s (exit 0) else (exit 1)", name)
	}
	return fmt.Sprintf("test -f %s", name)
}

type fixture struct {
	dir, repo, state string
}

func newFixture(t *testing.T, tasksJSON, agentKind string, maxAttempts int) *fixture {
	t.Helper()
	f := &fixture{dir: t.TempDir()}
	f.repo = filepath.Join(f.dir, "repo")
	f.state = filepath.Join(f.dir, "state")
	if err := os.MkdirAll(f.repo, 0o755); err != nil {
		t.Fatal(err)
	}
	git(t, f.repo, "init", "-b", "main")
	git(t, f.repo, "config", "user.name", "af e2e")
	git(t, f.repo, "config", "user.email", "af@test")
	writeFile(t, filepath.Join(f.repo, "README.md"), "# scratch\n", 0o644)
	git(t, f.repo, "add", ".")
	git(t, f.repo, "commit", "-m", "init")

	cfgDir := filepath.Join(f.dir, "config")
	if err := os.MkdirAll(cfgDir, 0o755); err != nil {
		t.Fatal(err)
	}
	writeFile(t, filepath.Join(cfgDir, "tasks.json"), tasksJSON, 0o644)
	agent := filepath.Join(f.dir, "fake-agent.cmd")
	if runtime.GOOS != "windows" {
		agent = filepath.Join(f.dir, "fake-agent.sh")
	}
	writeFile(t, agent, agentScript(agentKind), 0o755)
	workersJSON := fmt.Sprintf(
		`{"workers":[{"name":"w1","provider":"test","model":"fake","cli":%q,"max_attempts":%d}]}`,
		agent, maxAttempts)
	writeFile(t, filepath.Join(cfgDir, "workers.json"), workersJSON, 0o644)
	return f
}

func (f *fixture) run(t *testing.T, args ...string) (string, int) {
	t.Helper()
	cmd := exec.Command(afBin, args...)
	cmd.Dir = f.dir
	cmd.Env = append(os.Environ(),
		"TF_REPO_DIR="+f.repo,
		"TF_STATE_DIR="+f.state,
		"TF_TASKS_JSON="+filepath.Join(f.dir, "config", "tasks.json"),
		"TF_WORKERS_JSON="+filepath.Join(f.dir, "config", "workers.json"),
		"TF_POLL=1",
	)
	out, err := cmd.CombinedOutput()
	code := 0
	if err != nil {
		code = 1
		var ee *exec.ExitError
		if errors.As(err, &ee) {
			code = ee.ExitCode()
		}
	}
	return string(out), code
}

type taskStatus struct {
	State     string `json:"state"`
	Attempts  int    `json:"attempts"`
	LastError string `json:"last_error"`
}

func (f *fixture) readState(t *testing.T) map[string]taskStatus {
	t.Helper()
	data, err := os.ReadFile(filepath.Join(f.state, "run-state.json"))
	if err != nil {
		t.Fatalf("read run-state.json: %v", err)
	}
	var m map[string]taskStatus
	if err := json.Unmarshal(data, &m); err != nil {
		t.Fatalf("parse run-state.json: %v", err)
	}
	return m
}

// / Happy path: agent creates out.txt, gate passes, both tasks (A, then dep B)
// / are done and merged into the main repo branch; a second run is a no-op.
func TestHappyPathMergesAndSecondRunIsNoOp(t *testing.T) {
	gate := gateCmd("out.txt")
	f := newFixture(t, fmt.Sprintf(
		`[{"id":"A","title":"create out","scope":["out.txt"],"accept":%q},
		  {"id":"B","title":"follow on","deps":["A"],"scope":["out.txt"],"accept":%q}]`,
		gate, gate), "", 1)

	out, code := f.run(t, "run")
	if code != 0 {
		t.Fatalf("run exit=%d, want 0\n%s", code, out)
	}
	st := f.readState(t)
	if st["A"].State != "done" || st["B"].State != "done" {
		t.Fatalf("want A+B done, got %+v", st)
	}
	if st["A"].Attempts != 1 || st["B"].Attempts != 1 {
		t.Fatalf("want 1 attempt each, got %+v", st)
	}
	if _, err := os.Stat(filepath.Join(f.repo, "out.txt")); err != nil {
		t.Fatalf("out.txt not merged into main repo: %v", err)
	}
	if _, err := os.Stat(filepath.Join(f.state, "worktrees", "A")); !os.IsNotExist(err) {
		t.Fatalf("worktree A not removed after merge")
	}

	// Second run: everything already done → immediate no-op.
	out2, code2 := f.run(t, "run")
	if code2 != 0 {
		t.Fatalf("second run exit=%d, want 0\n%s", code2, out2)
	}
	st2 := f.readState(t)
	if st2["A"].Attempts != 1 || st2["B"].Attempts != 1 {
		t.Fatalf("second run dispatched again: %+v", st2)
	}

	// status renders the board.
	out3, code3 := f.run(t, "status")
	if code3 != 0 || !strings.Contains(out3, "done") {
		t.Fatalf("status exit=%d, board=%q", code3, out3)
	}
}

// / Failing gate: agent succeeds but the gate demands a file it never
// / creates → attempts increment to max_attempts, final state failed,
// / run exits 2, nothing merged.
func TestFailingGateIncrementsAttemptsAndFails(t *testing.T) {
	gate := gateCmd("NEVER.txt")
	f := newFixture(t, fmt.Sprintf(
		`[{"id":"A","title":"blocked","scope":["out.txt"],"accept":%q}]`, gate), "", 3)

	out, code := f.run(t, "run")
	if code != 2 {
		t.Fatalf("run exit=%d, want 2\n%s", code, out)
	}
	st := f.readState(t)
	if st["A"].State != "failed" {
		t.Fatalf("want failed, got %+v", st)
	}
	if st["A"].Attempts != 3 {
		t.Fatalf("want 3 attempts, got %+v", st)
	}
	if !strings.Contains(st["A"].LastError, "acceptance gate failed") {
		t.Fatalf("last_error should name the gate: %+v", st["A"])
	}
	if _, err := os.Stat(filepath.Join(f.repo, "out.txt")); !os.IsNotExist(err) {
		t.Fatalf("nothing should have been merged")
	}
}

// / Failing agent: exit 7 twice (max_attempts=2) → failed, exit 2.
func TestFailingAgentExits2(t *testing.T) {
	gate := gateCmd("out.txt")
	f := newFixture(t, fmt.Sprintf(
		`[{"id":"A","title":"boom","scope":["out.txt"],"accept":%q}]`, gate), "fail7", 2)

	out, code := f.run(t, "run")
	if code != 2 {
		t.Fatalf("run exit=%d, want 2\n%s", code, out)
	}
	st := f.readState(t)
	if st["A"].State != "failed" || st["A"].Attempts != 2 {
		t.Fatalf("want failed after 2 attempts, got %+v", st)
	}
	if !strings.Contains(st["A"].LastError, "agent exited 7") {
		t.Fatalf("last_error should carry the agent exit code: %+v", st["A"])
	}
}
