# Capability: worktree

## ADDED Requirements

### Requirement: Worktree lifecycle

`af` SHALL create and remove git worktrees and delete task branches via the git CLI subprocess, with branch names prefixed by `TF_BRANCH_PREFIX`.

#### Scenario: create and remove

WHEN a task attempt starts, a worktree exists at `TF_WORKTREE_ROOT/<id>` on a fresh branch; when it ends, the worktree and branch are removed.

### Requirement: Merge serialization

Merges into the base branch SHALL be serialized per repository (one in-process lock) so concurrent task merges never race. Only gate-passing tasks SHALL merge. A merge conflict SHALL mark the task failed rather than force-pushing.

#### Scenario: serialized merges

WHEN multiple tasks pass their gates at the same time
THEN merges happen one at a time and the base branch remains consistent.

#### Scenario: conflict fails task

WHEN a task branch conflicts with the base branch at merge time
THEN the task is marked `failed` (retry allowed), never force-pushed.
