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
- **Multi-repo** — one campaign can touch several repositories: `repos.json` maps names to paths, each task's worktree/branch/merge targets its own repo, deps order across repos
- **Measured routing** — every attempt leaves a receipt with its outcome; free workers are picked by UCB1 (track record + exploration), per-worker trust shown in `af cost`
- **Retry memory** — failed attempts record their error; retry prompts list the task's earlier failures so the agent doesn't repeat them
- **Exact acceptance gates** — each task declares a shell command that must exit 0 before merge
- **Dependency DAG** — `deps` ordering, critical-path priority, deadlock detection
- **Contention avoidance** — tasks with overlapping `scope` globs are not dispatched concurrently
- **Retry** — `max_attempts` per task, fresh branch + worktree on every attempt
- **Self-healing** — atomic JSON state; crash-safe resume; orphan worktree cleanup at startup, plus `af clean [--dry-run]` to sweep leftovers from crashed runs
- **Validated config** — `af validate` pre-flights tasks/workers (dependency cycles, duplicate ids, no enabled workers) without dispatching anything
- **Observable** — status board (`af status [--json]`), live `attach`, per-task logs, wall-clock cost receipts
- **Sound by construction** — spec → contract → test pyramid (71 tests, incl. E2E against a stub agent + scratch git repos; no network in CI)

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
af run [--once] [--dry-run] [--worker NAME] [--task ID] [--poll SECS]
af status [--json]
af api status [--json] | results --task ID
af attach ID
af cost [--task ID]
af clean [--dry-run]
af validate [--worker NAME]
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
| `gate_replay` | Whether the gate is replay-safe (default `true`); `false` = side effects |

Replay contract: the `accept` command MUST be idempotent; declare
`gate_replay: false` for a gate with side effects — an interrupted such gate
is settled as failed instead of re-run. The gate process receives the
decision in `TF_GATE_REPLAY` (`1`/`0`).

## Worker schema (`config/workers.json`)

`defaults` (`accept_timeout_s`, `max_attempts`, `retry_delay_s`, `agent_timeout_s`)
plus a `workers` list: `name`, `provider`, `model`, `api_base`, `api_key_env`,
`enabled`, `cli` (agent CLI binary; default `pi`). Each worker runs at most one
task at a time.

Tasks accept an optional `repo` field: `""` (default) or `"main"` target the
main repo (`TF_REPO_DIR`); other names must appear in `repos.json` (next to
tasks.json or via `--repos`/`TF_REPOS_JSON`) — e.g.
`{"repos": {"docs": "../docs-site"}}`. Unknown names warn and fall back.

## Sandboxing

The agent CLI executes LLM-directed tool calls, so `af` treats it as untrusted
(ADR-10):

- **Env allowlist** — the agent child sees only system basics, the dispatched
  worker's `api_key_env`, and `TF_AGENT_ENV_PASSTHROUGH` (comma-separated
  extras). Other workers' API keys and your shell secrets are withheld.
- **Git hygiene** — `GIT_TERMINAL_PROMPT=0`, credential helpers disabled:
  no credential popups, no hangs.
- **Wrapper seam** — `TF_SANDBOX_CMD="firejail --net=none"` (or bwrap /
  sandbox-exec / a container) is prepended to the agent argv for real
  filesystem/network containment. Env hygiene alone is not a sandbox —
  see `docs/arc42/08-concepts.md` §8.8 for threat model and recipes.

## Agent CLI compatibility

`af` invokes your agent CLI as:

```
<cli> --provider <name> --model <model> -p @<prompt-file>
```

Any OpenAI-compatible CLI supporting that shape works. `src/bin/example_agent.rs`
is a stub agent (writes a file + commits) used by the test suite and CI.

## Dogfooding / parallel workers

agentflow is built to be run *by* agents, and the fastest way to exercise it
is to dispatch several tasks at once — often across more than one repository —
to two or more workers. Here is a full two-repo / two-worker campaign.

### `config/repos.json`

```json
{
  "repos": {
    "main": "/abs/path/to/agentflow",
    "docs":  "/abs/path/to/docs-site"
  }
}
```

### `config/tasks.json`

```json
{
  "_meta": { "project": "dogfood" },
  "tasks": [
    {
      "id": "fix-readme",
      "title": "Document the --repos flag in the README",
      "repo": "main",
      "deps": [],
      "scope": ["README.md"],
      "accept": "grep -q -- '--repos' README.md",
      "acceptance_prose": "README documents the --repos flag.",
      "manual": false
    },
    {
      "id": "fix-docs-index",
      "title": "Refresh the docs site index page",
      "repo": "docs",
      "deps": [],
      "scope": ["index.md"],
      "accept": "git diff --stat --exit-code",
      "acceptance_prose": "Docs index is updated.",
      "manual": false
    }
  ]
}
```

### `config/workers.json`

```json
{
  "defaults": {
    "accept_timeout_s": 600,
    "max_attempts": 3,
    "retry_delay_s": 30,
    "agent_timeout_s": 3600
  },
  "workers": [
    { "name": "opus",  "provider": "anthropic", "model": "claude-opus-4", "api_base": "https://api.anthropic.com/v1", "api_key_env": "ANTHROPIC_API_KEY", "enabled": true, "cli": "pi" },
    { "name": "gpt4o", "provider": "openai",    "model": "gpt-4o",        "api_base": "https://api.openai.com/v1",      "api_key_env": "OPENAI_API_KEY",    "enabled": true, "cli": "pi" }
  ]
}
```

### Invocation

```sh
export TF_REPO_DIR=/abs/path/to/agentflow
export TF_STATE_DIR=state

./target/release/af run --dry-run \
    --tasks config/tasks.json --workers config/workers.json --repos config/repos.json
./target/release/af run \
    --tasks config/tasks.json --workers config/workers.json --repos config/repos.json
./target/release/af status
./target/release/af cost
```

`fix-readme` targets repo `main` and `fix-docs-index` targets repo `docs`; they
have no `deps` between them, so the scheduler treats both as ready at the same
time. With two idle workers the two tasks dispatch **in parallel** — one in an
isolated git worktree on `main`, the other on `docs` — each later merging into
its own repo. Swap in `--task`/`--worker` to pin a single task or worker.

### How trust routing picks workers

Every attempt leaves a **receipt** (worker, model, wall-clock time, outcome).
On dispatch, the router replays receipts into per-worker `(wins, attempts)`
stats and, among the currently free workers, picks by **UCB1**:

```
score(worker) =  mean(worker)  +  sqrt( 2 * ln(N + 1) / (n + 1) )
                └─ exploitation ┘   └─ exploration bonus ┘
```

- `mean` = `wins ÷ attempts` — the worker's measured trust rate.
- `N` = total attempts across all workers; `n` = this worker's attempts.

With no receipts yet, every worker scores `0` (a tie), broken in **config
order** — the first configured worker (`opus`) takes the first ripe task and
`gpt4o` takes the second, so both run concurrently. As receipts accumulate, a
worker that keeps failing lowers its `mean`, while the exploration term gives
an under-tried (or untried) worker the chance to be routed past it. `af cost`
shows each worker's live trust rate so you can watch routing adapt between
campaigns.

## Testing

```sh
cargo test        # 71 tests: unit (scheduler DAG, contention, deadlock, state,
                  # receipts, config validation, sandbox policy, multi-repo,
                  # UCB1 router, retry context, subprocess contracts) + E2E
                  # (fake agent + scratch git repos — no network)
```

See `docs/arc42/` for the architecture documentation (12 chapters), and
`openspec/changes/` for the spec-driven change history (spec → contract → test).

## License

MIT
