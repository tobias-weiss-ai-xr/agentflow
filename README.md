# agentflow

**Parallel LLM task execution on isolated git worktrees.**

agentflow is a small Rust orchestrator that dispatches declarative tasks to
multiple LLM providers in parallel (via an OpenAI-compatible **agent CLI**),
each running in an isolated git worktree. Tasks are verified against exact
acceptance gates before being merged to the base branch.

```
 tasks.json (N tasks, declarative)  ─┐
 workers.json (M providers)          ├──► af ──► per-task:
                                      │         worktree → agent CLI → gate → merge
 env (TF_*)                          ─┘
```

The orchestrator stays thin (ADR-1): it drives `git` and an OpenAI-compatible
agent CLI (`pi`, opencode, …) as subprocesses. It never talks to LLM
providers directly.

## Features

- **Parallel dispatch** — one worker per provider+model slot; concurrent tasks in isolated git worktrees
- **Exact acceptance gates** — each task declares a shell command that must exit 0 before merge
- **Dependency DAG** — `deps` ordering, critical-path priority, deadlock detection
- **Contention avoidance** — tasks with overlapping `scope` globs are not dispatched concurrently
- **Retry** — `max_attempts` per task, fresh branch + worktree on every attempt
- **Self-healing** — atomic JSON state; crash-safe resume; orphan worktree cleanup at startup
- **Observable** — status board (`--json`), live `attach`, per-task logs, wall-clock cost receipts
- **Sound by construction** — spec → contract → test pyramid (30 tests, incl. E2E against a stub agent + scratch git repo; no network in CI)

## Quick start

```sh
cargo build --release            # produces target/release/af

cp config/tasks.json.example config/tasks.json
cp config/workers.json.example config/workers.json
# edit workers.json: provider/model/api_base + your agent CLI (default: pi)

export TF_REPO_DIR=/path/to/repo/being/modified
export TF_STATE_DIR=state

./target/release/af run --dry-run   # show the dispatch plan, change nothing
./target/release/af run             # run until all tasks done or deadlock
./target/release/af status          # task status board
```

## CLI

```
af run    [--once] [--dry-run] [--worker NAME] [--task ID] [--poll SECS]
af status
af api    status [--json] | results --task ID
af attach ID
af cost   [--task ID]
```

## Task schema (`config/tasks.json`)

| Field | Description |
|-------|-------------|
| `id` | Unique task id (branch names, status keys) |
| `title` | Human-readable description (injected into the prompt) |
| `deps` | Task ids that must reach `done` first |
| `scope` | File globs the task may modify (contention + advisory) |
| `accept` | Shell command run in the task's worktree; exit 0 = pass |
| `acceptance_prose` | Natural-language success criteria (injected into the prompt) |
| `manual` | Skip the acceptance gate (manual sign-off) |
| `priority` | Tie-breaker when multiple tasks are ready |

## Worker schema (`config/workers.json`)

`defaults` (`accept_timeout_s`, `max_attempts`, `retry_delay_s`, `agent_timeout_s`)
plus a `workers` list: `name`, `provider`, `model`, `api_base`, `enabled`,
`cli` (agent CLI binary; default `pi`). Each worker runs at most one task at a time.

## Agent CLI compatibility

`af` invokes your agent CLI as:

```
<cli> --provider <name> --model <model> -p @<prompt-file>
```

Any OpenAI-compatible CLI supporting that shape works. `src/bin/example_agent.rs`
is a stub agent (writes a file + commits) used by the test suite and CI.

## Testing

```sh
cargo test        # 30 tests: unit (scheduler DAG, contention, deadlock, state,
                  # receipts, config validation, subprocess contracts) + E2E
                  # (fake agent + scratch git repo — no network)
```

See `docs/arc42/` for the architecture documentation (12 chapters), and
`openspec/changes/` for the spec-driven change history (spec → contract → test).

## License

MIT
