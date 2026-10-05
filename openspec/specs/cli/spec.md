# cli Specification

## Purpose
TBD - created by archiving change rust-orchestrator. Update Purpose after archive.

## Requirements

### Requirement: Run commands

`af run` SHALL run the dispatch loop until all tasks are done or deadlock, honoring `--once` (one dispatch round), `--dry-run` (show plan, change nothing), `--worker <name>`, `--task <id>`, and `--poll <secs>`.

#### Scenario: dry run changes nothing

WHEN `af run --dry-run` runs against a config with pending tasks
THEN it prints the dispatch plan, creates no worktrees, and exits 0.

#### Scenario: full run completes

WHEN `af run` runs against a config whose tasks all pass
THEN all tasks reach `done` and the process exits 0.

### Requirement: Status and inspection commands

`af status` SHALL print a human-readable status board; `af api status [--json]` SHALL output machine-readable status; `af api results --task <id>` SHALL show gate output and result; `af attach <id>` SHALL tail a running task's live log.

#### Scenario: json status

WHEN `af api status --json` runs
THEN valid JSON with every task's state is printed to stdout.

#### Scenario: attach tails log

WHEN a task is running and `af attach <id>` runs
THEN it streams that task's log lines until the task finishes.

### Requirement: Cost report

`af cost` SHALL append a per-worker trust section: worker name, wins/total,
and trust rate (wins ÷ attempts, two decimals), computed from receipt
outcomes.

#### Scenario: trust section lists each worker with history

GIVEN receipts exist for workers w1 (2 merged, 1 failed) and w2 (1 merged)
WHEN `af cost` runs
THEN the trust section shows `w1 2/3 0.67` and `w2 1/1 1.00`.
