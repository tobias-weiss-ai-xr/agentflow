# Current System Analysis — taskfleet

## Overview

taskfleet is a standalone bash-based agentic orchestrator that dispatches declarative tasks to multiple LLM providers in parallel, each running in an isolated git worktree. Tasks are verified against exact acceptance gates before being merged to the main branch.

## Architecture

```
orchestrator.sh          Main loop: poll → reap → dispatch → sleep
├── lib/common.sh        Constants, logging, JSON helpers (jq-based)
├── lib/status.sh        Task status machine (ready→running→verifying→done/failed)
├── lib/worktree.sh      Git worktree create/merge/remove/branch management
├── lib/dispatch.sh      Prompt rendering, agent CLI invocation, lifecycle
├── lib/verify.sh        Acceptance gate runner + error classification
├── lib/vllm-worker.py   Direct vLLM worker for local model inference
├── config/tasks.json    Declarative task definitions (id, title, deps, scope, accept)
├── config/workers.json  Provider/model configuration (name, provider, model, endpoint)
├── prompts/worker.md    Agent prompt template ({{PLACEHOLDER}} syntax)
├── prompts/content-worker.md   Content generation prompt template
├── prompts/content-worker-vllm.md  Content generation for vLLM models
├── courses/             Course content generation workflow (27 Microsoft exams, 54 tasks)
└── tests/               22 test suites, 159 tests (10 testing paradigms)
```

## Core Flow

1. Parse `tasks.json` and `workers.json`
2. Main loop: reap finished tasks → find ready tasks → pair with free workers → dispatch
3. Per task: create worktree → render prompt → run LLM agent → verify acceptance gate → merge or retry
4. Retry logic: fresh branch (or preserved branch for merge conflicts), error feedback injected into prompt
5. Deadlock detection: exits when all remaining tasks are blocked by failures

## Strengths

### 1. Git Worktree Isolation (Unique)
Each task gets its own branch + working tree. No cross-contamination between parallel tasks.

### 2. Exact Acceptance Gates
Each task declares a shell command that must exit 0 before merge. Deterministic verification — not LLM-based review.

### 3. Provider Agnosticism
Works with any OpenAI-compatible API. Supports `pi`, vLLM direct, and any CLI with `--provider --model -p`.

### 4. Error Classification + Retry Intelligence
9 structured categories (compile_error, test_failure, missing_package, linker_error, timeout, no_op, rate_limit, auth_error, network_error). Injects classified errors into retry prompts.

### 5. Merge Conflict Recovery
Preserves worktrees for conflict resolution on retry instead of redoing all work.

### 6. Robustness Stack (6 Levels)
- L1: Stale process killing at startup
- L2: Stale worktree/branch recovery
- L3: Task/gate validation
- L4: Provider health checks before dispatch
- L5: Deadlock detection with infra-failure auto-retry
- L6: Graceful retry for transient failures

### 7. SOTA Testing (159 tests, 10 paradigms)
Mutation (100% kill rate), chaos/fault injection, property-based with shrinking, fuzzing, table-driven, contract, idempotency, golden files, stress.

### 8. Dependency DAG
Tasks declare `deps` — prerequisite tasks must complete first. Topological scheduling.

### 9. Scope Checking
Advisory check detects out-of-scope file edits per task.

### 10. Zero Dependencies Beyond jq
Pure bash + jq. Extremely portable.

## Weaknesses

### 1. Bash Limitations
No native structured data, crude process management, no streaming, fragile locking.

### 2. No Task Prioritization
Tasks dispatched in JSON order. No priority queue or strategic model assignment.

### 3. No Cost Tracking
No token usage or cost visibility per task/worker/run.

### 4. No Human-in-the-Loop
No approval gates, pause/resume, or interactive conflict resolution.

### 5. No Cancellation
No way to cancel a running task or the entire run.

### 6. Single Machine
No distributed execution or horizontal scaling.

### 7. No Observability
No metrics, tracing, or dashboards.

### 8. No Task Templates
Tasks are fully manual JSON. No template/generator system.

### 9. No Incremental Resume
Crashes lose all in-flight work.

### 10. Basic Prompt Management
Single markdown template with placeholder substitution.
