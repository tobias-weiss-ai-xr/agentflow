# Proposal: Rust Orchestrator (agentflow)

## Why

The public agentflow project is a bash-fork of the internal taskfleet orchestrator — proven semantics, but a monolithic, untyped shell that cannot be confidently published as open source (no real CLI, global state, string-parsed JSON, cross-process flocking). We need a clean Rust implementation with sound architecture that becomes the public product, while taskfleet remains the private internal campaign runner.

## What Changes

- **New Rust crate** `agentflow` (single binary `af`) implementing declarative parallel LLM task execution on isolated git worktrees.
- Executes tasks by **shelling out to an OpenAI-compatible agent CLI** (ADR-1) and to the **git CLI** — the orchestrator stays thin.
- Parallel dispatch, dependency DAG, contention avoidance, retry on fresh branches, scope checking, exact acceptance gates, deadlock detection, atomic JSON state, cost receipts.
- **Config compatibility**: `tasks.json` / `workers.json` schemas work with existing taskfleet examples (drop-in migration).
- arc42 documentation (12 chapters, one file per chapter) in `docs/arc42/`.
- **BREAKING** for the public repo: the bash implementation is replaced by the Rust binary; internal campaign configs and state are removed from the public repo.

## Capabilities

**New Capabilities** (each becomes `specs/<capability>/spec.md`):
- `config` — task/worker schema loading + validation + env overrides
- `scheduling` — dependency DAG, priority, contention, retry, deadlock detection
- `lifecycle` — task execution: worktree → agent → gate → merge
- `state` — status persistence, atomic writes, resume/self-heal, receipts
- `worktree` — git worktree management + merge serialization
- `cli` — `af` command surface (run / status / api / attach / cost)

**Modified Capabilities**: none (new project).

## Impact

- **Repos**: public `tobias-weiss-ai-xr/agentflow` becomes the Rust product; private `tobias-weiss-ai-xr/taskfleet` keeps the bash campaign runner (ADR-8).
- **Code**: replaces `orchestrator.sh` + `lib/*.sh` with Rust modules; `config/tasks.json` / `workers.json` schemas preserved.
- **Dependencies**: Rust, `serde`/`serde_json`; git + agent CLI remain external.
- **Docs**: `docs/arc42/` is the architecture documentation; this change's design.md summarizes.
- **Hygiene**: public repo must contain no secrets or internal configs; `.gitignore` real `workers.json` and `state/`.
