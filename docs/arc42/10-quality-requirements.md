# 10. Quality Requirements

## 10.1 Quality Goals

| Priority | Goal | Topics |
|----------|------|--------|
| 1 ("must") | **Correctness of gate semantics.** A passing gate proves the acceptance criteria; a failing gate blocks merge. Never infer success from exit codes, lack of errors, or succeed without durable evidence in the base branch. | Contract tests, E2E probes, 08.4 Honest Success |
| 1 | **+Never destroy work.** Every failure path preserves the agent's committed changes as a recoverable archive; kill signals, timeouts, and scope failures all archive before cleanup. | `preserve_work`/`preserve_and_note` in `execute.rs`; `archive_branch` in `worktree.rs`; `heal` at startup |
| 1 | **Worktree isolation.** Each attempt runs in its own git worktree + branch; retries are fresh branches; concurrent attempts never see each other's partial state. | `worktree.rs`, `execute.rs`, `run.rs` |
| 1 | **Atomic persistence.** State files (status, receipts) survive every crash scenario without corruption. Single writer; temp+fsync+rename writes; append-only receipts. | `state.rs` |
| 2 | **+Directly measurable routing.** UCB1 from receipts; tie-breaks declared cost then duration; trust surfaced in `af cost`. Excludes invented heuristics, proxy conversions, or name-based inference. | `router.rs`, 08.2 |
| 2 | **Test pyramid.** Unit tests > integration > E2E for every feature. 340 tests, 96.83% line coverage, raise-only ratchet. | CI, `.coverage-min`, `scripts/coverage-gate.sh` |
| 2 | **Zero runtime dependencies** beyond std + serde. | `Cargo.toml` |
| 3 | **Spec contract tests.** Every public module boundary carries executable guarantees (totality, exit codes, error contracts). | contract/ modules in `tests/` |
| 3 | **+Honest ledger.** `af cost` reports what happened, not what was hoped. Measured cost separate from estimated; interrupted vs failed vs recovered vs merged all distinct. | `run.rs::cost`, `state.rs::Receipt` |
| 3 | **Config-time safety.** `af validate` catches bad configs (duplicate ids, cycle in deps, `touch` outside `scope`, unusable cost bases, no enabled workers) before an agent is paid. | `config.rs::validate` |

> Starred items (✓) are **hardened by round-13**: work integrity, honest success, and
> work preservation are now property-checked on every real and synthetic
> campaign.

## 10.2 Quality Tree (GQM)

**Goal:** Trustworthy LLM-task orchestrator

- **Question:** Does every gate-pass result in correctly merged work?
  - **Metric:** E2E probe count passinghonest-success checks (after round 13: all
    probes pass for dirty-commit, no-change, merge-verify)
- **Question:** Is work preserved on every failure path?
  - **Metric:** Branch archive exists + carries agent commits on every synthetic
    failure injection (11 arms in `execute.rs`), plus startup heal
- **Question:** Are routing decisions trustworthy?
  - **Metric:** UCB1 from receipts only; trust percentage in `af cost` excludes
    interrupted/recovered outcomes; no invented conversions between Basis variants
- **Question:** Is persistence crash-safe?
  - **Metric:** 0 torn-writes on SIGKILL during I/O (temp+fsync+rename atomic
    install); `af run` refuses to start when state is torn

## 10.3 Quality Scenarios

