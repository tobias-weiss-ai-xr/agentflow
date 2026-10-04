# Delta: config

## ADDED Requirements

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
