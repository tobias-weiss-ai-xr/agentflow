# sandbox Specification

## Purpose
TBD - created by archiving change agent-sandboxing. Update Purpose after archive.

## Requirements

### Requirement: Agent environment allowlist

The agent CLI child process SHALL run with an empty environment plus: a fixed
set of system keys (PATH, HOME, USERPROFILE, TEMP, TMP, SYSTEMROOT, WINDIR,
COMSPEC, APPDATA, LOCALAPPDATA, PROGRAMFILES), the dispatched worker's
`api_key_env` (if set in the parent environment), and any keys named in
`TF_AGENT_ENV_PASSTHROUGH` (comma-separated). Git invocations and acceptance
gates SHALL inherit the full parent environment (trusted code).

#### Scenario: foreign secrets are not leaked

GIVEN the orchestrator process has env `TEST_AF_LEAK=leak-456`
WHEN an agent runs for a worker whose `api_key_env` is `TEST_AF_KEY`
THEN the agent child sees `TEST_AF_KEY` but does NOT see `TEST_AF_LEAK`.

#### Scenario: passthrough escape hatch

GIVEN `TF_AGENT_ENV_PASSTHROUGH=TEST_AF_EXTRA`
WHEN an agent runs
THEN the agent child sees `TEST_AF_EXTRA`.

### Requirement: Git hygiene for agent children

Agent children SHALL receive `GIT_TERMINAL_PROMPT=0` and an empty
`credential.helper` (via `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_0`/
`GIT_CONFIG_VALUE_0`) so a confused agent cannot hang on credential prompts
or use stored helpers.

#### Scenario: git hygiene pairs present

WHEN the agent environment policy is built
THEN it contains `GIT_TERMINAL_PROMPT=0`, `GIT_CONFIG_COUNT=1`,
`GIT_CONFIG_KEY_0=credential.helper`, and `GIT_CONFIG_VALUE_0=` (empty).

### Requirement: Sandbox wrapper hook

`af` SHALL support `TF_SANDBOX_CMD`: a whitespace-split command prefix
prepended to the agent argv (e.g. `firejail --net=none`). When unset, the
agent argv is unchanged. `af` SHALL document that real filesystem/network
containment requires such a wrapper and give per-OS recipes.

#### Scenario: wrapper prepended

GIVEN `TF_SANDBOX_CMD="echo wrapped"`
WHEN an agent dispatches
THEN the child argv starts with `echo wrapped <agent-cli> …`.

#### Scenario: unset wrapper is a no-op

GIVEN `TF_SANDBOX_CMD` is unset
WHEN an agent dispatches
THEN the child argv starts with the agent CLI as before.
