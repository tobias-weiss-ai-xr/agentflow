# Capability: scheduling

## ADDED Requirements

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

#### Scenario: no deadlock while progress possible

WHEN at least one task is `running` or `ready`
THEN the run does not exit.
