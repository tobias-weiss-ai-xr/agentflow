---
file: specs/lifecycle/spec.md
action: add

location:
  after: Requirement: Gate and agent-tool shell selection
  under: Requirements

---@insert

### Requirement: Post-bash scope guard for builtin workers

For the native harness (`cli: "builtin"`), every `bash` tool call SHALL be
followed by a scope check that detects out-of-scope file changes (new,
modified, or untracked) by running `git status --porcelain -uall` in the
worktree, and MUST fail the tool call immediately with an `ERROR:` message
listing the violating path(s) when such changes are present and the task
has a non-empty scope. Git unavailable or not a repository MUST be ignored
silently so absent git does not block the tool call. This closes the
residual hole from the write/edit time guard (#3) where agents can still
bypass scope by writing through bash.

#### Scenario: in-scope bash write succeeds

GIVEN a task with `scope: ["src/**"]`
WHEN the agent runs `bash` with `echo 'fn x() {}' > src/new.rs`
THEN the tool call succeeds and returns the bash output.

#### Scenario: out-of-scope bash write fails immediately

GIVEN a task with `scope: ["src/**"]`
WHEN the agent runs `bash` with `echo data > out.txt`
THEN the tool call fails with an `ERROR: bash created out-of-scope files: out.txt` message.

#### Scenario: scope guard skipped when scope is empty

GIVEN a task with empty `scope: []`
WHEN the agent runs `bash` that writes to any path
THEN the tool call succeeds (no scope constraint).

#### Scenario: missing git in worktree does not block bash

GIVEN a task whose worktree is not a git repo or has no git command
WHEN the agent runs `bash` that writes to any path
THEN the tool call succeeds (no scope_probe can run).
