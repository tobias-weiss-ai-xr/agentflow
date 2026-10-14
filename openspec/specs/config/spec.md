# config Specification

## Purpose
TBD - created by archiving change rust-orchestrator. Update Purpose after archive.

## Requirements

### Requirement: Task schema loading

`af` SHALL load a `tasks.json` file of the form `{ "_meta": {...}, "tasks": [...] }` into typed `Task` records. Each task SHALL support the fields `id`, `title`, `deps`, `scope`, `touch`, `accept`, `acceptance_prose`, `manual`, `repo`, `priority` (a number, or the levels `LOW`/`MEDIUM`/`HIGH`/`CRITICAL`). `touch` is an optional array of file paths the operator believes the agent must edit; absent or empty SHALL be the normal case with no behaviour change. Validation SHALL reject a `touch` entry that is covered by no `scope` entry — a hard error naming the task, the entry, and the scope — because such a task is unpassable by construction: it must edit a file its own scope forbids. Coverage SHALL be decided by the same matcher enforcement uses (`scheduler::scope_overlap`), so validation can never accept a config that enforcement will later reject; an empty `scope` means "any file" and covers every `touch` entry; a `touch` entry naming a file that does not exist yet SHALL be accepted (the task may create it). Validation SHALL reject: duplicate task ids and dependency cycles. It SHALL warn (without failing) on: `deps` referencing ids not present in the file (taskfleet composes configs across files), and tasks that are neither `manual` nor have an `accept` gate (the corpus runs these gate-less).

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

#### Scenario: touch entry not covered by scope is rejected

GIVEN a task declaring `touch: ["src/router.rs"]` whose `scope` omits any entry covering it
WHEN `tasks.json` is loaded
THEN loading fails with an error naming the task, the touch entry, and the scope entries (so the fix — widen `scope` or drop the entry — is obvious), while a fully covered `touch`, an empty `touch`, and an empty `scope` all load unchanged.

### Requirement: Worker schema loading

`af` SHALL load a `workers.json` file of the form `{ "defaults": {...}, "workers": [...] }`. Each worker SHALL support `name`, `provider`, `model`, `api_base`, `enabled`, and `args` — an array of extra agent-CLI arguments in which an absent field means an empty list. Validation SHALL reject duplicate worker names, a config with zero enabled workers, and any `args` entry that is an empty string (naming the worker). `args` entries SHALL be passed through to the agent CLI verbatim — agentflow never interprets or whitelists them (CLI-agnostic, ADR-1) — appended to the agent argv AFTER `--model` and BEFORE the `-p @file` prompt handoff, so a caller can reach CLI flags agentflow does not model (or override an earlier flag) while the prompt file always stays last.

Each worker SHALL also support `output` — the agent CLI's output mode: `"text"` (the default when the field is absent) or `"json"`. `"text"` keeps the legacy behaviour exactly: no extra argv entry, the raw stdout/stderr stream in the task log, and no token capture (`Receipt.tokens` stays `None`). `"json"` spawns the CLI with `--mode json` (placed with the built-in flags, before the worker's own `args` and never after the `-p @file` handoff), parses the JSON Lines transcript defensively (non-JSON lines and unknown event types are ignored; parsing never panics), takes the usage from the LAST event that carries one, writes a human-readable rendering of the transcript to the task log instead of raw JSON Lines, and records that total in the attempt receipt's `tokens`. A transcript that yields no usage at all (a crash, a truncation, a CLI that ignores the flag) SHALL leave the attempt otherwise unchanged with `tokens: None` — missing telemetry never fails an attempt. Any other `output` value SHALL be rejected at load time with an error naming the worker and the accepted values.

