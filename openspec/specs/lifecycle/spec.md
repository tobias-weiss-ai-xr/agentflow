# lifecycle Specification

## Purpose
TBD - created by archiving change rust-orchestrator. Update Purpose after archive.

## Requirements

### Requirement: Execute pipeline

For each task attempt, `af` SHALL: create a git worktree on a fresh branch; render a prompt from the task template; spawn the agent CLI with `--provider <p> --model <m> -p @<prompt-file>`; if the agent succeeds, run the acceptance gate; if the gate passes, merge the branch to the base branch; otherwise record failure/retry. When an attempt fails after the agent has committed work — a non-zero exit, a stall-watchdog kill, the total agent timeout, a scope violation, an acceptance-gate failure, or a merge conflict — `af` SHALL preserve that committed work on an archived branch (`<branch>-rejected-<now>`) rather than destroying it with cleanup, and SHALL name that branch in the failure reason (so the receipt carries it). Archiving is best-effort and can never fail the attempt.

#### Scenario: happy path

WHEN the agent exits 0 and the acceptance gate exits 0
THEN the task branch is merged to the base branch and the task reaches `done`.

#### Scenario: gate failure

WHEN the agent exits 0 but the gate exits non-zero
THEN the task is marked `failed` (or retried), never merged.

#### Scenario: agent failure

WHEN the agent CLI exits non-zero
THEN the attempt fails without running the gate and without merging.

### Requirement: Subprocess execution contract

All subprocesses (agent, gate, git) SHALL run through one helper that captures stdout/stderr, applies a hard timeout, kills the process tree on timeout or abort, and classifies the result by exit code (success / non-zero / killed-by-us / missing-binary). No subprocess SHALL outlive the orchestrator.

#### Scenario: timeout kills

WHEN a gate exceeds its timeout
THEN the process is killed, the attempt fails, and no merge happens.

### Requirement: Prompt rendering

A task prompt SHALL be rendered from `prompts/worker.md` with the task's `title`, `id`, `scope`, and `acceptance_prose` substituted, and written to a file passed to the agent CLI. For attempt ≥ 2, the rendered prompt SHALL also include a "Previous attempts" block listing this task's earlier failed attempts (attempt number + error line) so the agent avoids repeating them. Attempt 1 SHALL render without the block.

#### Scenario: template substitution

WHEN a task with title and acceptance prose is dispatched
THEN the rendered prompt file contains the task title and acceptance prose.

#### Scenario: retry prompt names the earlier failure

GIVEN attempt 1 of task A failed with an agent exit code
WHEN attempt 2 renders its prompt
THEN the prompt contains a previous-attempts entry for attempt 1.

#### Scenario: first attempt has no history block

GIVEN a task dispatching its first attempt
WHEN the prompt renders
THEN no previous-attempts block appears.

### Requirement: Gate replay contract

Every task SHALL carry a `gate_replay` flag that defaults to `true`, and `af`
SHALL export the effective decision to the acceptance gate as the environment
variable `TF_GATE_REPLAY` (`1` when replay is on, `0` when a task opts out).
This lets a user-authored acceptance gate detect a resumed or replayed run and
stay idempotent.

#### Scenario: gate_replay defaults to true

WHEN a task omits `gate_replay`
THEN the loaded task has `gate_replay == true`.

#### Scenario: explicit opt-out parses

WHEN a task declares `"gate_replay": false`
THEN the loaded task has `gate_replay == false`.

#### Scenario: replay decision reaches the gate

WHEN the acceptance gate runs
THEN `TF_GATE_REPLAY` is exported to the gate process as `1` when replay is on and `0` when it is off.

### Requirement: Prompt placeholder guarantee

Every agent-prompt render path SHALL emit the task's file scope and the exact
acceptance gate command, and SHALL leak no unresolved `{{...}}` placeholder or
conditional marker. A configured template that omits `{{SCOPE}}`,
`{{ACCEPTANCE}}` or `{{ACCEPT_CMD}}` SHALL be reported on stderr and repaired
by appending the missing sections; a template that cannot be read SHALL fall
back to the built-in default. An empty scope SHALL render as the `*` wildcard.

#### Scenario: every render path carries scope and gate command

WHEN a prompt renders from the built-in default, a complete custom template, or a hostile template that omits the placeholders
THEN the output contains every scope path, the task id and title, and the exact `accept` command verbatim.

#### Scenario: no placeholder leaks

WHEN any render path produces a prompt
THEN the output contains no `{{` token, because substituted placeholders and conditional markers are both removed.

#### Scenario: hostile template is repaired

WHEN a custom template omits `{{SCOPE}}`, `{{ACCEPTANCE}}` and `{{ACCEPT_CMD}}`
THEN `af` warns on stderr and appends auto-filled scope, acceptance and gate-command sections.

#### Scenario: empty scope renders the wildcard

WHEN a task declares no scope entries
THEN the rendered prompt shows `*` for the file scope.

