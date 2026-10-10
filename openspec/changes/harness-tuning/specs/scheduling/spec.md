# Capability: scheduling

## MODIFIED Requirements

### Requirement: UCB1 worker selection

Among free, enabled workers eligible for dispatch, `af` SHALL pick the
worker maximizing `mean_reward + sqrt(2·ln(N+1)/(n+1))` where `n` is the
worker's recorded attempts, `N` the total recorded attempts (replayed from
receipts at startup, updated in-memory per attempt), and reward is 1 for a
merged attempt, 0 for a failed one. The trust mean SHALL be Laplace-smoothed
`(wins+1)/(attempts+2)` rather than the raw `wins/attempts`: a single attempt
is 0-or-1 noise, so a lone failure scores 1/3 (not 0) and a lone win 2/3
(not 1), converging to the empirical rate as attempts accumulate; the display
trust column in `af cost` SHALL keep showing the RAW rate — measured data
stays measured, the smoothed mean is the routing term only. A strictly higher
score SHALL always win: measured reliability never yields to a declared
expense assumption — cost-per-task is confounded by task difficulty (the hard
work goes to the trusted worker), so a cost term in the score itself would
penalise a worker for being given the hard tasks and starve it. A score tie
within a small tolerance SHALL prefer the worker with the strictly cheaper
DECLARED cost basis (`params_b` / `price_per_mtok_usd`) when the two bases
are comparable; when the costs do not decide — bases incomparable or equal —
the tie SHALL go to the worker with the strictly lower MEAN wall-clock
duration over its VERDICT attempts (`merged` or `failed`; an `interrupted`
receipt's duration is a placeholder, never a measurement, and SHALL NOT enter
the mean); remaining ties SHALL break in config order, so a fresh state with
no declared bases reproduces the previous first-free behavior exactly. The
full tie-break ordering is therefore trust > declared price > duration >
config order. Duration, like cost, is a tie-break ONLY and never a score
term: wall-clock is confounded by task difficulty (the hard tasks go to
the trusted worker), so a duration term in the score itself would
penalise a worker for being given the hard tasks and starve it — while
UCB1's exploration term still guarantees an unpicked worker is eventually
tried, so no tie-break can starve one. A worker with no duration history
SHALL never displace the incumbent on a tie, and a missing duration SHALL
never be treated as zero.

#### Scenario: fresh state picks first configured worker

GIVEN no receipts exist and all workers are free
WHEN a task dispatches
THEN the first enabled worker in config order is chosen (all score the prior
mean 0.5, an exact tie).

#### Scenario: unexplored worker is tried before a failing one

GIVEN worker A has 0/3 wins and worker B has no history
WHEN both are free
THEN B is chosen (exploration term dominates).

#### Scenario: reliable worker wins at equal counts

GIVEN worker A has 3/3 wins and worker B has 0/3 wins
WHEN both are free
THEN A is chosen.

#### Scenario: a single loss is not scored zero

GIVEN worker A has 0/1 wins and worker B has no history
WHEN both are free and the exploration terms would not override the means
THEN A's routing mean is 1/3, not 0, so a single disaster does not
disqualify a worker outright.

#### Scenario: cheaper worker wins a score tie

GIVEN two free workers with identical recorded stats (their scores tie) whose declared cost bases are comparable, and the cheaper one is not first in config order
WHEN both are free
THEN the cheaper worker is chosen.

#### Scenario: a strictly better score beats a cheaper competitor

GIVEN a free worker with a strictly better win rate whose declared cost basis is more expensive than its competitor's
WHEN both are free
THEN the better-scoring worker is chosen; cost never overrides measured trust.

#### Scenario: incomparable bases keep config order

GIVEN a score tie where one worker declares a price and the other only a parameter count, or where either worker declares no cost basis at all
WHEN a task dispatches
THEN config order decides; a price is never converted into a parameter count.

#### Scenario: faster worker wins only when trust and cost tie

GIVEN two free workers whose scores tie, whose declared cost bases do not decide the tie (both undeclared, or equal), and whose mean verdict-attempt durations differ
WHEN a task dispatches
THEN the worker with the strictly lower mean duration is chosen regardless of config order, while a strictly better trust score or a strictly cheaper declared price still beats a faster worker; a worker with no duration history never displaces the incumbent (config order stands), and an `interrupted` receipt never enters the mean.
