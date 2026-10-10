# lifecycle Specification (delta)

## Modified Requirement: Builtin-harness attempt lifecycle

For attempts driven by the builtin harness (`cli: "builtin"`), two
enforcement changes:

### Write-time scope guard

The harness SHALL reject, at tool-call time, a `write` or `edit` whose target
path (worktree-relative) is not covered by any entry in the task's `scope`,
returning the error to the model as the tool result (naming the path and the
allowed scope). An empty `scope` means "any file" and SHALL allow every path,
exactly as at the end-of-attempt check. The scope rejection must resolve the
path against the WORKTREE, so `..`/absolute/drive-relative escapes are
rejected by the existing containment guard first. This fail-fast check is a
guard, not a substitute: the end-of-attempt scope check and archive-preserving
failure SHALL remain for writes performed through the `bash` tool and for CLI
workers.

#### Scenario: out-of-scope write rejected at call time

WHEN a builtin attempt with scope `["src/**"]` calls `write` on `README.md`
THEN the tool result is an error naming the path and allowed scope, and the
attempt continues (the model can correct course rather than burn an hour).

#### Scenario: in-scope write allowed

WHEN a builtin attempt with scope `["src/**"]` calls `write` on `src/lib.rs`
THEN the write succeeds.

### Readonly attempts

`af` SHALL treat a task with `readonly: true` specially at attempt time: the
harness rejects `write`/`edit` tool calls (as scoped narrowing, scope empty or
not); when the agent stops normally the attempt SHALL be reported done WITHOUT
running an acceptance gate and WITHOUT merging — the branch holds no committed
task work by construction. A readonly attempt that leaves uncommitted work in
the worktree SHALL still be preserved by the standard archive path. CLI-worker
readonly attempts SHALL behave the same: a normal stop = done, no gate, no
merge.

#### Scenario: readonly attempt completes without gate or merge

WHEN a `readonly: true` task's agent stops normally
THEN the attempt is reported done, no acceptance gate runs, and no branch is
merged to the base.

#### Scenario: readonly write rejected by the harness

WHEN a readonly builtin attempt calls `write`
THEN the tool result is an error stating the task is read-only.