| # | Scenario | Motivation | Verification |
|---|----------|------------|--------------|
| QS-1 | Gate passes only when acceptance criteria met | Prevent false greens (round 13's dirty-commit defect) | E2E probe: dirty-agent task with no commit reported as Failed, not Done |
| QS-2 | Dirty worktree committed before judging | Scope-gate-merge judge identical committed tree | Unit test `changed_paths` + `commits_ahead` guard in `execute.rs` |
| QS-3 | Zero-commit attempt fails with explicit reason | Prevent vacuous success | `execute.rs` line ~385: `"agent produced no change"` |
| QS-4 | Merge claims verified against base | Prevent mistaken "Already up to date" | `branch_merged_into_head` check after merge |
| QS-5 | Archive on every failure path | 11 failure arms + heal | Grep 11 `preserve_work`/`preserve_and_note` calls in `execute.rs` |
| QS-6 | Archivename includes attempt (current) + back-compat | Recover & reuse identify correct archive | Parser disambiguation by TS_FLOOR (≥ 1_000_000_000) |
| QS-7 | Recovered work excluded from WASTED | Honest ledger for cost accounting | `recovered` receipt pairs with failed receipt; `af cost` RECOVERED line |
| QS-8 | Interrupted receipts excluded from trust | Orphaned attempts don't poison worker trust | `counts_as_verdict()` excludes `OUTCOME_INTERRUPTED` and `OUTCOME_RECOVERED` |
| QS-9 | Measured cost (provider) outranks declared | Truthfulness ladder | `attempt_expense` checks `cost_micros` first; ledger marks measured vs estimated |
| QS-10 | Atomically installed status + receipts | Crash safety | temp+fsync+rename writes; torn write detected at load with error |
| QS-11 | Single writer lock prevents concurrent state corruption | Heal, resume, run ordering | `state/state.lock` with pid; dead pid reclaim |
| QS-12 | UCB1Trust ≠ f(price, duration) | Decoupling | `router.rs` score uses only wins/total; tie-break consults cost & duration separately |
| QS-13 | Scope check uses durable diff (commit, not worktree) | `changed_paths` = `git diff <base>...<branch>` |
| QS-14 | `af validate` catches `touch ⊄ scope` at config time | Prevent unpassable tasks | `config.rs::validate` checks every `touch` entry against `scope` |

## 10.4 Test levels

| Level | Count | Purpose | Example |
|-------|-------|---------|---------|
| Unit (crate internals) | 205 | Pure functions; boundaries stubbed | `router::tests`, `scheduler::tests`, `worktree::tests` |
| Integration (module joining) | 37 | Multiple modules wired; scratch git repos; fake agent | `retry_pacing.rs`, `interrupted.rs`, `receipt_atomic.rs` |
| Contract (module guarantees) | 30 | Totality, exit codes, error contracts per public module | `contract_worktree.rs`, `contract_scheduler.rs` |
| E2E (full binary) | 53 | Real `af` binary, scratch repos, behavioral assertions | `cli.rs`, `e2e.rs` |
| Spec traceability | 4 | OpenSpec scenarios ↔ test fixtures | `spec_traceability.rs` |
| **Total** | **340** | | |

Flow-down: unit tests prove each function; integration joins modules;
contract tests prove the interface; E2E validates the binary; spec
finite-state scenarios tested by both unit and E2E.

## 10.5 Coverage Ratchet

- Floor: **94%** line coverage (`.coverage-min`)
- Gate: `scripts/coverage-gate.sh` runs `cargo llvm-cov --workspace
  --show-missing-lines --fail-under-lines $(cat .coverage-min)`
- Policy: **raise-only**; lowering requires revising the floor in
  `.coverage-min`
- Current: **96.83% total / 97.42% line** (post round 14)
- CI: `cargo llvm-cov` installed via GitHub Actions workflow
- Note: Spawned stub binaries (`example_agent`) are NOT covered by llvm-cov
  (subprocess, not in-process) — they must remain thin printers

## 10.6 Static Analysis

- `cargo fmt --check` on every commit and CI
- `cargo clippy --all-targets -- -D warnings` on every commit and CI
- No lints suppressed

## 10.7 Contract Testing Discipline (ADR-5)

Every public module carries victim tests that:
1. Enumerate every public item (function, type, const)
2. State its exit/error contract
3. Prove each contract with at least one test case

Ordered validation:
- Unit tests (behavior of the function alone)
- Contract tests (guarantees at the boundary)
- Integration tests (function + neighbors)
- E2E tests (binary + system)

Renamed/removed items are treated as breaking at the boundary; contract tests
fail and must be updated before the change ships.
