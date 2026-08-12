# taskfleet

**Parallel LLM task execution on isolated git worktrees.**

Taskfleet is a standalone agentic orchestrator that dispatches declarative tasks
to multiple LLM providers in parallel, each running in an isolated git worktree.
Tasks are verified against exact acceptance gates before being merged to the
main branch.

```
 tasks.json (N tasks, declarative)  ─┐
 workers.json (M providers)          ├──► orchestrator.sh ──► per-task:
 TF_GATE_ENV (project env vars)      ─┘      worktree → agent --provider X --model Y
                                               → acceptance gate → merge → status
```

## Features

- **Parallel dispatch** — up to N workers simultaneously, each in an isolated git worktree
- **Provider-agnostic** — works with any OpenAI-compatible API (pi, opencode, anthropic, openrouter, etc.)
- **Git worktree isolation** — each task gets its own branch + working tree; no cross-contamination
- **Exact acceptance gates** — each task declares a shell command that must exit 0 before merge
- **Automatic retry** — configurable max attempts per task, fresh branch on each retry
- **Merge serialization** — fine-grained flock ensures concurrent merges don't race
- **Self-healing** — main worktree is cleaned at startup and after each failed merge
- **Scope checking** — advisory check detects out-of-scope file edits
- **Deadlock detection** — exits cleanly when all remaining tasks are blocked by failures
- **29/29 self-tests** — comprehensive test suite validates the full lifecycle

## Quick start

```sh
git clone https://github.com/tobias-weiss-ai-xr/taskfleet.git
cd taskfleet

# 1. Copy and configure worker definitions
cp config/workers.json.example config/workers.json
# Edit workers.json: set provider, model, api_base for each worker

# 2. Define your tasks
cp config/tasks.json.example config/tasks.json
# Edit tasks.json: each task has id, title, deps, scope, and accept (gate command)

# 3. (Optional) Set project-specific environment for gates
export TF_GATE_ENV="RUSTUP_TOOLCHAIN=nightly PATH=\$HOME/.cargo/bin:\$PATH"

# 4. Point to your git repo (default: 2 levels up)
export TF_REPO_DIR=/path/to/your/repo

# 5. Self-test
bash tests/run-all-tests.sh

# 6. Dry run (see what would be dispatched)
./orchestrator.sh --dry-run

# 7. Run
./orchestrator.sh

# 8. Check status
./orchestrator.sh --status
```

## CLI

```sh
./orchestrator.sh              # run until all tasks done or deadlock
./orchestrator.sh --once       # dispatch one round, then exit
./orchestrator.sh --dry-run    # show dispatch plan, change nothing
./orchestrator.sh --status     # print status board and exit
./orchestrator.sh --worker X   # restrict to a single worker
./orchestrator.sh --task ID    # dispatch exactly one task
./orchestrator.sh --poll SECS  # polling interval (default 15)
```

## Task schema (`config/tasks.json`)

```json
{
  "_meta": { "project": "my-project" },
  "tasks": [
    {
      "id": "FC-1",
      "title": "Create path.rs with Path/Range types",
      "deps": [],
      "scope": ["core/crates/wo-common/src/path.rs"],
      "accept": "cargo test -p wo-common path::",
      "acceptance_prose": "Path and Range types compile and round-trip via serde",
      "manual": false
    }
  ]
}
```

| Field | Description |
|-------|-------------|
| `id` | Unique task identifier (used for branch names, status keys) |
| `title` | Human-readable description (injected into agent prompt) |
| `deps` | List of task IDs that must complete before this task runs |
| `scope` | File globs the task is allowed to modify (advisory check) |
| `accept` | Shell command run as acceptance gate (exit 0 = pass) |
| `acceptance_prose` | Natural-language description of success criteria |
| `manual` | If true, skip acceptance gate (manual sign-off required) |

## Worker schema (`config/workers.json`)

```json
{
  "defaults": {
    "accept_timeout_s": 600,
    "max_attempts": 3,
    "retry_delay_s": 30
  },
  "workers": [
    {
      "name": "openai",
      "enabled": true,
      "provider": "openai",
      "model": "gpt-4o",
      "api_base": "https://api.openai.com/v1"
    },
    {
      "name": "anthropic",
      "enabled": true,
      "provider": "anthropic",
      "model": "claude-sonnet-4-20250514",
      "api_base": "https://api.anthropic.com/v1"
    }
  ]
}
```

Workers are configured with `name`, `provider` (matches `--provider` flag of your agent CLI),
`model` (matches `--model` flag), and `api_base`. Each worker runs at most one task at a time.

## Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `TF_DIR` | (auto-detect) | Taskfleet installation directory |
| `TF_REPO_DIR` | `../../` (from TF_DIR) | Git repo being modified |
| `TF_MAX_PARALLEL` | (# enabled workers) | Max concurrent tasks |
| `TF_BRANCH_PREFIX` | `tf` | Branch prefix for agent work |
| `TF_GATE_ENV` | (none) | Extra env exports for acceptance gates |
| `TF_POLL` | `15` | Seconds between dispatch rounds |

## Agent CLI compatibility

Taskfleet dispatches tasks via an agent CLI (default: `pi`). The CLI must support:

```
<cli> --provider <name> --model <model> -p @<prompt-file>
```

Override the CLI command by editing `lib/dispatch.sh` — the dispatch function
calls `pi --provider "$provider" --model "$model" -p "@$prompt"`.

## Architecture

```
orchestrator.sh          Main loop: poll → reap → dispatch → sleep
├── lib/common.sh        Constants, logging, JSON helpers
├── lib/status.sh        Task status machine (ready→running→done/failed)
├── lib/worktree.sh      Git worktree create/merge/remove/branch management
├── lib/dispatch.sh      Prompt rendering, agent CLI invocation, lifecycle
├── lib/verify.sh        Acceptance gate runner + scope drift checker
├── config/tasks.json    Declarative task definitions
├── config/workers.json  Provider/model configuration
├── prompts/worker.md     Agent prompt template ({{PLACEHOLDER}} syntax)
└── tests/               29 tests covering status, worktree, verify, dispatch, integration
```

## License

MIT
