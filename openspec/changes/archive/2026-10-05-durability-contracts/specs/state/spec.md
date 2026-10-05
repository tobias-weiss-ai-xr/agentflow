# Delta: state

## ADDED Requirements

### Requirement: Single-writer state lock

`af run` SHALL acquire an exclusive lock on its state directory before reading
or writing state, so two orchestrators can never clobber one `run-state.json`.
`Store::acquire_lock` SHALL create `<state_dir>/.lock` atomically
(O_CREAT|O_EXCL) recording the owning process id, and SHALL return a
`LockGuard` that releases the lock when dropped. When the lock is already held
by a live process, acquisition SHALL fail with an error naming the owning pid
and `af run` SHALL exit with status 2. A lock whose recorded pid is no longer
alive SHALL be reclaimed.

#### Scenario: second writer is rejected naming the owner

WHEN one `af run` holds the state lock and a second `af run` targets the same state directory
THEN the second acquisition fails with an error naming the owning pid and the second process exits 2.

#### Scenario: lock is released on drop

WHEN the lock-holding `af run` ends and its `LockGuard` drops
THEN the `.lock` file is removed and the next `af run` acquires the directory.

#### Scenario: stale lock is reclaimed

GIVEN a `.lock` file whose recorded pid is no longer alive
WHEN `af run` starts
THEN it reclaims the lock and proceeds.

### Requirement: Attempt phase journal

`af` SHALL journal each attempt's furthest phase to persisted state at the
effect boundaries — `Spawned` after the worktree exists and the agent is about
to run, `AgentDone` after the agent's change is committed and in scope, and
`GatePassed` after the acceptance gate passes — with the phase serialized in
`snake_case`. On startup, each stale `running` attempt SHALL resume from its
journaled phase so a crash never re-runs the non-replayable agent step:
`Spawned` (or a legacy state file with no phase) SHALL re-run the agent,
`AgentDone` SHALL re-run only the acceptance gate then merge, and `GatePassed`
SHALL re-run only the idempotent merge.

#### Scenario: phase boundaries persist at every step

WHEN an attempt completes successfully
THEN persisted status shows the attempt reached at least `AgentDone` and the phase serializes snake_case.

#### Scenario: crash after agent done resumes at the gate

GIVEN persisted phase `agent_done` for a stale running task
WHEN `af run` restarts
THEN it re-runs only the acceptance gate and merges the committed branch without invoking the agent.

#### Scenario: crash after gate passed merges only

GIVEN persisted phase `gate_passed` for a stale running task
WHEN `af run` restarts
THEN it re-runs only the merge — no agent and no gate — and the merge is idempotent if the branch already landed.

#### Scenario: legacy state resumes by re-running the agent

GIVEN a persisted running task whose state file predates the journal and has no phase field
WHEN `af run` restarts
THEN the phase defaults to `Spawned` and the attempt re-runs the agent.

### Requirement: Storage backend conformance suite

`af` SHALL define a `StateStore` persistence interface — `status_file`, `load`,
`save`, `append_receipt`, `load_receipts` — and SHALL provide a reusable
conformance suite that any backend must pass. The suite SHALL assert that a
torn write never corrupts the readable status file, that receipts are
append-only (equal keys never overwrite one another), that `load_receipts`
returns receipts ordered by timestamp, and that a saved status map round-trips
through a fresh handle. The shipped JSON `Store` SHALL pass the suite.

#### Scenario: torn write never corrupts

WHEN a status write is interrupted and a partial temp file is left behind
THEN loading the store returns the last complete map and never the partial file.

#### Scenario: receipts are append-only

WHEN two receipts with the same task, attempt and timestamp are appended
THEN both are present when receipts are loaded, with no overwrite.

#### Scenario: receipts order by timestamp

WHEN receipts are appended out of timestamp order
THEN `load_receipts` returns them sorted ascending by timestamp.

#### Scenario: save round-trips across reopen

WHEN a status map is saved and a fresh handle over the same location is opened
THEN it loads an equal map.

#### Scenario: shipped backend conforms

WHEN the conformance suite is pointed at `Store`
THEN every check passes.
