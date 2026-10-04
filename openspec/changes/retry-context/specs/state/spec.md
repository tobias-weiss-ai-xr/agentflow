# Delta: state

## MODIFIED Requirements

### Requirement: Cost receipts

Every attempt receipt SHALL also carry `error: Option<String>`: the first
line of the attempt's failure reason, capped at 200 characters, `None` for
merged attempts. Receipts written before this change SHALL deserialize with
`error = None`.

#### Scenario: failed receipt carries the reason

GIVEN an agent that exits 7
WHEN the attempt's receipt is loaded
THEN `error` contains the failure's first line.

#### Scenario: legacy receipts parse with no error

GIVEN a receipt file from before this change
WHEN receipts are loaded
THEN `error` is `None`.
