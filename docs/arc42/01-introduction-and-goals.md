# 1. Introduction and Goals

## Requirements Overview

**agentflow** (binary: `af`) is a parallel LLM task execution orchestrator on
isolated git worktrees. It dispatches declarative tasks to multiple LLM
providers (via an OpenAI-compatible agent CLI) in parallel; each task runs in
its own git worktree, is verified against an exact acceptance gate, and is
merged only when the gate passes.

agentflow is the **public, open-source** product. The internal, private
**taskfleet** codebase (bash) remains the personal campaign runner and is
*not* what this architecture documents — but it is the source of proven
semantics (scheduler, gates, worktrees) that agentflow re-implements cleanly.

| Id | Goal |
|----|------|
| G1 | Parallel dispatch: up to N workers, each on an isolated git worktree |
| G2 | Exact acceptance gates: per-task shell command, exit 0 = pass, gate before merge |
| G3 | Dependency DAG: `deps` honored, critical-path priority, deadlock detection |
| G4 | Retry: configurable attempts, fresh branch per attempt |
| G5 | Scope checking: advisory check that a task only touches declared files |
| G6 | Observability: status board, per-task logs, cost receipts |
| G7 | Config compatibility: `tasks.json` / `workers.json` schemas work with existing taskfleet configs |
| G8 | Sound architecture: spec → contract → test pyramid discipline (see 04, 09, 10) |

## Quality Goals

- Reliability under crash: task progress survives a killed orchestrator; re-run resumes.
- Concurrency safety: no lost updates, no racing merges, one deterministic writer.
- Testability: every module testable in isolation; the whole pipeline runnable against a *fake* agent CLI and a scratch git repo.
- Maintainability: strict module boundaries, documented via this arc42 documentation.

## Stakeholders

| Role | Expectations |
|------|--------------|
| Maintainer (Tobias Weiss) | Sound architecture, tests, low maintenance cost |
| Open-source users | Clear docs, binary + crate, MIT license, no internal configs leaking |
