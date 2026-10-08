# 11. Technical Risks

## 11.1 Risk Overview

| Risk | Likelihood | Impact | Mitigation | Status |
|------|------------|--------|------------|--------|
| R1: Agent produces partially committed changes | High | High | Dirty-commit BEFORE judging; `commits_ahead` guard; merge-verify | **Mitigated in round 13** |
| R2: False-green merge (0 commits ahead) | Medium | High | Fail attempt explicitly; `manual: true` escape hatch | **Mitigated in round 13** |
| R3: Work lost on crash | Medium | High | Single writer + effect sandwich + atomic writes + heal at startup | **Mitigated in rounds 9-13** |
| R4: Scope violation silently permitted | Medium | High | `changed_paths` vs `scope` check; `touch` config-time validation | **Mitigated in rounds 11-13** |
| R5: State corruption from concurrent `af run` | Low | High | Lock file with pid; dead pid reclaim | **Mitigated** |
| R6: Version skew (code vs state file schema) | Low | Medium | State files are append-only receipt lists + simple status map; unknown fields ignored by serde | **Accepted** |
| R7: Toxic worker trust poisoning | Low | Medium | Trust excludes interrupted/recovered; only merged/failed count; history is cumulative | **Mitigated** |
| R8: Provider-reported cost is always 0 | Medium | Medium | Ledger marks measured vs estimated; measured outranks declared; `-` for unknown | **Accepted (provider limitation)** |
| R9: Timing flakes in test suite | Medium | Low | Slack ≥ 1.7x; retry_pacing widened from 1s→2s (round 15) | **Mitigated** |
| R10: Git pre-commit hooks reject orchestrator commits | Low | Medium | Commit failures are surfaced; agent's hooks are its problem; operator can opt out via `--no-verify` in config | **Accepted** |
| R11: Multi-repo deadlock | Low | Medium | Merges serialized per repo + globally; deps stay global DAG | **Mitigated** |
| R12: Orchestrator killed mid-agent (SIGKILL/timeout) | Low | Medium | Attempt recorded as `interrupted`; worktree archived at heal; receipt with 0.0s wall-clock; `af cost` INTERRUPTED line | **Mitigated** |
| R13: Stub binary coverage debt | Low | Low | Keep `example_agent` thin; logic in pure, unit-tested functions | **Accepted** |
| R14: ctrl-C during dispatch prints no receipt | Low | Low | Handler (SIGINT/SIGTERM) saves receipt as `interrupted` | **Future** |

## 11.2 Detailed Risk Discussion

### R1 & R2: Partial commits and vacuous success

**Discovery:** Round-13 recon found a latent false-green. An agent that edits
but never commits passes vacuously: `changed_paths` sees no committed changes
→ scope check passes; gate runs in the worktree, sees the dirty file, passes;
`git merge --no-ff` returns "Already up to date" (exit 0) → `Merged`/`Done`
with nothing in the base.

**Mitigation proven:**
1. `execute::execute_attempt` now commits a dirty worktree with a generated
   message BEFORE running scope check, gate, and merge
2. `commits_ahead > 0` guard: `ahead == 0` → `Failed("agent produced no change...")`
3. `branch_merged_into_head` verifies the tip is an ancestor of HEAD

**20-merge audit:** 0 zero-file merges across all real campaigns; defect was
Latent.

### R3: Work loss on crash

**Mitigation layers:**
- **Single writer:** `state/state.lock` with pid prevents concurrent writers
- **Atomic writes:** temp+fsync+rename for every JSON file
- **Effect sandwich:** `AttemptPhase` journaled at every boundary;
  `resume_action` derives what to redo after crash
- **Startup heal:** `worktree::heal` archives stale worktrees from dead
  attempts; `heal_stale_attempt` finishes only uncommitted effects
- **Append-only receipts:** Receipts are never overwritten; temp filename for
  each new receipt prevents collision

**Residual:** An `interrupted` receipt (orchestrator killed mid-attempt) has
`wall_clock_s: 0.0` because the elapsed time is unknown. Duration-specific
statistics exclude interrupted receipts.

### R4: Scope violation silently permitted

**Mitigation:**
- **Runtime:** `scope_violations(changed_paths, task.scope)` checks every
  changed file against every scope glob
- **Config-time:** `af validate` rejects any `touch` entry not covered by a
  `scope` entry
- **Archive:** Failed scope check still calls `preserve_work`, so the work is
  recoverable after scope widening

**False factor:** Standard glob matching ensures `src/*.rs` covers `src/main.rs`
but not `tests/main.rs`.

### R9: Timing flakes in test suite

**Discovery:** Round 13 observed one load-sensitive flake in 320 runs
(9-min llvm-cov build). Round 15 identified the exact test:
`retry_pacing::retry_delay_paces_attempts_without_delaying_the_happy_path`.

**Root cause:** Two upper-bound assertions compared sequential campaign
elapsed times with only 1s of slack. Under 8 concurrent CPU hogs, variance
~1.5s.

**Mitigation:** Widened slack from 1s → 2s (commit `2d5caa4`). Lower-bound
assertions (`paced >= 3s`) were not changed (cannot flake from load). Other
timing tests have ≥ 1.7x headroom (Wakeup 20s/6s, Watchdog 20s/12s, Budget
20s/8s, Contract Gate 15-20s/1s) and passed 10/10 under 8 CPU hogs.

## 11.3 Residuals and Accepted Risks

| Risk | Why accepted | Future work |
|------|--------------|-------------|
| R8: Provider cost always 0 | Every real provider tested reports `usage.cost.total: 0`; not actionable without provider changes | Provider limitation; agentflow ready to capture when providers ship |
| R10: Pre-commit hooks | Agent's hooks are its own; operator can configure or opt out | None; by design |
| R13: Stub binary coverage | `llvm-cov` does not attribute spawned processes; thin printer pattern | None; unavoidable |
| R14: ctrl-C receipt | SIGINT/SIGTERM handler not yet wired to save receipts | Add signal handler in `main.rs` |
