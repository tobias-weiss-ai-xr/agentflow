# scheduling Specification

## Purpose
TBD - created by archiving change rust-orchestrator. Update Purpose after archive.

## Requirements

### Requirement: Dependency DAG

The scheduler SHALL build a DAG from task `deps`, and SHALL only dispatch a task when all of its dependencies are in state `done`. The DAG SHALL be validated for cycles at load time.

#### Scenario: dependency ordering

WHEN task B lists A as a dep and A is not yet done
THEN B is not dispatched, and is dispatched only after A reaches `done`.

#### Scenario: cycle rejected

WHEN tasks form a dependency cycle
THEN loading fails with an error naming the cycle.

### Requirement: Critical-path priority

Tasks with a deeper dependency depth SHALL be dispatched before shallower tasks when both are ready.

#### Scenario: deeper task first

WHEN A and B are both ready and B has dependents while A has none
THEN B is dispatched before A.

### Requirement: Scope contention avoidance

When two ready tasks have overlapping `scope` file globs, they SHALL NOT be dispatched concurrently (default `defer`: one waits for the other to finish).

#### Scenario: overlapping scope deferred

WHEN task X and task Y both declare the same file glob and both are ready
THEN only one is dispatched; the other is held until the first finishes.

#### Scenario: disjoint scope dispatches in parallel

WHEN task X and task Y declare disjoint globs
THEN both may be dispatched concurrently.

### Requirement: Retry with fresh branch

A task SHALL allow up to `max_attempts` attempts (default from worker defaults). Each attempt SHALL use a newly created branch and worktree; a failed attempt SHALL NOT be retried on dirty state.

#### Scenario: retry after gate failure

WHEN a task's gate fails on attempt 1 and `max_attempts = 3`
THEN the task is re-queued for a fresh attempt and runs at most 3 times total.

### Requirement: Deadlock detection

When the scheduler has ready-queueable work only through tasks that are permanently `failed`, or when no task can make progress, `af run` SHALL exit cleanly with a non-zero status and a message naming the blocking tasks.

#### Scenario: all tasks blocked by failure

WHEN every remaining task depends on a task that is `failed` with attempts exhausted
THEN the run exits with status code 2 and lists the blocked tasks.

#### Scenario: absent dependency is a deadlock

WHEN a task's dep id does not exist in the loaded config (merged in from a sibling file that is not present)
THEN the run exits cleanly with status 2 instead of looping forever.

#### Scenario: no deadlock while progress possible

WHEN at least one task is `running` or `ready`
THEN the run does not exit.

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