Each worker SHALL also support two optional cost-basis fields — operator DECLARATIONS used only when the provider reports no price (agentflow never guesses a model's size from its name; these are assumptions, not vendor data): `params_b` (model size in billions of parameters, a proxy for expense) and `price_per_mtok_usd` (real price in USD per million tokens; a declared price beats the `params_b` proxy). Both absent SHALL mean neutral (no opinion), so a workers.json that never mentions the fields loads unchanged. A declared value that is not finite or not strictly positive SHALL be rejected at load time with an error naming the worker and the field. An ENABLED worker declaring neither field SHALL produce exactly one warning (cost estimates will be neutral for it); a disabled worker SHALL NOT warn — it is never dispatched.

#### Scenario: valid workers load

WHEN a `workers.json` with two enabled workers is loaded
THEN both workers are parsed and dispatch can use them.

#### Scenario: zero enabled workers rejected

WHEN all workers have `enabled: false` or the worker list is empty
THEN loading fails with an error.

#### Scenario: Worker args are passed through to the agent CLI

GIVEN a worker with `args: ["--model", "from-worker-args"]`
WHEN a task dispatches on that worker
THEN the agent CLI receives those entries in its argv after the built-in `--model` and before `-p @file`, and a worker with no `args` dispatches exactly as before.

#### Scenario: Empty arg entries are rejected

GIVEN a worker whose `args` array contains an empty string
WHEN `workers.json` is loaded
THEN loading fails with an error naming the worker.

#### Scenario: output defaults to text

GIVEN a worker that omits `output`
WHEN it dispatches
THEN the agent argv carries no `--mode` entry, the raw agent output lands in the task log, and the attempt receipt records no token count (`tokens: None`).

#### Scenario: json output mode captures token usage

GIVEN a worker with `output: "json"` whose agent CLI emits a JSON Lines transcript whose final events carry a usage object
WHEN a task dispatches on that worker and the attempt completes
THEN the attempt receipt's `tokens` carries the transcript's final `totalTokens`, and the task log contains the rendered assistant text (with other activity compact) and no raw JSON Lines.

#### Scenario: unknown output value is rejected

GIVEN a worker declaring `output: "jsn"`
WHEN `workers.json` is loaded
THEN loading fails with an error naming the worker and the accepted values (`text`, `json`).

#### Scenario: cost basis is declared and optional

GIVEN a workers.json where one worker declares `params_b` and `price_per_mtok_usd` and another declares neither
WHEN the file is loaded
THEN the declared values survive loading, the undeclared worker stays neutral (both fields `None`), and a workers.json that never mentions the fields loads unchanged.

#### Scenario: unusable cost basis values are rejected

GIVEN a worker declaring `params_b: 0` or a negative `price_per_mtok_usd`
WHEN `workers.json` is loaded
THEN loading fails with an error naming the worker and the field.

#### Scenario: missing cost basis warns

GIVEN an enabled worker that declares neither `params_b` nor `price_per_mtok_usd`
WHEN `workers.json` is loaded
THEN loading succeeds with exactly one warning naming that worker, and a worker that declares either field (or is disabled) produces no such warning.

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

### Requirement: Per-task turn cap and readonly task fields

`af` SHALL support two optional Task fields beyond the schema documented
above.

- `max_turns` — an integer >= 1 capping the number of LLM round-trips for a
  `cli: "builtin"` attempt of THIS task. It SHALL override the worker's
  `max_turns`, then `TF_AGENT_MAX_TURNS`, then the harness default of 32, only
  for this task. Validation SHALL reject `max_turns: 0` or negative with a
  hard error naming the task. Absent = no per-task override.
- `readonly` — a boolean (default `false`). When `true` the task is
  investigation-only: `af` SHALL complete the attempt when the agent stops
  normally, with no acceptance gate and no merge, and the builtin harness
  SHALL reject every `write` and `edit` tool call. Validation SHALL reject
  `readonly: true` combined with an `accept` gate — a gate on a read-only task
  is contradictory — with a hard error naming the task. Absent = normal task
  behavior unchanged.

#### Scenario: per-task max_turns parses and validates

WHEN a task declares `max_turns: 12`
THEN it loads successfully.
WHEN a task declares `max_turns: 0`
THEN loading fails naming the task.

#### Scenario: readonly parses and validates

WHEN a task declares `readonly: true` and no `accept`
THEN it loads without the usual no-gate warning.
WHEN a task declares `readonly: true` and an `accept` command
THEN loading fails naming the task.

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
