# Delta: cli

## MODIFIED Requirements

### Requirement: Cost report

`af cost` SHALL append a per-worker trust section: worker name, wins/total,
and trust rate (wins ÷ attempts, two decimals), computed from receipt
outcomes. `af cost` SHALL also report wasted spend — the total wall-clock
seconds and the attempt count of attempts whose outcome was not `merged`,
the wasted percentage of all selected attempts, and a breakdown grouped by
failure reason — computed over the SAME receipt selection as the rest of the
report, so `--last`, `--since`, and `--task` narrow the waste figures too.

#### Scenario: trust section lists each worker with history

GIVEN receipts exist for workers w1 (2 merged, 1 failed) and w2 (1 merged)
WHEN `af cost` runs
THEN the trust section shows `w1 2/3 0.67` and `w2 1/1 1.00`.

#### Scenario: wasted spend surfaces failed attempts

GIVEN receipts where 2 of 4 attempts failed (100.0s on one failure reason, 20.0s on another)
WHEN `af cost` runs
THEN the report shows the waste total `WASTED: 120.0s on 2 of 4 attempt(s) (50.0%)` and a by-reason breakdown naming each failure reason with its seconds, and `--last` narrows the waste figures to the same selected receipts.
