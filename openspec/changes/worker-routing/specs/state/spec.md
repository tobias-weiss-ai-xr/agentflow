# Delta: state

## MODIFIED Requirements

### Requirement: Cost receipts

Every attempt end SHALL append one receipt (wall-clock truth, ADR-9) —
including failed attempts. Receipts gain `outcome` (`"merged"` | `"failed"`);
receipts written before this change SHALL deserialize with outcome
`"merged"` (they were only written on success).

#### Scenario: failed attempts are recorded

GIVEN a task whose agent exits non-zero twice, then merges on attempt 3
WHEN receipts are loaded
THEN there are 3 receipts for the task with outcomes
`failed, failed, merged`.

#### Scenario: legacy receipts stay loadable

GIVEN a receipts file from before this change (no `outcome` field)
WHEN receipts are loaded
THEN outcome is `"merged"` and `af cost` output is unchanged.
