// Command af (Go port, stdlib only) — parallel LLM task execution on
// isolated git worktrees. Port of the Rust af core subset (openspec
// change go-port): tasks/workers JSON config, per-task git worktree,
// subprocess agent dispatch, acceptance gate, retry with attempts,
// atomic run-state JSON, ff-merge on success, logs.
//
// CLI: af run [--once] | af status | af attach <ID>
package main

import (
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

const usage = `af (Go port) — parallel LLM task execution on isolated git worktrees

USAGE:
  af run       [--once]
  af status
  af attach    ID

ENV: TF_REPO_DIR, TF_STATE_DIR, TF_MAX_PARALLEL, TF_BRANCH_PREFIX, TF_POLL,
     TF_TASKS_JSON, TF_WORKERS_JSON, TF_AGENT_TIMEOUT_S, TF_ACCEPT_TIMEOUT_S`

// Settings: resolved runtime configuration (env overrides, af defaults).
type Settings struct {
	RepoDir        string
	StateDir       string
	WorktreeRoot   string
	TasksFile      string
	WorkersFile    string
	PromptFile     string
	BranchPrefix   string
	PollSecs       int
	MaxParallel    int // 0 = auto (enabled workers)
	AgentTimeoutS  int // 0 = workers.json defaults / 3600
	AcceptTimeoutS int // 0 = workers.json defaults / 600
}

func settingsFromEnv() *Settings {
	stateDir := envOr("TF_STATE_DIR", ".af-state")
	return &Settings{
		RepoDir:        envOr("TF_REPO_DIR", "."),
		StateDir:       stateDir,
		WorktreeRoot:   filepath.Join(stateDir, "worktrees"),
		TasksFile:      envOr("TF_TASKS_JSON", filepath.Join("config", "tasks.json")),
		WorkersFile:    envOr("TF_WORKERS_JSON", filepath.Join("config", "workers.json")),
		PromptFile:     filepath.Join("prompts", "worker.md"),
		BranchPrefix:   envOr("TF_BRANCH_PREFIX", "tf"),
		PollSecs:       envIntOr("TF_POLL", 15),
		MaxParallel:    envIntOr("TF_MAX_PARALLEL", 0),
		AgentTimeoutS:  envIntOr("TF_AGENT_TIMEOUT_S", 0),
		AcceptTimeoutS: envIntOr("TF_ACCEPT_TIMEOUT_S", 0),
	}
}

func envOr(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}

func envIntOr(key string, def int) int {
	if v := os.Getenv(key); v != "" {
		if n, err := strconv.Atoi(v); err == nil {
			return n
		}
	}
	return def
}

// firstPositive returns the first value > 0 (chain: env > config > default).
func firstPositive(vals ...int) int {
	for _, v := range vals {
		if v > 0 {
			return v
		}
	}
	return 0
}

func main() { os.Exit(run(os.Args[1:])) }

func run(argv []string) int {
	if len(argv) == 0 || argv[0] == "-h" || argv[0] == "--help" {
		fmt.Println(usage)
		return 0
	}
	cmd, rest := argv[0], argv[1:]
	switch cmd {
	case "run":
		fs := flag.NewFlagSet("run", flag.ContinueOnError)
		once := fs.Bool("once", false, "dispatch a single round, drain it, then exit")
		if err := fs.Parse(rest); err != nil {
			return 2
		}
		st, cfg, err := load()
		if err != nil {
			return fatal(err)
		}
		return RunLoop(cfg, st, *once)
	case "status":
		st, cfg, err := load()
		if err != nil {
			return fatal(err)
		}
		fmt.Println(Board(cfg.Tasks, (Store{Dir: st.StateDir}).Load()))
		return 0
	case "attach":
		if len(rest) == 0 {
			return fatal(fmt.Errorf("af attach requires a task id (af attach <ID>)"))
		}
		return attach(settingsFromEnv(), rest[0])
	default:
		fmt.Fprintf(os.Stderr, "error: unknown command '%s'\n\n%s\n", cmd, usage)
		return 2
	}
}

func load() (*Settings, *Config, error) {
	st := settingsFromEnv()
	cfg, err := LoadConfig(st.TasksFile, st.WorkersFile)
	if err != nil {
		return nil, nil, err
	}
	return st, cfg, nil
}

func fatal(err error) int {
	fmt.Fprintf(os.Stderr, "error: %v\n", err)
	return 2
}

// attach prints the last 20 lines of the task's log.
func attach(st *Settings, id string) int {
	data, err := os.ReadFile(filepath.Join(st.StateDir, "logs", id+".log"))
	if err != nil {
		fmt.Printf("no log for task %s\n", id)
		return 0
	}
	lines := strings.Split(strings.TrimRight(string(data), "\n"), "\n")
	if len(lines) > 20 {
		lines = lines[len(lines)-20:]
	}
	fmt.Println(strings.Join(lines, "\n"))
	return 0
}