### Requirement: Attempt work is durable before it is judged

Before an attempt is judged, `af` SHALL make the agent's work durable on the
attempt branch. When the agent exits 0 leaving uncommitted changes in its
worktree, `af` SHALL stage and commit those changes on the attempt branch
(without passing `--no-verify`, so the operator's git hooks still run) so the
scope check, the acceptance gate and the merge all judge exactly the same
committed tree. If that commit fails, the attempt SHALL fail with a reason
naming the commit error; a tree that cannot be made durable SHALL NOT be
judged or merged.

#### Scenario: dirty worktree is committed before judging

WHEN the agent exits 0 leaving uncommitted work in its worktree
THEN the work is committed on the attempt branch before the scope check, the gate and the merge, and a passing attempt merges with the change present in the base branch's committed tree.

#### Scenario: an uncommittable tree fails the attempt

WHEN the agent's uncommitted work cannot be committed
THEN the attempt fails with a reason naming the commit error and no merge happens.

### Requirement: An attempt that produces no change is not merged

`af` SHALL count the commits the attempt branch carries beyond the base branch
after the agent's work is made durable and BEFORE the scope check. A count of
zero SHALL fail the attempt with a deterministic reason naming the zero-commit
condition; such an attempt SHALL NOT be reported as merged and its task SHALL
NOT reach `done`. Before reporting an attempt as merged, `af` SHALL verify that
the attempt branch tip is an ancestor of the base branch, and SHALL fail the
attempt naming the discrepancy when it is not.

#### Scenario: zero-commit attempt fails, never merged

WHEN the agent exits 0 but the attempt branch carries zero commits beyond the base branch
THEN the task does not reach `done`, the receipt outcome is not `merged`, and the failure reason names the zero-commit condition.

#### Scenario: a reported merge is verified in the base

WHEN the attempt's merge succeeds
THEN `af` verifies the attempt branch tip is an ancestor of the base branch before reporting the attempt as merged.

### Requirement: Pre-dispatch reuse of an archived branch

When a task becomes ready and a worker is about to be picked/dispatched,
`af run` SHALL FIRST look for an archived rejected branch for that task in
its repository (`<branch_prefix>/<id>-rejected-<ts>`, with an optional `-<n>`
collision suffix) and select the NEWEST by PARSING the numeric `<ts>` (then
the `<n>` suffix) and never by git's output order or committer dates. It
SHALL re-validate that branch against the task's CURRENT scope and gate
exactly as `af recover` does, reusing the same enforcement helpers: (a)
SCOPE — every path the archived branch changes relative to the merge base
with the base branch MUST be covered by the task's CURRENT `scope` (an empty
scope means any file); (b) GATE — unless the task is `manual`, the
acceptance gate MUST pass on the checked-out archived branch, including the
task's `gate_replay`. When it qualifies, `af run` SHALL merge it with the
message `af: <id> — <title>`, set the task to `Done` with phase `GatePassed`,
consume (delete) the archived branch, and take NO agent dispatch for it. The
gate remains the SOLE arbiter, so reuse can only ever save money and never
accept work a fresh attempt would have had to redo. When the newest archive
is out of scope or fails the gate, `af run` SHALL leave the archived branch
in place for `af clean`/a later `af recover` and fall through to the normal
agent dispatch, without retrying that branch again in the same run, and SHALL
log the reason at most once per candidate. A task SHALL be considered for
pre-dispatch reuse at most once per run — the first time it becomes ready — so
an archive produced by an attempt within the SAME run is left to the retry
machinery (`scheduling`) instead of being re-validated in a loop. Reuse SHALL
NOT fire for a task with no archive, a task already `Done`, or when
`TF_NO_REUSE=1` is set (the operator's explicit escape hatch for a clean
re-run). Reuse SHALL NOT append
a receipt: the attempt that produced the archive already has one, and reuse
is not a new attempt. The temporary worktree SHALL be removed on every path.

#### Scenario: an in-scope archive is reused without an agent

GIVEN a ready task whose CURRENT scope covers the archived branch's change
and whose CURRENT gate passes on it
WHEN `af run` dispatches the task
THEN the archive is merged, the task reaches `done`, the archived branch is
deleted, and no agent is invoked.

#### Scenario: an out-of-scope archive falls through to the agent

GIVEN a ready task with an archived branch whose change includes a file the
task's CURRENT scope does not cover
WHEN `af run` dispatches the task
THEN the archived branch survives and the run dispatches the agent normally.

#### Scenario: a gate-failing archive falls through to the agent

GIVEN a ready task with an archived branch that fails the task's CURRENT
acceptance gate
WHEN `af run` dispatches the task
THEN the archived branch survives and the run dispatches the agent normally.

#### Scenario: TF_NO_REUSE opts out

GIVEN `TF_NO_REUSE=1` and a ready task with a reusable archived branch
WHEN `af run` dispatches the task
THEN the archive is not reused and the agent is dispatched normally.
