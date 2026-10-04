# 8. Cross-cutting Concepts

## 8.1 Task state machine

| State | Meaning | Guarded transitions |
|-------|---------|---------------------|
| `ready` | DAG-eligible, queued | → `running` on free worker + no contention |
| `running` | attempt in flight | → `done` (gate+merge ok) \| `failed` (gate/agent fail, attempts exhausted) \| `ready` (retry with fresh branch) |
| `done` | merged to base branch | terminal |
| `failed` | attempts exhausted, or dep failed | terminal; also trigger of deadlock exit |

- Persisted **before** every visible transition; idempotent re-run resumes from
  deepest `done`.

## 8.2 Concurrency & serialization

- One process, one writer; atomic JSON (temp + rename) for all `state/` files.
- Merges are serialized per repository (single merge mutex in-process).
- Contention: tasks whose `scope` globs overlap are not dispatched
  concurrently (default `defer`).

## 8.3 Subprocess lifecycle (shared helper)

- spawn → optional streaming tail to log → wait with timeout → on timeout or
  kill: kill entire process tree → classify by exit code (0 / non-zero /
  killed-by-us / missing binary). This is the *one* path that `git`, agent, and
  gate all use, so exit-contract behavior is uniform and contract-tested once.

## 8.4 Acceptance gate

- `accept` is a shell string run in the worktree, subprocess helper semantics,
  bounded by `accept_timeout_s`, env scoped via `TF_GATE_ENV`. exit 0 = pass;
  output captured for `af api results`.

## 8.5 Scope checking (advisory)

- After an attempt, diff against base; files outside declared `scope` globs are
  reported (advisory — does not fail the gate by default). Used by contention
  scheduling regardless.

## 8.6 Receipts & cost

- Every attempt appends a receipt (task, attempt, worker, model, wall-clock,
  tokens if reported by the agent CLI) → `state/receipts/`; `af cost` aggregates.
  Wall-clock, not token-count, is the source of truth for "elapsed".

## 8.7 Logging & transparency

- Per-task logs → `state/logs/<task>.log`; `af attach <task>` tails live.
- Status board is human (`--status`) and machine (`api status --json`) readable.

## 8.8 Error contracts

- All module public fns return typed errors (`Result`) with a stable message;
  no silent partial writes; a crash mid-write never corrupts state (8.2/6.2).
