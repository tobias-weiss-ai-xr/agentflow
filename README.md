# agentflow

> **⚠️ DEPRECATED (2026-10-09):** this branch is the retired **Go port**.
> The Rust af on \`main\` is feature-complete (including the \`command\`
> worker template) and ships fleet-wide. See DEPRECATED.md.


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
- **Self-healing** — atomic JSON state; crash-safe resume; orphan worktree cleanup at startup
- **Observable** — status board (`--json`), live `attach`, per-task logs, wall-clock cost receipts
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
