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

### Functional goals

| Id | Goal | Status |
|----|------|--------|
| G1 | Parallel dispatch: up to N workers, each on an isolated git worktree | ✅ shipped |
| G2 | Exact acceptance gates: per-task shell command, exit 0 = pass, gate before merge | ✅ shipped |
| G3 | Dependency DAG: `deps` honored, critical-path priority, deadlock detection | ✅ shipped |
| G4 | Retry: configurable attempts, fresh branch per attempt, optional backoff (`retry_delay_s`) | ✅ shipped |
| G5 | Scope checking: advisory + enforced — `touch` validation, scope-violation detection, contention avoidance | ✅ shipped |
| G6 | Observability: status board, per-task logs, cost receipts, `af cost` with waste/recovery ledger | ✅ shipped |
| G7 | Config compatibility: `tasks.json` / `workers.json` schemas work with existing taskfleet configs | ✅ shipped |
| G8 | Sound architecture: spec → contract → test pyramid discipline (340 tests, 96.83% line coverage) | ✅ shipped |
| G9 | Multi-repo: one campaign can touch several repositories (`repos.json`, per-task `repo`) | ✅ shipped (ADR-11) |
| G10 | Measured routing: UCB1 worker selection from receipt history, cost-aware tie-breaks | ✅ shipped (ADR-12) |
| G11 | Retry memory: failed attempts record their error; retry prompts list earlier failures | ✅ shipped (ADR-13) |
| G12 | Sandbox: env allowlist + git hygiene + opt-in wrapper seam for the untrusted agent child | ✅ shipped (ADR-10) |
| G13 | Token + cost capture: JSON-mode transcript parsing, provider-reported cost, declared-basis estimates | ✅ shipped |
| G14 | Budget cap: `TF_MAX_WALL_CLOCK_S` stops new dispatches when the spend ceiling is reached | ✅ shipped |
| G15 | Stall watchdog: `TF_AGENT_STALL_S` kills a silent agent before the total timeout burns tokens | ✅ shipped |
| G16 | Work preservation: every failure path archives the branch as `<branch>-rejected-[<attempt>-]<ts>` | ✅ shipped |
| G17 | Recovery: `af recover --task ID [--attempt N]` and auto-reuse re-validate archived work without re-invoking the agent | ✅ shipped |
| G18 | Honest success: `Merged` means the base really contains the work (dirty worktree committed, zero-commits-ahead = failure, merge verified) | ✅ shipped |
| G19 | Interrupted-attempt tracking: a killed orchestrator records `interrupted` (not a verdict, excluded from trust) | ✅ shipped |

## Quality Goals

- **Reliability under crash**: task progress survives a killed orchestrator; re-run resumes. Orphan worktrees are self-healed at startup. A killed attempt is recorded as `interrupted` with the worker and an upper-bound duration.
- **Concurrency safety**: no lost updates, no racing merges, one deterministic writer. State JSON is atomic (temp + fsync + rename); a torn write is reported, never silently empty.
- **Honest success**: `Merged` is never inferred from an exit code. A dirty worktree is committed before judging; zero commits ahead is a failure; the branch tip is verified in the base.
- **Work preservation**: a failed attempt's committed work is never destroyed — on ANY failure path (stall, timeout, non-zero exit, scope violation, merge conflict, interrupted) the branch is archived and recoverable.
- **Cost transparency**: every attempt leaves a receipt (worker, model, wall-clock, tokens, cost); `af cost` reports measured cost, declared-basis estimates, waste by reason, and reclaimed (recovered) work.
- **Testability**: every module testable in isolation; the whole pipeline runnable against a *fake* agent CLI and a scratch git repo (340 tests, no network in CI).
- **Maintainability**: strict module boundaries, documented via this arc42 documentation; a coverage ratchet (94% floor) prevents silent rot.

## Stakeholders

| Role | Expectations |
|------|--------------|
| Maintainer (Tobias Weiss) | Sound architecture, tests, low maintenance cost |
| Open-source users | Clear docs, binary + crate, MIT license, no internal configs leaking |
| Agent users (dogfooding) | agentflow dispatches tasks that improve agentflow itself; 14 rounds of dogfooding have shaped every feature |
