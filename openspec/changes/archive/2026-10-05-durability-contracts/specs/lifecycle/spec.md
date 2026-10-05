# Delta: lifecycle

## ADDED Requirements

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
