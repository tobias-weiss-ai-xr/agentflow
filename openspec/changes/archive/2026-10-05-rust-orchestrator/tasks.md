# Tasks: Rust Orchestrator (agentflow)

## 1. Scaffolding

- [x] 1.1 Create Cargo crate `agentflow` with binary `af` and deps `serde`, `serde_json`; `cargo build` compiles
- [x] 1.2 Implement CLI arg parser (`run/status/api/attach/cost` + flags) with unit tests for parse errors

## 2. Config (spec: config)

- [x] 2.1 Implement `Task` / `Worker` data models with `Deserialize` and defaults
- [x] 2.2 Implement config loader: `tasks.json` + `workers.json` + `TF_*` env overrides
- [x] 2.3 Implement validation: unique ids, resolvable deps, no dependency cycles, gate-or-manual, ≥1 enabled worker
- [x] 2.4 Unit tests: every validation failure scenario from spec (each asserts an error message)

## 3. State (spec: state)

- [x] 3.1 Implement atomic JSON store (temp + rename) with read/write
- [x] 3.2 Implement status machine transitions (ready/running/done/failed) + persistence before transition
- [x] 3.3 Implement receipt append + `cost` aggregation (wall-clock truth)
- [x] 3.4 Unit tests: torn-write safety, transition persistence, receipt math

## 4. Scheduling (spec: scheduling)

- [x] 4.1 Implement DAG build with depths and cycle detection
- [x] 4.2 Implement ready-queue computation: deps done, free worker, max_parallel, contention (overlapping scope defer), critical-path priority
- [x] 4.3 Implement retry budget (`max_attempts`) and deadlock detection (clean exit, code 2, names blocked tasks)
- [x] 4.4 Unit tests: ordering, contention, retry exhaustion, seeded property test for deadlock on random DAGs

## 5. Subprocess + gate (spec: lifecycle)

- [x] 5.1 Implement subprocess helper: spawn, capture stdout/stderr, hard timeout, kill on timeout, exit-code classification
- [x] 5.2 Implement gate runner: `accept` shell with scoped env (`TF_GATE_ENV`), timeout (`accept_timeout_s`), exit-0 contract
- [x] 5.3 Integration/unit tests: timeout kills, exit codes, env scoping (fake shell commands)

## 6. Worktree (spec: worktree)

- [x] 6.1 Implement worktree create/remove + branch delete via git CLI, `TF_BRANCH_PREFIX` naming
- [x] 6.2 Implement serialized merge per repo (in-process mutex); conflict → failed, never force-push
- [x] 6.3 Integration tests on a scratch git repo: lifecycle, serialized merges, conflict case

## 7. Execute pipeline (spec: lifecycle)

- [x] 7.1 Implement prompt rendering from `prompts/worker.md` template (title/id/scope/acceptance_prose)
- [x] 7.2 Implement `execute`: worktree → agent → gate → merge → status; retry with fresh branch
- [x] 7.3 Integration test with a fake agent CLI + scratch repo (happy path, gate fail, agent fail, retry)

## 8. Run loop + CLI (spec: cli)

- [x] 8.1 Implement `run` loop: poll → reap → dispatch → sleep; `--once`, `--dry-run`, `--worker`, `--task`, `--poll`
- [x] 8.2 Implement `status`, `api status [--json]`, `api results --task`, `attach`, `cost` commands
- [x] 8.3 E2E test (fake agent + scratch repo): full `af run` completes with all tasks done; `--dry-run` changes nothing

## 9. Docs + repo hygiene (ADR-7, ADR-8)

- [x] 9.1 Update `docs/arc42/09-architecture-decisions.md` ADR-2 wording (std threads) and add implementation-status notes where needed
- [x] 9.2 Rewrite public `README.md` for the Rust product; `.gitignore` real `config/workers.json` and `state/`
- [x] 9.3 Remove internal configs/state/run-*.sh wrappers from the public working tree; set `taskfleet` repo to private via `gh repo edit`
