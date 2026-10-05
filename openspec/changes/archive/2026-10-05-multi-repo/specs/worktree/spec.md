# Delta: worktree

## ADDED Requirements

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
