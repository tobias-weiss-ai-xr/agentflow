# config Specification (delta)

## Modified Requirement: Task schema loading

`af` SHALL support two new optional Task fields:

- `max_turns` — a positive integer capping the number of LLM round-trips for a
  `cli: "builtin"` attempt of THIS task, overriding the worker's `max_turns`,
  then `TF_AGENT_MAX_TURNS`, then the harness default of 32. Validation SHALL
  reject a `max_turns` of 0 or negative, naming the task. Absent = no
  per-task override (existing behavior unchanged).
- `readonly` — a boolean (default `false`). When `true`, the task is
  investigation-only: the builtin harness SHALL reject every `write` and
  `edit` tool call, and the attempt SHALL complete when the agent stops
  normally without running an acceptance gate or merging. Validation SHALL
  reject `readonly: true` combined with an `accept` gate (a gate on a
  read-only task is contradictory) — a hard error naming the task.

#### Scenario: per-task max_turns parses and validates

WHEN a task declares `max_turns: 12`
THEN it loads successfully, and a `max_turns: 0` task fails loading with an
error naming the task.

#### Scenario: readonly parses and validates

WHEN a task declares `readonly: true` and no `accept`
THEN it loads without warning.
WHEN a task declares `readonly: true` and an `accept` command
THEN loading fails with a hard error naming the task.
