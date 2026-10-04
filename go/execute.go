package main

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"
)

// logLine appends one entry to the task's per-attempt log.
func logLine(logPath, format string, a ...any) {
	f, err := os.OpenFile(logPath, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
	if err != nil {
		return
	}
	defer f.Close()
	fmt.Fprintf(f, format+"\n", a...)
}

// runShell runs a user-authored command (acceptance gate) through the
// platform shell with a hard timeout: cmd /C on Windows, sh -c otherwise.
// Gates are trusted: full inherited environment.
func runShell(command, dir string, timeout time.Duration) (string, int, error) {
	shell, flag := "sh", "-c"
	if runtime.GOOS == "windows" {
		shell, flag = "cmd", "/C"
	}
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	cmd := exec.CommandContext(ctx, shell, flag, command)
	cmd.Dir = dir
	out, err := cmd.CombinedOutput()
	code := 0
	if err != nil {
		var ee *exec.ExitError
		if errors.As(err, &ee) {
			// Non-zero exit is the gate's verdict (data), not an error.
			code = ee.ExitCode()
			err = nil
		} else if ctx.Err() == context.DeadlineExceeded {
			err = fmt.Errorf("timeout after %s", timeout)
		}
	}
	return string(out), code, err
}

// agentEnv: sandbox layer 1 — the agent child sees only OS essentials,
// this worker's api_key_env, and af's git identity. Everything else in the
// orchestrator environment is withheld.
func agentEnv(w *Worker) []string {
	allow := []string{
		"PATH", "HOME", "USERPROFILE", "TEMP", "TMP", "SYSTEMROOT", "WINDIR",
		"COMSPEC", "APPDATA", "LOCALAPPDATA", "PROGRAMFILES",
	}
	if w.APIKeyEnv != "" {
		allow = append(allow, w.APIKeyEnv)
	}
	var env []string
	for _, k := range allow {
		if v, ok := os.LookupEnv(k); ok {
			env = append(env, k+"="+v)
		}
	}
	return append(env,
		"GIT_AUTHOR_NAME=af", "GIT_AUTHOR_EMAIL=af@agentflow.local",
		"GIT_COMMITTER_NAME=af", "GIT_COMMITTER_EMAIL=af@agentflow.local",
		"GIT_TERMINAL_PROMPT=0", // git hygiene: no credential prompts
	)
}

// agentCmd wraps .cmd/.bat CLIs in cmd /C (CreateProcess won't execute
// batch files directly).
func agentCmd(cli string, args []string) (string, []string) {
	if runtime.GOOS == "windows" {
		switch strings.ToLower(filepath.Ext(cli)) {
		case ".cmd", ".bat":
			return "cmd", append([]string{"/C", cli}, args...)
		}
	}
	return cli, args
}

// agentInvocation builds the agent dispatch for one attempt. With
// w.Command set, {prompt} is substituted with the absolute prompt file
// path and the string runs through the platform shell in the task
// worktree; otherwise the default pi-shaped CLI line is built.
func agentInvocation(w *Worker, promptPath string) (string, []string) {
	if w.Command != "" {
		shell, flag := "sh", "-c"
		if runtime.GOOS == "windows" {
			shell, flag = "cmd", "/C"
		}
		return shell, []string{flag, strings.ReplaceAll(w.Command, "{prompt}", promptPath)}
	}
	return agentCmd(w.CLI, []string{"--provider", w.Provider, "--model", w.Model, "-p", "@" + promptPath})
}

const defaultPrompt = `You are an autonomous coding agent working in a git worktree.

TASK: {{TASK}}

Files you are allowed to modify (scope):
{{SCOPE}}

Acceptance criteria (the orchestrator will run this gate):
{{ACCEPTANCE}}

You are worker {{WORKER}}, attempt {{ATTEMPT}}.
Work on the task only; do not touch files outside the scope. When the
acceptance criteria hold, commit your changes on the current branch.`

// renderPrompt substitutes {{TASK}} {{SCOPE}} {{ACCEPTANCE}} {{WORKER}}
// {{ATTEMPT}} into prompts/worker.md (default template if absent).
func renderPrompt(st *Settings, t *Task, w *Worker, attempt int) string {
	tpl, err := os.ReadFile(st.PromptFile)
	if err != nil {
		tpl = []byte(defaultPrompt)
	}
	scope := "*"
	if len(t.Scope) > 0 {
		scope = strings.Join(t.Scope, "\n")
	}
	acc := t.AcceptanceProse
	if acc == "" {
		acc = "(declared acceptance gate command)"
	}
	return strings.NewReplacer(
		"{{TASK}}", t.ID,
		"{{SCOPE}}", scope,
		"{{ACCEPTANCE}}", acc,
		"{{WORKER}}", w.Name,
		"{{ATTEMPT}}", fmt.Sprint(attempt),
	).Replace(string(tpl))
}

// runAttempt performs ONE attempt: worktree → prompt → agent → gate →
// ff-merge. Retry/status transitions belong to the run loop. The worktree
// is always removed; a nil error means the attempt was merged.
func runAttempt(st *Settings, cfg *Config, t *Task, w *Worker, attempt int, logPath string) (err error) {
	logLine(logPath, "== attempt %d on worker %s (%s) ==", attempt, w.Name, w.Model)
	wt, err := CreateWorktree(st.RepoDir, st.WorktreeRoot, t.ID, st.BranchPrefix)
	if err != nil {
		return err
	}
	defer func() {
		RemoveWorktree(st.RepoDir, wt)
		if err != nil {
			logLine(logPath, "-- attempt failed: %v", err)
		} else {
			logLine(logPath, "-- merged --")
		}
	}()

	// 1) Render + write prompt (absolute: the path crosses into the agent's
	// cwd, which is the worktree).
	promptDir := (Store{Dir: st.StateDir}).PromptDir()
	if err := os.MkdirAll(promptDir, 0o755); err != nil {
		return err
	}
	promptPath, err := filepath.Abs(filepath.Join(promptDir, t.ID+".md"))
	if err != nil {
		return err
	}
	if err := os.WriteFile(promptPath, []byte(renderPrompt(st, t, w, attempt)), 0o644); err != nil {
		return fmt.Errorf("cannot write prompt: %w", err)
	}

	// 2) Agent dispatch: w.Command shell template, or the default
	// <cli> --provider P --model M -p @<promptPath>.
	agentTimeout := time.Duration(firstPositive(st.AgentTimeoutS, cfg.Defaults.AgentTimeoutS, 3600)) * time.Second
	cli, cliArgs := agentInvocation(w, promptPath)
	ctx, cancel := context.WithTimeout(context.Background(), agentTimeout)
	defer cancel()
	cmd := exec.CommandContext(ctx, cli, cliArgs...)
	cmd.Dir = wt.Path
	cmd.Env = agentEnv(w)
	out, err := cmd.CombinedOutput()
	logLine(logPath, "-- agent --\n%s", strings.TrimRight(string(out), "\n"))
	if err != nil {
		if ctx.Err() == context.DeadlineExceeded {
			return fmt.Errorf("agent timed out after %s", agentTimeout)
		}
		code := -1
		var ee *exec.ExitError
		if errors.As(err, &ee) {
			code = ee.ExitCode()
		}
		return fmt.Errorf("agent exited %d", code)
	}

	// 3) Acceptance gate (cwd = worktree; nonzero exit = failed attempt).
	if t.Accept != "" {
		acceptTimeout := time.Duration(firstPositive(st.AcceptTimeoutS, cfg.Defaults.AcceptTimeoutS, 600)) * time.Second
		gout, code, gerr := runShell(t.Accept, wt.Path, acceptTimeout)
		logLine(logPath, "-- gate --\n%s", strings.TrimRight(gout, "\n"))
		if gerr != nil {
			return fmt.Errorf("acceptance gate error: %w", gerr)
		}
		if code != 0 {
			return fmt.Errorf("acceptance gate failed (exit %d): %s", code, strings.TrimSpace(gout))
		}
	}

	// 4) ff-merge into the orchestrator's current branch (serialized).
	return MergeSerialized(st.RepoDir, wt.Branch)
}
