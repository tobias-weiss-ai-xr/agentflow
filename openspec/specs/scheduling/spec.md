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

#### Scenario: priority breaks ties among equally deep ready tasks

GIVEN several ready tasks at the same dependency depth with different `priority` ranks and no scope overlap
WHEN the scheduler orders them for dispatch
THEN the higher rank is dispatched first, and tasks with equal rank keep their original order from the config file.

### Requirement: Scope contention avoidance

When two ready tasks have overlapping `scope` file globs, they SHALL NOT be dispatched concurrently (default `defer`: one waits for the other to finish).

#### Scenario: overlapping scope deferred

WHEN task X and task Y both declare the same file glob and both are ready
THEN only one is dispatched; the other is held until the first finishes.

#### Scenario: disjoint scope dispatches in parallel

WHEN task X and task Y declare disjoint globs
THEN both may be dispatched concurrently.

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
merged attempt, 0 for a failed one. A strictly higher score SHALL always
win: measured reliability never yields to a declared expense assumption —
cost-per-task is confounded by task difficulty (the hard work goes to the
trusted worker), so a cost term in the score itself would penalise a worker
for being given the hard tasks and starve it. A score tie within a small
tolerance SHALL prefer the worker with the strictly cheaper DECLARED cost
basis (`params_b` / `price_per_mtok_usd`) when the two bases are comparable;
otherwise ties SHALL break in config order, so a fresh state with no
declared bases reproduces the previous first-free behavior exactly.

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

#### Scenario: cheaper worker wins a score tie

GIVEN two free workers with identical recorded stats (their scores tie) whose declared cost bases are comparable, and the cheaper one is not first in config order
WHEN a task dispatches
THEN the cheaper worker is chosen regardless of config order.

#### Scenario: a strictly better score beats a cheaper competitor

GIVEN a free worker with a strictly better win rate whose declared cost basis is more expensive than its competitor's
WHEN both are free
THEN the better-scoring worker is chosen; cost never overrides measured trust.

#### Scenario: incomparable bases keep config order

GIVEN a score tie where one worker declares a price and the other only a parameter count, or where either worker declares no cost basis at all
WHEN a task dispatches
THEN config order decides; a price is never converted into a parameter count.

### Requirement: Scope enforcement on agent edits

After the agent exits successfully, `af` SHALL compute the attempt branch's
changed files (a three-dot diff against the base branch) and check every path
against the task's declared `scope`, using the same matcher as the scheduler's
contention check. A path that no scope entry allows SHALL fail the attempt
before the durable `AgentDone` boundary and before any merge. An empty scope
SHALL allow any file.

#### Scenario: out-of-scope edit fails before merge

WHEN the agent edits a file that no declared scope entry allows
THEN the attempt fails naming the offending path, the task is not merged, and the change never reaches the base branch.

#### Scenario: in-scope edit still merges

WHEN every changed file matches a declared scope entry
THEN scope enforcement passes and the attempt proceeds to the gate and merge.

#### Scenario: empty scope allows any file

WHEN a task declares no scope
THEN every changed file is accepted.

### Requirement: Retry reuses verified agent work

A task SHALL allow up to `max_attempts` attempts (default from worker
defaults). When an attempt fails only at the acceptance gate, the attempt
branch SHALL be kept (it carries the agent's committed, scope-clean work),
and the next attempt SHALL attach to that branch and re-run ONLY the gate —
the agent SHALL NOT be re-invoked while its verified work survives. A scope
violation, agent failure, or merge failure SHALL discard the attempt branch,
and the next attempt SHALL be a fresh agent run on a newly created branch.
If the gate fails again on the gate-only reuse path, the branch SHALL be
dropped so the following attempt is a fresh agent run.

#### Scenario: gate failure retries only the gate

GIVEN a task whose attempt 1 commits its agent work, passes the scope check, and fails only the gate
WHEN the task is retried while the attempt branch still carries that committed work
THEN the retry re-runs only the gate, the agent is not invoked again, and the task merges once the gate passes.

#### Scenario: gate failure keeps the verified branch

GIVEN a task whose agent committed its work and the gate failed
WHEN the failed attempt is cleaned up
THEN only the worktree directory is removed — the attempt branch survives carrying the agent's commits beyond the base branch.

#### Scenario: a second gate failure falls back to a fresh attempt

GIVEN a gate-only retry whose gate fails again with `max_attempts = 3`
WHEN the next attempt starts
THEN the task runs a total of 3 attempts with receipts `failed, failed, merged` — the second gate failure drops the branch so the final attempt is a fresh agent run that can merge.

#### Scenario: attempts are bounded by max_attempts

GIVEN a task whose agent fails on every attempt and `max_attempts = 2`
WHEN the task runs to exhaustion
THEN exactly 2 attempts are made and the task ends `failed`.
