# config Delta — feedback-6-hardening

## ADDED Requirements

### Requirement: Status output names its state directory

The human `af status` board SHALL print the consulted state directory as its
first line (`state dir: <path>`), so an operator reading a board can tell
which campaign it describes. The `--json` output SHALL remain unchanged
(machine contract).

#### Scenario: human board names the consulted state dir

- Given a state directory different from the default `state/`
- When `af status` prints the human board
- Then the first line is `state dir: <absolute or as-configured path>`
- And the `--json` output does not gain a new top-level field

### Requirement: Task config hot-reload between dispatch rounds

`af run` SHALL re-read tasks.json/workers.json when their mtimes change,
so a corrected scope/accept definition reaches the next retry without a
process restart. On an unparseable or invalid reload, the loop SHALL keep
the last good config, warn once per broken generation, and continue.

#### Scenario: scope fix reaches the next retry without restart

- Given a running campaign whose task failed an attempt on a too-narrow scope
- When the operator edits the tasks file to widen the scope while `af run` runs
- Then the next dispatch round uses the widened scope (no restart needed)

#### Scenario: broken config file keeps the last good config

- Given a running campaign
- When the tasks file is edited into invalid JSON
- Then the loop keeps dispatching with the last good config and warns once
- And after the operator fixes the file, the next round loads the fix
