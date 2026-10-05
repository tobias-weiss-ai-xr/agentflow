# Delta: cli

## ADDED Requirements

### Requirement: Cost report

`af cost` SHALL append a per-worker trust section: worker name, wins/total,
and trust rate (wins ÷ attempts, two decimals), computed from receipt
outcomes.

#### Scenario: trust section lists each worker with history

GIVEN receipts exist for workers w1 (2 merged, 1 failed) and w2 (1 merged)
WHEN `af cost` runs
THEN the trust section shows `w1 2/3 0.67` and `w2 1/1 1.00`.
