# 9. Architecture Decisions

Records are kept here (arc42 chapter 9 as ADR log) rather than scattered in
commit messages. Each ADR: *status, context, decision, consequence*.

| # | Decision | Status |
|---|----------|--------|
| ADR-1 | **Agent via subprocess, not direct LLM API.** `execute` shells out to an OpenAI-compatible agent CLI (`pi --provider X --model Y -p @prompt`). Orchestrator stays thin; reuses agent logic + prompt templates; no vendor coupling. Consequence: features the agent CLI lacks are unavailable; we never reimplement an agent. | accepted |
| ADR-2 | **One process; std thread-per-task concurrency** (amended at implementation: subprocess orchestration needs no async runtime). Mutual exclusion via in-process per-repo merge mutex + per-worker busy map; no cross-process flock. Consequence: simpler state, no cross-process races; a single long task blocks its worker only. | accepted (amended) |
| ADR-3 | **JSON state store, no SQLite.** Atomic temp+rename writes, single writer, greppable/diffable. Consequence: fine up to ~100s of tasks; revisit if we need queries/locking. | accepted |
| ADR-4 | **Config schema compatibility** with taskfleet's shipped `tasks.json`/`workers.json` examples. Consequence: real campaign configs act as the integration corpus; drop-in migration. | accepted |
| ADR-5 | **Spec → contract → test pyramid as the dev workflow.** Every change starts as an OpenSpec change (requirements/design/tasks); module boundaries carry executable contract tests (totality, exit codes, error contracts); the test pyramid (unit ≫ integration ≫ E2E) fills underneath, with a fake agent CLI + scratch git repo so CI needs no network/LLM. | accepted |
| ADR-6 | **v1 scope = core orchestrator only.** Excluded from v1: Bayesian routing, episodic memory, trust/constitution/corrections/transparency, vLLM worker, openspec bridge, multi-repo (see `docs/multi-repo-design.md`). Documented in 01/04 as v2. | accepted |
| ADR-7 | **arc42 documentation, one file per chapter** in `docs/arc42/`, maintained in the same change as the code. | accepted |
| ADR-8 | **Repo split: taskfleet private/internal (bash, campaign runner), agentflow public (Rust product).** Public repo never contains secrets or internal configs. | accepted |
| ADR-9 | **Wall-clock time, not token count, is the receipt/elapsed truth** (learned from the shell forks). | accepted |
| ADR-10 | **Agent sandbox = env allowlist + git hygiene + opt-in wrapper seam; no embedded OS sandbox.** The agent child is untrusted: it gets an empty environment plus system basics, the dispatched worker's `api_key_env`, and `TF_AGENT_ENV_PASSTHROUGH`; `GIT_TERMINAL_PROMPT=0` and an empty `credential.helper` prevent auth hangs/theft. Real filesystem/network containment is delegated to a user-provided wrapper (`TF_SANDBOX_CMD`, e.g. firejail/bwrap) because OS sandboxes are platform-specific and af stays dependency-free. Limits documented in ch. 8.8. | accepted |
| ADR-11 | **Multi-repo via optional `repos.json` + per-task resolution; unknown repo names warn and fall back.** Deps stay a global DAG; worktree/branch/merge target the task's resolved repo; merges stay globally serialized (correct for any repo count). Fallback (not hard error) keeps ADR-4: taskfleet corpora compose across files and `repo: "main"` must never block single-repo planning. | accepted |
| ADR-12 | **Measured routing over first-free: every attempt leaves a receipt with its outcome; free workers are picked by UCB1 (mean reward + exploration term), ties in config order.** Fresh state reproduces the old behavior exactly; history is cumulative (ponytail: add windowing if stale workers ever haunt us). Trust (wins/total) is surfaced in `af cost`. | accepted |

## Review items (open by default assumptions)

- ADR-1 alternative B (direct API) remains technically possible; chosen A for v1.
- ADR-6 list is the default; any extras can be promoted per change.

## How an ADR is made

1. OpenSpec change must include the decision in its design or its own ADR entry.
2. This table is updated in the same change. Never retrofit decisions silently.
