# Capability: config

## ADDED Requirements

### Requirement: Per-task turn cap and readonly task fields

`af` SHALL support two optional Task fields beyond the schema documented
above.

- `max_turns` — an integer >= 1 capping the number of LLM round-trips for a
  `cli: "builtin"` attempt of THIS task. It SHALL override the worker's
  `max_turns`, then `TF_AGENT_MAX_TURNS`, then the harness default of 32, only
  for this task. Validation SHALL reject `max_turns: 0` or negative with a
  hard error naming the task. Absent = no per-task override.
- `readonly` — a boolean (default `false`). When `true` the task is
  investigation-only: `af` SHALL complete the attempt when the agent stops
  normally, with no acceptance gate and no merge, and the builtin harness
  SHALL reject every `write` and `edit` tool call. Validation SHALL reject
  `readonly: true` combined with an `accept` gate — a gate on a read-only task
  is contradictory — with a hard error naming the task. Absent = normal task
  behavior unchanged.

#### Scenario: per-task max_turns parses and validates

WHEN a task declares `max_turns: 12`
THEN it loads successfully.
WHEN a task declares `max_turns: 0`
THEN loading fails naming the task.

#### Scenario: readonly parses and validates

WHEN a task declares `readonly: true` and no `accept`
THEN it loads without the usual no-gate warning.
WHEN a task declares `readonly: true` and an `accept` command
THEN loading fails naming the task.
