package main

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
)

// git runs one git command in dir; combined output is returned and a
// non-zero exit becomes an error carrying that output.
func git(dir string, args ...string) (string, error) {
	cmd := exec.Command("git", args...)
	if dir != "" {
		cmd.Dir = dir
	}
	out, err := cmd.CombinedOutput()
	if err != nil {
		return string(out), fmt.Errorf("git %s: %w\n%s",
			strings.Join(args, " "), err, strings.TrimSpace(string(out)))
	}
	return string(out), nil
}

type Worktree struct {
	Path   string
	Branch string
}

// CreateWorktree: per-task worktree at wtRoot/<ID> on branch <prefix>/<ID>.
// Prunes stale worktree registrations FIRST (a raw-deleted worktree dir
// still holds its branch "checked out" in git's metadata), then clears a
// stale branch, so a dead run's leftovers can't block the add.
func CreateWorktree(repo, wtRoot, id, prefix string) (*Worktree, error) {
	if _, err := git(repo, "rev-parse", "--git-dir"); err != nil {
		return nil, fmt.Errorf("%s is not a git repository", repo)
	}
	if err := os.MkdirAll(wtRoot, 0o755); err != nil {
		return nil, err
	}
	branch := prefix + "/" + id
	path := filepath.Join(wtRoot, id)
	_ = os.RemoveAll(path)                   // stale dir from a dead run
	_, _ = git(repo, "worktree", "prune")    // stale registrations
	_, _ = git(repo, "branch", "-D", branch) // stale branch
	if out, err := git(repo, "worktree", "add", "-b", branch, path, "HEAD"); err != nil {
		return nil, fmt.Errorf("worktree add failed: %s", strings.TrimSpace(out))
	}
	return &Worktree{Path: path, Branch: branch}, nil
}

// RemoveWorktree: best-effort worktree + branch removal; never fails the caller.
func RemoveWorktree(repo string, wt *Worktree) {
	_, _ = git(repo, "worktree", "remove", "--force", wt.Path)
	_, _ = git(repo, "branch", "-D", wt.Branch)
}

var mergeMu sync.Mutex // serializes merges across parallel attempts

// MergeSerialized fast-forwards the orchestrator's current branch to branch.
// On failure (e.g. the base moved while the attempt ran) the attempt fails
// and the run loop retries on a fresh worktree — never force-pushes.
func MergeSerialized(repo, branch string) error {
	mergeMu.Lock()
	defer mergeMu.Unlock()
	out, err := git(repo, "merge", "--ff-only", branch)
	if err != nil {
		return fmt.Errorf("merge of %s failed: %s", branch, strings.TrimSpace(out))
	}
	return nil
}

// HealWorktrees removes every worktree dir under wtRoot (startup only —
// nothing can be running yet, stale running state was just reset).
func HealWorktrees(repo, wtRoot, prefix string) {
	entries, err := os.ReadDir(wtRoot)
	if err != nil {
		return
	}
	_, _ = git(repo, "worktree", "prune")
	for _, e := range entries {
		_, _ = git(repo, "worktree", "remove", "--force", filepath.Join(wtRoot, e.Name()))
		_, _ = git(repo, "branch", "-D", prefix+"/"+e.Name())
	}
}
