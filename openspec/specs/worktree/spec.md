# worktree Specification

## Purpose
TBD - created by archiving change rust-orchestrator. Update Purpose after archive.

## Requirements

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

### Requirement: Worktrees target the task's repository

Worktree creation, branch creation/deletion, and merging SHALL operate on the
task's resolved repo (`repo_dir_for`), not always `TF_REPO_DIR`. The worktree
directory remains `$TF_WORKTREE_ROOT/<task_id>/` and the branch remains
`<prefix>/<task_id>` in the target repo. Merges stay serialized (single
in-process lock) — correct for any repo count.

#### Scenario: cross-repo campaign lands in the right repos

GIVEN repos `main` and `aux`, task A on `main`, task B on `aux` (deps: A)
WHEN the campaign runs to completion
THEN A's changes are merged into `main` and B's into `aux`, each on the base
branch of its own repo.

### Requirement: Self-heal spans all repositories

Startup orphan cleanup SHALL attempt worktree removal and branch deletion for
stale worktree dirs against every configured repo plus the default repo
(best-effort, never fails the run).

#### Scenario: stale worktree is cleaned regardless of its repo

GIVEN a leftover worktree dir from a task whose repo is `aux`
WHEN the next run starts
THEN `aux` reports the worktree removed and `aux`'s stale branch is deleted.

### Requirement: Rejected work is preserved on an archived branch

A rejected attempt's committed work SHALL survive the cleanup that follows
its failure, preserved as a COPY under `<branch>-rejected-<now>` (never a
rename — the original branch, the cleanup, and the retry are unaffected),
so the retry still starts clean from the current base while the paid-for
work stays recoverable and the receipt names the archived branch. Archiving
SHALL be best-effort and can never fail the attempt.

#### Scenario: scope violation archives the committed work

WHEN an attempt fails because the agent edited a file outside the task's scope
AND archiving the attempt branch succeeds
THEN the work is kept on `<branch>-rejected-<now>`, the failure message names
that branch, the archived branch contains the agent's commit, and the original
branch is removed so a retry starts clean.

#### Scenario: merge conflict archives the committed work

WHEN an attempt's committed branch conflicts with the base branch at merge
time AND archiving the attempt branch succeeds
THEN the work is kept on `<branch>-rejected-<now>`, the failure message names
that branch, and the original branch is removed so a retry starts clean.

#### Scenario: archiving never fails the attempt

WHEN a rejected attempt cannot be archived (a git error, or a name collision
that cannot be resolved)
THEN the attempt still fails with its original message unchanged and no empty
suffix, exactly as if archiving had not been attempted.
