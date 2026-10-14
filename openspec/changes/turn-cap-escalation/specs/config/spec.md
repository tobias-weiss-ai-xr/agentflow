# Capability: config

## ADDED Requirements

### Requirement: Gate/scope mismatch warning at validate time

`af validate` SHALL print a non-fatal warning for a task whose `accept`
command contains the marker `--workspace` while the task's `scope` is
non-empty and every scope entry lacks a leading `**` wildcard. The warning
SHALL name the task id and state that out-of-scope diagnostics can fail a
workspace-wide gate forever.

#### Scenario: Narrow scope with workspace gate warns

- **WHEN** a task declares `scope: ["core/crates/wo-wopi/**"]` and
  `accept` containing `cargo clippy --workspace --all-targets`
- **THEN** `af validate` emits a warning mentioning the task id and the
  gate/scope mismatch, and validation still passes

#### Scenario: Unscoped tasks never warn

- **WHEN** a task declares an empty `scope` and a workspace-wide `accept`
- **THEN** no mismatch warning is emitted
