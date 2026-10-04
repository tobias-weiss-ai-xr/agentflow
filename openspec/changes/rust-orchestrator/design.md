# Design: Rust Orchestrator (agentflow)

## Context

The public agentflow repo is a bash fork of the internal taskfleet orchestrator. This change reimplements it as a small Rust crate (`af`) with sound architecture. Full architecture documentation lives in `docs/arc42/` (12 chapters, one file per chapter, committed). Stakeholder: maintainer; consumers: OSS users of the public repo.

## Goals / Non-Goals

**Goals:** single small binary; config drop-in compatibility with taskfleet examples; parallel dispatch, DAG, gates, retry, deadlock, atomic JSON state, receipts; strong test pyramid (unit ≫ integration ≫ E2E) with a fake agent so CI needs no network.

**Non-Goals (v1):** direct LLM API integration; Bayesian routing, episodic memory, trust/constitution/corrections, vLLM worker, openspec bridge, multi-repo (see `docs/multi-repo-design.md`).

## Decisions

1. **Agent via subprocess** — `execute` shells out to an OpenAI-compatible agent CLI (`--provider/--model/-p @file`). No direct HTTP. (ADR-1)
2. **One process; std `thread-per-task` concurrency** — tokio was considered; subprocess orchestration needs no async runtime. Mutual exclusion via per-repo merge mutex. *(amends ADR-2: tokio → std threads; recorded in arc42 ch. 09)*
3. **JSON state, atomic writes** (temp+rename), single writer. (ADR-3)
4. **Config schema compatibility** with taskfleet examples = the integration corpus. (ADR-4)
5. **Spec → contract → test pyramid.** (ADR-5) Specs: this change's `specs/`. Contracts: executable tests per module boundary. Pyramid: unit ≫ integration ≫ E2E (fake agent + scratch repo).
6. **Git operations via `git` CLI** (no libgit2). Worktree add/remove, branch delete, merge serialized per repo.

## Risks / Trade-offs

- [Semantic drift from taskfleet] → config schema compat + campaign configs as integration corpus; ADR log.
- [Subprocess leaks/zombies] → single subprocess helper with hard timeouts + kill-tree; startup self-heal for orphan worktrees.
- [Cost burn on retries] → `max_attempts` hard cap in scheduler; wall-clock receipts.
- [Secret leak to public repo] → `.gitignore` real `workers.json` + `state/`; repo visibility split (taskfleet private).

## Migration Plan

1. Land crate on agentflow `main` (bash shell files remain in git history; clean history — no internal content).
2. Scrub public working tree (internal configs, state, run-*.sh wrappers) in the same change.
3. Set `tobias-weiss-ai-xr/taskfleet` to private on GitHub.
4. Rollback: previous commit restores bash state; taskfleet private is reversible via `gh repo edit --visibility public`.

## Open Questions

None blocking. (ADR-1 alternate direct-API and ADR-6 extras remain open by default assumption; see arc42 ch. 09.)
