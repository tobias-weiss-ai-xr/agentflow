# Delta: scheduling

## REMOVED Requirements

### Requirement: Retry with fresh branch

Superseded by `Retry reuses verified agent work`. Round 7 (`r7-gate-retry`)
deliberately changed the gate-failure case: a gate failure keeps the attempt
branch and the retry re-runs only the gate. The old requirement's rule that
each attempt uses a newly created branch, and its `retry after gate failure`
scenario, described pre-round-7 behavior and were false against the
implementation and its green tests.

## ADDED Requirements

### Requirement: Retry reuses verified agent work

A task SHALL allow up to `max_attempts` attempts (default from worker
defaults). When an attempt fails only at the acceptance gate, the attempt
branch SHALL be kept (it carries the agent's committed, scope-clean work),
and the next attempt SHALL attach to that branch and re-run ONLY the gate —
the agent SHALL NOT be re-invoked while its verified work survives. A scope
violation, agent failure, or merge failure SHALL discard the attempt branch,
and the next attempt SHALL be a fresh agent run on a newly created branch.
If the gate fails again on the gate-only reuse path, the branch SHALL be
dropped so the following attempt is a fresh agent run.

#### Scenario: gate failure retries only the gate

GIVEN a task whose attempt 1 commits its agent work, passes the scope check, and fails only the gate
WHEN the task is retried while the attempt branch still carries that committed work
THEN the retry re-runs only the gate, the agent is not invoked again, and the task merges once the gate passes.

#### Scenario: gate failure keeps the verified branch

GIVEN a task whose agent committed its work and the gate failed
WHEN the failed attempt is cleaned up
THEN only the worktree directory is removed — the attempt branch survives carrying the agent's commits beyond the base branch.

#### Scenario: a second gate failure falls back to a fresh attempt

GIVEN a gate-only retry whose gate fails again with `max_attempts = 3`
WHEN the next attempt starts
THEN the task runs a total of 3 attempts with receipts `failed, failed, merged` — the second gate failure drops the branch so the final attempt is a fresh agent run that can merge.

#### Scenario: attempts are bounded by max_attempts

GIVEN a task whose agent fails on every attempt and `max_attempts = 2`
WHEN the task runs to exhaustion
THEN exactly 2 attempts are made and the task ends `failed`.
