# Delta: scheduling

## ADDED Requirements

### Requirement: UCB1 worker selection

Among free, enabled workers eligible for dispatch, `af` SHALL pick the
worker maximizing `mean_reward + sqrt(2·ln(N+1)/(n+1))` where `n` is the
worker's recorded attempts, `N` the total recorded attempts (replayed from
receipts at startup, updated in-memory per attempt), and reward is 1 for a
merged attempt, 0 for a failed one. Ties SHALL break in config order, so a
fresh state reproduces the previous first-free behavior exactly.

#### Scenario: fresh state picks first configured worker

GIVEN no receipts exist and all workers are free
WHEN a task dispatches
THEN the first enabled worker in config order is chosen.

#### Scenario: unexplored worker is tried before a failing one

GIVEN worker A has 0/3 wins and worker B has no history
WHEN both are free
THEN B is chosen (exploration term dominates).

#### Scenario: reliable worker wins at equal counts

GIVEN worker A has 3/3 wins and worker B has 0/3 wins
WHEN both are free
THEN A is chosen.
