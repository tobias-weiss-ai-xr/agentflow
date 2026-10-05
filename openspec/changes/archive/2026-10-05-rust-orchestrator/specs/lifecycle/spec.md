# Capability: lifecycle

## ADDED Requirements

### Requirement: Execute pipeline

For each task attempt, `af` SHALL: create a git worktree on a fresh branch; render a prompt from the task template; spawn the agent CLI with `--provider <p> --model <m> -p @<prompt-file>`; if the agent succeeds, run the acceptance gate; if the gate passes, merge the branch to the base branch; otherwise record failure/retry.

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

A task prompt SHALL be rendered from `prompts/worker.md` with the task's `title`, `id`, `scope`, and `acceptance_prose` substituted, and written to a file passed to the agent CLI.

#### Scenario: template substitution

WHEN a task with title and acceptance prose is dispatched
THEN the rendered prompt file contains the task title and acceptance prose.
