# scheduling Specification (delta)

## Modified Requirement: UCB1 worker selection

The UCB1 score for a worker SHALL use a Laplace-smoothed trust mean —
`(wins + 1) / (attempts + 2)` — in place of the raw `wins / attempts` for
the exploitation term, so a worker with a single failed attempt scores 0.33
(not 0.00) and a single win scores 0.67 (not 1.00), converging to the
empirical rate as attempts grow. The exploration term and every tie-break
(declared cost, then mean duration, then config order) SHALL be unchanged. A
worker with no receipts keeps its prior-only score of 0.5. The display trust
column in `af cost` SHALL keep showing the RAW rate (measured data stays
measured); the smoothed mean is the routing mean only.

#### Scenario: single loss is not scored zero

WHEN a worker has 1 attempt and 0 wins while a rival has no attempts
THEN the 0/1 worker's exploitation term is 1/3, not 0, so a single disaster
does not disqualify the worker outright.

#### Scenario: prior converges to the empirical rate

WHEN a worker has 100 attempts with 50 wins
THEN the smoothed mean is 51/102 ≈ 0.5, within rounding of the raw 0.5.
