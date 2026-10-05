# Delta: state

## MODIFIED Requirements

### Requirement: Cost receipts

Each attempt SHALL append a receipt (task id, attempt number, worker, model, wall-clock elapsed, agent-reported tokens if available) to `state/receipts/` — every attempt end, including failed attempts (wall-clock truth, ADR-9). Receipts carry `outcome` (`"merged"` | `"failed"`) and `error: Option<String>` — the first line of the attempt's failure reason, capped at 200 characters, `None` for merged attempts. Receipts written before this change SHALL deserialize with outcome `"merged"` and `error = None`. `af cost` SHALL aggregate receipts (last run, since date, or per task).

#### Scenario: receipt appended per attempt

WHEN an attempt finishes
THEN a receipt record exists for it.

#### Scenario: cost aggregates receipts

WHEN receipts exist for a run
THEN `af cost --last` prints total elapsed and per-task breakdown.

#### Scenario: failed attempts are recorded

GIVEN a task whose agent exits non-zero twice, then merges on attempt 3
WHEN receipts are loaded
THEN there are 3 receipts for the task with outcomes
`failed, failed, merged`.

#### Scenario: legacy receipts stay loadable

GIVEN a receipts file from before this change (no `outcome` field)
WHEN receipts are loaded
THEN outcome is `"merged"` and `af cost` output is unchanged.

#### Scenario: failed receipt carries the reason

GIVEN an agent that exits 7
WHEN the attempt's receipt is loaded
THEN `error` contains the failure's first line.

#### Scenario: legacy receipts parse with no error

GIVEN a receipt file from before this change
WHEN receipts are loaded
THEN `error` is `None`.
