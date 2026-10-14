# Capability: lifecycle

## ADDED Requirements

### Requirement: Turn-cap escalation on retry

For attempts driven by the builtin harness, before the harness loop starts the
orchestrator SHALL inspect stored receipts of the same task: if any earlier
failed attempt's error matches `harness: turn cap N reached`, the attempt SHALL
run with an effective turn cap of `2 × N_max` where `N_max` is the highest such
`N`, clamped to at most **128**, regardless of the configured base cap. If no
prior turn-cap failure exists, the effective cap SHALL resolve exactly as
before (task override → worker field → `TF_AGENT_MAX_TURNS` → 32).

#### Scenario: Retry after turn-cap death doubles the budget

- **WHEN** attempt 1 of task `A` fails with `harness: turn cap 12 reached`
  and attempt 2 is dispatched via the builtin harness
- **THEN** attempt 2 runs with a turn cap of 24

#### Scenario: Escalation is bounded

- **WHEN** a task's receipts record turn-cap failures at 64 and 128
- **THEN** the next attempt runs with a turn cap of 128 (ceiling), not 256

#### Scenario: Non-cap retries keep the configured cap

- **WHEN** attempt 1 of task `A` fails with a provider timeout and attempt 2
  is dispatched
- **THEN** attempt 2 runs with the cap the configuration resolves to
  (no escalation)

#### Scenario: Escalation survives resume

- **WHEN** a run is killed after attempt 1 died at the turn cap, and a fresh
  `af run` against the same state dir dispatches attempt 2
- **THEN** the receipts persisted by attempt 1 still trigger the doubled cap
