# Delta: lifecycle

## MODIFIED Requirements

### Requirement: Prompt rendering on retry

For attempt ≥ 2, the rendered prompt SHALL include a "Previous attempts"
block listing this task's earlier failed attempts (attempt number + error
line) so the agent avoids repeating them. Attempt 1 SHALL render without
the block.

#### Scenario: retry prompt names the earlier failure

GIVEN attempt 1 of task A failed with an agent exit code
WHEN attempt 2 renders its prompt
THEN the prompt contains a previous-attempts entry for attempt 1.

#### Scenario: first attempt has no history block

GIVEN a task dispatching its first attempt
WHEN the prompt renders
THEN no previous-attempts block appears.
