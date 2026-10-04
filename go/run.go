package main

import (
	"fmt"
	"os"
	"path/filepath"
	"time"
)

type attemptResult struct {
	id          string
	worker      string
	maxAttempts int
	err         error
}

// RunLoop: poll → reap → dispatch until every task is done or no task can
// make progress (deadlock). Returns the process exit code: 0 all done,
// 2 deadlock/failure. State transitions are persisted before dispatch
// (crash-safety) and after every reap.
func RunLoop(cfg *Config, st *Settings, once bool) int {
	for _, w := range cfg.Warnings {
		fmt.Fprintf(os.Stderr, "warning: %s\n", w)
	}
	store := Store{Dir: st.StateDir}
	status := store.Load()

	// Self-heal: a previous process may have died mid-run — reset stale
	// running entries, clear leftover worktrees.
	reset := 0
	for id, s := range status {
		if s.State != StateRunning {
			continue
		}
		s.State = StateReady
		s.LastError = "previous run interrupted"
		status[id] = s
		reset++
	}
	if reset > 0 {
		fmt.Printf("self-heal: reset %d stale running task(s)\n", reset)
	}
	HealWorktrees(st.RepoDir, st.WorktreeRoot, st.BranchPrefix)
	_ = store.Save(status)
	_ = os.MkdirAll(store.LogDir(), 0o755)
	_ = os.MkdirAll(store.PromptDir(), 0o755)

	enabled := 0
	for i := range cfg.Workers {
		if cfg.Workers[i].IsEnabled() {
			enabled++
		}
	}
	maxParallel := firstPositive(st.MaxParallel, cfg.MaxParallel, enabled, 1)

	// All shared state below is owned by this goroutine; attempt goroutines
	// only send results over the channel.
	results := make(chan attemptResult)
	running := map[string]bool{}
	busy := map[string]bool{}

	reap := func() {
		for {
			select {
			case r := <-results:
				delete(running, r.id)
				busy[r.worker] = false
				s := status[r.id]
				s.Attempts++
				switch {
				case r.err == nil:
					s.State = StateDone
					s.LastError = ""
					fmt.Printf("  ✓ %s done (attempt %d)\n", r.id, s.Attempts)
				case s.Attempts >= r.maxAttempts:
					s.State = StateFailed
					s.LastError = r.err.Error()
					fmt.Printf("  ✗ %s failed (attempt %d/%d): %v\n", r.id, s.Attempts, r.maxAttempts, r.err)
				default:
					s.State = StateReady
					s.LastError = r.err.Error()
					fmt.Printf("  ✗ %s attempt %d failed, retrying: %v\n", r.id, s.Attempts, r.err)
				}
				status[r.id] = s
				_ = store.Save(status)
			default:
				return
			}
		}
	}

	ready := func() []Task {
		var out []Task
		for _, t := range cfg.Tasks {
			if s, ok := status[t.ID]; ok && s.State != StateReady {
				continue
			}
			depsDone := true
			for _, d := range t.Deps {
				if ds, ok := status[d]; !ok || ds.State != StateDone {
					depsDone = false
					break
				}
			}
			if depsDone {
				out = append(out, t)
			}
		}
		return out
	}

	pickFree := func() *Worker {
		for i := range cfg.Workers {
			w := &cfg.Workers[i]
			if w.IsEnabled() && !busy[w.Name] {
				return w
			}
		}
		return nil
	}

	fmt.Printf("af run: %d tasks, %d enabled worker(s), max_parallel=%d\n",
		len(cfg.Tasks), enabled, maxParallel)
	poll := time.Duration(firstPositive(st.PollSecs, 1)) * time.Second
	for {
		reap()

		allDone := true
		for _, t := range cfg.Tasks {
			if s, ok := status[t.ID]; !ok || s.State != StateDone {
				allDone = false
				break
			}
		}
		if allDone {
			fmt.Println("\nAll tasks done.")
			fmt.Println(Board(cfg.Tasks, status))
			return 0
		}

		// Deadlock: nothing in flight and nothing ready (unmet deps that
		// can never resolve, or a failed task) → exit 2, never hang.
		if len(running) == 0 && len(ready()) == 0 {
			fmt.Fprintf(os.Stderr, "DEADLOCK: no task can make progress.\n%s\n", Board(cfg.Tasks, status))
			return 2
		}

		for _, t := range ready() {
			if len(running) >= maxParallel {
				break
			}
			w := pickFree()
			if w == nil {
				break
			}
			// Mark Running + persist BEFORE spawning (crash-safety).
			s := status[t.ID]
			s.State = StateRunning
			status[t.ID] = s
			_ = store.Save(status)
			running[t.ID] = true
			busy[w.Name] = true
			attempt := s.Attempts + 1
			budget := cfg.AttemptBudget(w)
			logPath := filepath.Join(store.LogDir(), t.ID+".log")
			fmt.Printf("  → %s dispatch on %s (%s) [attempt %d]\n", t.ID, w.Name, w.Model, attempt)
			task, worker := t, *w
			go func() {
				err := runAttempt(st, cfg, &task, &worker, attempt, logPath)
				results <- attemptResult{id: task.ID, worker: worker.Name, maxAttempts: budget, err: err}
			}()
		}

		if once {
			// --once: dispatch no more after this round; drain in-flight.
			for len(running) > 0 {
				time.Sleep(100 * time.Millisecond)
				reap()
			}
			fmt.Println(Board(cfg.Tasks, status))
			return 0
		}
		time.Sleep(poll)
	}
}
