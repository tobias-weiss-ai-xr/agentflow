# Capability: lifecycle

## ADDED Requirements

### Requirement: Write-time scope guard for builtin workers

For attempts driven by the builtin harness (`cli: "builtin"`), a `write` or
`edit` tool call whose target path — resolved against the WORKTREE so
`..`/absolute/drive-relative escapes are rejected by the existing containment
guard first — is not covered by any entry in the task's `scope` SHALL be
rejected at tool-call time, and the rejection SHALL be returned to the model
as the tool result naming the path and the allowed scope, so the model can
correct course instead of burning the attempt. An empty `scope` means "any
file" and SHALL allow every path, exactly as at the end-of-attempt check.
This fail-fast check is a guard, not a substitute: the end-of-attempt scope
check and its archive-preserving failure SHALL remain for writes performed
through the `bash` tool and for CLI workers. Scope coverage SHALL be decided
by the same matcher as enforcement (`scheduler::scope_overlap` via
`execute::scope_violations`), so tool-time admission and attempt-end
enforcement always agree.

#### Scenario: out-of-scope write rejected at call time

GIVEN a builtin attempt with scope `["src/**"]`
WHEN it calls `write` on `README.md`
THEN the tool result is an error naming the path and allowed scope, and the
attempt continues.

#### Scenario: in-scope write allowed

GIVEN a builtin attempt with scope `["src/**"]`
WHEN it calls `write` on `src/lib.rs`
THEN the write succeeds.

### Requirement: Readonly attempt completion

`af` SHALL treat a task with `readonly: true` specially at attempt time: the
builtin harness rejects `write`/`edit` tool calls regardless of scope, and
when the agent stops normally the attempt SHALL be reported done WITHOUT
running an acceptance gate and WITHOUT merging — a readonly task produces no
committed task work by construction. A readonly attempt that leaves committed
or uncommitted work SHALL still preserve it via the standard archive path, and
cleanup SHALL remove the worktree. CLI-worker readonly attempts SHALL behave
the same: a normal stop = done, no gate, no merge.

#### Scenario: readonly attempt completes without gate or merge

GIVEN a `readonly: true` task
WHEN its agent stops normally
THEN the attempt is reported done, no acceptance gate runs, and no branch is
merged to the base.

#### Scenario: readonly write rejected by the harness

GIVEN a readonly builtin attempt
WHEN it calls `write`
THEN the tool result is an error stating the task is read-only.

### Requirement: Gate and agent-tool shell selection

Acceptance gates and the builtin harness's `bash` tool SHALL run via
`/bin/sh -c` on unix. On Windows, `af` SHALL prefer a POSIX `sh` on PATH
(git-bash) — probed once per process and cached — and SHALL fall back to
`cmd /C` only when no `sh` is reachable, so that POSIX-authored gates and
agent commands keep working on Windows hosts that ship git-bash. The
selection SHALL be a single shared decision: the agent's `bash` tool and
the acceptance gate runner MUST use the same shell function, so a dialect
that works for one works for the other.

#### Scenario: bash-idiom gate passes on Windows with git-bash

GIVEN a Windows host with git-bash `sh` on PATH
WHEN a gate uses POSIX idioms (`[ ]`, backticks, single-quoted pipes,
`./prog`)
THEN the gate passes under `sh` (where it previously failed as
"'.' is not recognized").

#### Scenario: no POSIX sh falls back to cmd

GIVEN a Windows host without `sh` on PATH
WHEN a gate runs
THEN the gate runs via `cmd /C`.
