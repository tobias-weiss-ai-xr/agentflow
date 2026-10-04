# 11. Technical Risks

| # | Risk | Likelihood | Impact | Mitigation | Owned by |
|---|------|-----------|--------|-----------|----------|
| R1 | **Zombie / leaked subprocess** (agent or gate killed, children survive) | medium | high: hung workers, orphan worktrees | subprocess helper kills process trees on any abort path; startup self-heal removes orphans (8.3, 6.2) | core::execute |
| R2 | **Merge race / conflicting concurrent merges** | low | high: corrupted base branch | single per-repo merge mutex; gate must pass before merge; conflict detection → task failed, never force-push | core::worktree |
| R3 | **Gate timeout hang** (shell waits on network etc.) | medium | medium: worker stuck | hard `accept_timeout_s` deadline on all gates | core::gate |
| R4 | **Secret / internal data leak into public repo** | medium | high: reputational, credentials | ADR-8; git-ignore real configs; CI secret-scan; review gate in OpenSpec changes | maintainer |
| R5 | **Semantic drift from proven taskfleet behavior** (subtle scheduling difference) | medium | medium: surprising campaign results | config schema compat + campaign configs as integration corpus; ADR log; port behavior, not blind redesign | scheduler |
| R6 | **Cost/energy burn from endless retries** | medium | high: bill shock | max_attempts cap enforced in scheduler; wall-clock receipts; `af cost` visibility | scheduler |
| R7 | **Deadlock detection misses novel DAG shape** | low | medium: hang instead of clean exit | property-based tests on random DAGs (seeded) | scheduler |
| R8 | **CI flakiness from live LLM calls** | high | high: unusable CI | default test path uses **fake** agent CLI + scratch repo; real-agent E2E is opt-in `--ignored` | tests |

## Risk table → action

- R1, R2, R3, R6 are **v1 must-have** (contract tests + pyramid cover them).
- R4 is a process + CI gate, not code.
- R5, R7, R8 shape the test suite (seeded property tests, fake agent harness).
