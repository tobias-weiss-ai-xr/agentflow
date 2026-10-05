# config Specification

## Purpose
TBD - created by archiving change rust-orchestrator. Update Purpose after archive.

## Requirements

### Requirement: Task schema loading

`af` SHALL load a `tasks.json` file of the form `{ "_meta": {...}, "tasks": [...] }` into typed `Task` records. Each task SHALL support the fields `id`, `title`, `deps`, `scope`, `accept`, `acceptance_prose`, `manual`, `repo`, `priority` (a number, or the levels `LOW`/`MEDIUM`/`HIGH`/`CRITICAL`). Validation SHALL reject: duplicate task ids and dependency cycles. It SHALL warn (without failing) on: `deps` referencing ids not present in the file (taskfleet composes configs across files), and tasks that are neither `manual` nor have an `accept` gate (the corpus runs these gate-less).

#### Scenario: valid config loads

WHEN a `tasks.json` with two tasks and one dependency is loaded
THEN both tasks are parsed and the dependency graph resolves without error.

#### Scenario: duplicate id rejected

WHEN a `tasks.json` contains two tasks with the same `id`
THEN loading fails with an error naming the duplicate id.

#### Scenario: dependency cycle rejected

WHEN a task's `deps` (transitively) reference the task itself
THEN loading fails with an error naming the cycle.

#### Scenario: dangling dependency warns

WHEN a task's `deps` references an id that does not exist in the file
THEN loading succeeds and emits a warning naming the missing dep.

#### Scenario: task without gate and not manual warns

WHEN a task has neither `accept` nor `manual: true`
THEN loading succeeds, warns, and the gate is skipped at run time.

#### Scenario: string priority levels accepted

WHEN a task declares `priority: "HIGH"` and another `priority: 3`
THEN both parse and rank deterministically (CRITICAL > HIGH > number-ranked).

### Requirement: Worker schema loading

`af` SHALL load a `workers.json` file of the form `{ "defaults": {...}, "workers": [...] }`. Each worker SHALL support `name`, `provider`, `model`, `api_base`, `enabled`. Validation SHALL reject duplicate worker names and a config with zero enabled workers.

#### Scenario: valid workers load

WHEN a `workers.json` with two enabled workers is loaded
THEN both workers are parsed and dispatch can use them.

#### Scenario: zero enabled workers rejected

WHEN all workers have `enabled: false` or the worker list is empty
THEN loading fails with an error.

### Requirement: Environment overrides

`af` SHALL honor the environment variables `TF_REPO_DIR`, `TF_STATE_DIR`, `TF_MAX_PARALLEL`, `TF_BRANCH_PREFIX`, `TF_POLL`, `TF_GATE_ENV` with documented defaults, matching taskfleet semantics for drop-in compatibility.

#### Scenario: overrides applied

WHEN `TF_MAX_PARALLEL=2` and `TF_POLL=5` are set in the environment
THEN the scheduler uses max 2 parallel tasks and a 5s poll interval.

#### Scenario: defaults when unset

WHEN no `TF_*` variables are set
THEN defaults are used (max parallel = number of enabled workers, poll = 15s).

### Requirement: Optional repos.json defines named repositories

`af run` SHALL look for `repos.json` next to the tasks file, overridable by
`TF_REPOS_JSON` and `--repos FILE`. The file shape is
`{"repos": {"<name>": "<path>"}}`; relative paths SHALL resolve against the
repos.json file's directory. A missing file SHALL mean single-repo mode
(every task targets `TF_REPO_DIR`).

#### Scenario: missing repos.json is single-repo mode

GIVEN no repos.json exists
WHEN a config loads
THEN no repositories are registered and every task uses `TF_REPO_DIR`.

#### Scenario: relative repo paths resolve against the file

GIVEN repos.json in `config/` containing `"docs": "../docs-site"`
WHEN it loads
THEN the `docs` repo resolves to `<config-dir>/../docs-site`.

### Requirement: Per-task repo resolution with compat fallback

A task's `repo` field SHALL resolve as: `""` → `TF_REPO_DIR`; `"main"` →
`repos["main"]` if present, else `TF_REPO_DIR`; any other name →
`repos[name]`. A name not present in repos.json SHALL produce a warning and
fall back to `TF_REPO_DIR` (ADR-4: corpus compatibility — never hard-fail
planning on unresolvable repo names).

#### Scenario: empty repo means the default repo

GIVEN a task with no `repo` field
WHEN it dispatches
THEN its worktree, branch, and merge live in `TF_REPO_DIR`.

#### Scenario: main falls back to the default repo

GIVEN no repos.json and a task with `"repo": "main"`
WHEN it dispatches
THEN it uses `TF_REPO_DIR` (single-repo configs keep working).

#### Scenario: unknown repo warns and falls back

GIVEN a task with `"repo": "docs"` and no `docs` entry in repos.json
WHEN the run starts
THEN a warning is printed and the task dispatches against `TF_REPO_DIR`.
