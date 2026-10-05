# Delta: scheduling

## ADDED Requirements

### Requirement: Scope enforcement on agent edits

After the agent exits successfully, `af` SHALL compute the attempt branch's
changed files (a three-dot diff against the base branch) and check every path
against the task's declared `scope`, using the same matcher as the scheduler's
contention check. A path that no scope entry allows SHALL fail the attempt
before the durable `AgentDone` boundary and before any merge. An empty scope
SHALL allow any file.

#### Scenario: out-of-scope edit fails before merge

WHEN the agent edits a file that no declared scope entry allows
THEN the attempt fails naming the offending path, the task is not merged, and the change never reaches the base branch.

#### Scenario: in-scope edit still merges

WHEN every changed file matches a declared scope entry
THEN scope enforcement passes and the attempt proceeds to the gate and merge.

#### Scenario: empty scope allows any file

WHEN a task declares no scope
THEN every changed file is accepted.
