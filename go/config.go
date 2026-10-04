package main

import (
	"encoding/json"
	"fmt"
	"os"
)

// Task mirrors a tasks.json entry: {id,title,deps,scope,accept,acceptance_prose}.
type Task struct {
	ID              string   `json:"id"`
	Title           string   `json:"title"`
	Deps            []string `json:"deps"`
	Scope           []string `json:"scope"`
	Accept          string   `json:"accept"`
	AcceptanceProse string   `json:"acceptance_prose"`
}

// Worker mirrors a workers.json entry.
type Worker struct {
	Name        string `json:"name"`
	Provider    string `json:"provider"`
	Model       string `json:"model"`
	CLI         string `json:"cli"`
	APIKeyEnv   string `json:"api_key_env"`
	MaxAttempts int    `json:"max_attempts"` // 0 = defaults.max_attempts
	Enabled     *bool  `json:"enabled"`      // nil = enabled
}

func (w *Worker) IsEnabled() bool { return w.Enabled == nil || *w.Enabled }

type Defaults struct {
	MaxAttempts    int `json:"max_attempts"`
	AcceptTimeoutS int `json:"accept_timeout_s"`
	AgentTimeoutS  int `json:"agent_timeout_s"`
}

type workersFile struct {
	Workers     []Worker `json:"workers"`
	Defaults    Defaults `json:"defaults"`
	MaxParallel int      `json:"max_parallel"`
}

type Config struct {
	Tasks       []Task
	Workers     []Worker
	Defaults    Defaults
	MaxParallel int // 0 = auto
	Warnings    []string
}

// AttemptBudget: per-worker max_attempts with a config-level fallback.
func (c *Config) AttemptBudget(w *Worker) int {
	return firstPositive(w.MaxAttempts, c.Defaults.MaxAttempts, 3)
}

// LoadConfig reads tasks.json (an array, or {"tasks":[...]} for campaign
// files that carry both halves) and workers.json
// ({"workers":[...],"defaults":{...}}). TF_TASKS_JSON/TF_WORKERS_JSON env
// overrides are resolved by settingsFromEnv before this is called.
func LoadConfig(tasksPath, workersPath string) (*Config, error) {
	tasks, err := loadTasks(tasksPath)
	if err != nil {
		return nil, err
	}
	data, err := os.ReadFile(workersPath)
	if err != nil {
		return nil, fmt.Errorf("read %s: %w", workersPath, err)
	}
	var wf workersFile
	if err := json.Unmarshal(data, &wf); err != nil {
		return nil, fmt.Errorf("parse %s: %w", workersPath, err)
	}
	cfg := &Config{Tasks: tasks, Workers: wf.Workers, Defaults: wf.Defaults, MaxParallel: wf.MaxParallel}
	if err := cfg.validate(); err != nil {
		return nil, err
	}
	return cfg, nil
}

func loadTasks(tasksPath string) ([]Task, error) {
	data, err := os.ReadFile(tasksPath)
	if err != nil {
		return nil, fmt.Errorf("read %s: %w", tasksPath, err)
	}
	var arr []Task
	if err := json.Unmarshal(data, &arr); err == nil {
		return arr, nil
	}
	var obj struct {
		Tasks []Task `json:"tasks"`
	}
	if err := json.Unmarshal(data, &obj); err != nil {
		return nil, fmt.Errorf("parse %s: not a task array: %w", tasksPath, err)
	}
	return obj.Tasks, nil
}

func (c *Config) validate() error {
	seen := map[string]bool{}
	for _, t := range c.Tasks {
		if t.ID == "" {
			return fmt.Errorf("task with empty id")
		}
		if seen[t.ID] {
			return fmt.Errorf("duplicate task id: %s", t.ID)
		}
		seen[t.ID] = true
	}
	wnames := map[string]bool{}
	enabled := 0
	for i := range c.Workers {
		w := &c.Workers[i]
		if w.Name == "" {
			return fmt.Errorf("worker with empty name")
		}
		if w.Provider == "" || w.Model == "" {
			return fmt.Errorf("worker '%s': provider and model are required", w.Name)
		}
		if wnames[w.Name] {
			return fmt.Errorf("duplicate worker name: %s", w.Name)
		}
		wnames[w.Name] = true
		if w.IsEnabled() {
			enabled++
		}
	}
	if enabled == 0 {
		return fmt.Errorf("no enabled workers")
	}
	// Dangling deps warn (compat: configs referencing tasks merged in from
	// sibling files); deadlock detection turns a never-resolving dep into a
	// clean exit 2 instead of a hang.
	ids := map[string]bool{}
	for _, t := range c.Tasks {
		ids[t.ID] = true
	}
	for _, t := range c.Tasks {
		for _, d := range t.Deps {
			if !ids[d] {
				c.Warnings = append(c.Warnings,
					fmt.Sprintf("task '%s': dep '%s' not in this file (assumed merged elsewhere)", t.ID, d))
			}
		}
	}
	return nil
}
