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

An attempt's committed work SHALL survive the cleanup that follows EVERY
failure path, preserved as a COPY under `<branch>-rejected-<attempt>-<now>`
(the attempt number in the name keeps each paid-for attempt individually
selectable — `af recover --attempt N`); the older, attempt-less
`<branch>-rejected-<now>` SHALL remain valid for archives that predate the
attempt-in-name form and for startup heal, which has no attempt to name.
Archiving is never a rename — the original branch, the cleanup, and the retry
are unaffected — so the retry still starts clean from the current base while
the paid-for work stays recoverable and the receipt names the archived
branch. The
failure paths are: a scope violation, a merge conflict, an agent that exits
non-zero, the stall-watchdog kill, the total agent timeout, a second gate
failure of a reused (gate-only retry) attempt, and startup self-heal sweeping
a stale worktree whose task is no longer running. Any uncommitted change is
committed first so it is part of the preserved tip. Archiving SHALL be
best-effort and can never fail the attempt; a branch that carries no work
beyond the base SHALL NOT be archived.

#### Scenario: scope violation archives the committed work

WHEN an attempt fails because the agent edited a file outside the task's scope
AND archiving the attempt branch succeeds
THEN the work is kept on `<branch>-rejected-<attempt>-<now>`, the failure message names
that branch, the archived branch contains the agent's commit, and the original
branch is removed so a retry starts clean.

#### Scenario: merge conflict archives the committed work

WHEN an attempt's committed branch conflicts with the base branch at merge
time AND archiving the attempt branch succeeds
THEN the work is kept on `<branch>-rejected-<attempt>-<now>`, the failure message names
that branch, and the original branch is removed so a retry starts clean.

#### Scenario: archiving never fails the attempt

WHEN a rejected attempt cannot be archived (a git error, or a name collision
that cannot be resolved)
THEN the attempt still fails with its original message unchanged and no empty
suffix, exactly as if archiving had not been attempted.

#### Scenario: a stalled agent archives the committed work

WHEN the agent commits work and then stops producing output until the stall
watchdog kills the attempt
THEN the committed branch is archived as `<branch>-rejected-<attempt>-<now>` before
cleanup, the receipt names that branch, the archived tip contains the agent's
work, the task does NOT reach `done`, and the original branch is removed.

#### Scenario: a timed out agent archives the committed work

WHEN the agent commits work and then runs past the total agent timeout with
the stall watchdog disabled
THEN the committed branch is archived as `<branch>-rejected-<attempt>-<now>` before
cleanup, the receipt names that branch, the archived tip contains the agent's
work, and the task does NOT reach `done`.

#### Scenario: a second gate failure archives the reused work

WHEN a gate-only retry re-runs the gate on a kept attempt branch and the gate
fails a second time
THEN the reused branch is archived as `<branch>-rejected-<attempt>-<now>` before it is
dropped, the failure message names that branch, and the work remains
recoverable even though the next attempt starts fresh.

#### Scenario: startup heal archives an interrupted attempt's work

WHEN startup self-heal finds a stale worktree whose task is no longer running
AND its branch carries commits beyond the base
THEN the branch is archived as `<branch>-rejected-<now>` before the worktree
and branch are removed, while a task still marked running is never touched
and never archived.

#### Scenario: the work survives every failure path

WHEN an attempt fails after the agent has committed work (scope violation,
merge conflict, non-zero exit, stall, timeout, a second gate failure, or
startup heal of an interrupted attempt)
THEN the work is preserved under `<branch>-rejected-<attempt>-<now>` (or the
legacy `<branch>-rejected-<now>` when no attempt is named) rather than being
destroyed by cleanup.

### Requirement: af clean sweeps archived rejected branches

`af clean [--dry-run]` SHALL also remove the archived rejected branches that
rejected-work preservation leaves behind — local branches named
`<prefix>/<id>-rejected-<attempt>-<unix-ts>` or the legacy
`<prefix>/<id>-rejected-<unix-ts>` (each with an optional `-<n>` collision
suffix). A branch whose recovered task id is still marked `running` in the
state file SHALL be kept, exactly like a running worktree. With `--dry-run`
af SHALL report each branch it would remove as `would remove branch <name>`
and mutate no ref; without it af SHALL remove each branch (`git branch -D`)
and report `removed branch <name>`. The sweep SHALL be best-effort: a git
error is never fatal and the exit code stays 0. The existing orphan-worktree
output, ordering, and messages SHALL stay byte-identical, and a repository
with archived branches but no orphan worktrees SHALL still do the work
rather than short-circuiting on the `no orphaned worktrees` message.

#### Scenario: sweeps archived branches but keeps a running task's

GIVEN archived rejected branches `tf/dead-rejected-111` and
`tf/live-rejected-222`, where task `live` is still marked `running`
WHEN `af clean` runs
THEN `tf/dead-rejected-111` is gone and reported as `removed branch tf/dead-rejected-111`
AND `tf/live-rejected-222` survives and is not reported as removed.

### Requirement: Recovered branches are re-validated then merged

`af recover` SHALL check an archived rejected branch out in a temporary
worktree on its own tip (the copy is never created or moved — the committed
work it points at is exactly what re-validation must preserve), re-validate
its scope and gate, and on success merge that exact branch into the base so
the agent's committed work — never re-generated — becomes the task's merge
commit. Selection of which archived branch to recover SHALL parse the numeric
`<ts>` (then the `-<n>` collision suffix) and pick the NEWEST, never relying
on git's output order or committer dates. The temporary worktree is removed
on every exit path; a failed recovery keeps the archived branch in place.

#### Scenario: recovery checks out the archived branch on its own tip

WHEN `af recover` selects an archived rejected branch for re-validation
THEN the branch is checked out in a throwaway worktree on its own committed tip
(no new branch), the scope and gate run against that committed state, and the
worktree is removed whether recovery succeeds or fails.

#### Scenario: an archived branch is consumed by a successful recovery

GIVEN a scope-and-gate passing archived rejected branch
WHEN `af recover --task <id>` merges it into the base
THEN the recovered worktree AND the archived branch are removed, because the
work now lives in the base — leaving the rejected ref behind would be a ghost
for `af clean`.

### Requirement: The attempt's work is committed before it is merged

`af` SHALL commit any uncommitted agent work on the attempt branch before
merging it, so a merge carries real content rather than reporting success for
a branch that is already the base. `af` SHALL verify that the attempt branch
tip is an ancestor of the base branch before reporting the attempt as merged,
and SHALL NOT report a merge that the base does not contain.

#### Scenario: merge carries the attempt's real content

WHEN an attempt's worktree is dirty at judgment time
THEN the work is committed on the attempt branch and the merge carries it into the base, where `git show <base>:<path>` finds the committed file.

#### Scenario: a no-op merge is not reported

WHEN the attempt branch carries nothing beyond the base branch
THEN the attempt fails rather than reporting a merge that contains no work.
