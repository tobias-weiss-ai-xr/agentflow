# Delta: lifecycle

## MODIFIED Requirements

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
