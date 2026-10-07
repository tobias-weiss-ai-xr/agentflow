# agentflow

**Parallel LLM task execution on isolated git worktrees.**

agentflow is a small Rust orchestrator that dispatches declarative tasks to
multiple LLM providers in parallel (via an OpenAI-compatible **agent CLI**),
each running in an isolated git worktree. Tasks are verified against exact
acceptance gates before being merged to the base branch.

```
 tasks.json (N tasks, declarative)  ─┐
 workers.json (M providers)          ├──► af ──► per-task:
                                      │         worktree → agent CLI → gate → merge
 env (TF_*)                          ─┘
```

The orchestrator stays thin (ADR-1): it drives `git` and an OpenAI-compatible
agent CLI (`pi`, opencode, …) as subprocesses. It never talks to LLM
providers directly.

## Features

- **Parallel dispatch** — one worker per provider+model slot; concurrent tasks in isolated git worktrees
- **Multi-repo** — one campaign can touch several repositories: `repos.json` maps names to paths, each task's worktree/branch/merge targets its own repo, deps order across repos
- **Measured routing** — every attempt leaves a receipt with its outcome; free workers are picked by UCB1 (track record + exploration), per-worker trust shown in `af cost`; ties among equally trusted workers go to the cheaper declared basis, then to the faster measured mean, and only then to config order
- **Retry memory** — failed attempts record their error; retry prompts list the task's earlier failures so the agent doesn't repeat them
- **Exact acceptance gates** — each task declares a shell command that must exit 0 before merge
- **Dependency DAG** — `deps` ordering, critical-path priority, deadlock detection
- **Contention avoidance** — tasks with overlapping `scope` globs are not dispatched concurrently
- **Retry** — `max_attempts` per task, fresh branch + worktree on every attempt
- **Self-healing** — atomic JSON state; crash-safe resume; orphan worktree cleanup at startup, plus `af clean [--dry-run]` to sweep leftovers from crashed runs — including the archived `<branch>-rejected-*` refs, whose branches are kept while their task is still running
- **Validated config** — `af validate` pre-flights tasks/workers (dependency cycles, duplicate ids, no enabled workers) without dispatching anything
- **Observable** — status board (`af status [--json]`), live `attach`, per-task logs, wall-clock cost receipts
- **Sound by construction** — spec → contract → test pyramid (313 tests, incl. E2E against a stub agent + scratch git repos; no network in CI)

## Quick start

```sh
cargo build --release            # produces target/release/af

cp config/tasks.json.example config/tasks.json
cp config/workers.json.example config/workers.json
# edit workers.json: provider/model/api_base + your agent CLI (default: pi)

export TF_REPO_DIR=/path/to/repo/being/modified
export TF_STATE_DIR=state

./target/release/af run --dry-run   # show the dispatch plan, change nothing
./target/release/af run             # run until all tasks done or deadlock
./target/release/af status          # task status board
```

## CLI

```
af run [--once] [--dry-run] [--worker NAME] [--task ID] [--poll SECS]
af status [--json]
af api status [--json] | results --task ID
af attach ID
af cost [--task ID] [--last] [--since DATE|UNIX_TS]
af clean [--dry-run]
af recover --task ID [--dry-run]
af validate [--worker NAME]
```

### Exit codes

| Code | Meaning |
|------|---------|
| `0` | Done — every in-scope task reached `done` (or `--dry-run`/`--once` finished its work) |
| `2` | Configuration error, unknown flag/command, or **deadlock** (no task can make progress) |
| `3` | Stopped early — the wall-clock budget was exhausted before every in-scope task could be started |

### Campaign budget (`TF_MAX_WALL_CLOCK_S`)

A campaign can be given a spend ceiling in seconds. `TF_MAX_WALL_CLOCK_S`
(default `0`) is compared against the sum of `wall_clock_s` over the
receipts for the in-scope tasks — the exact TOTAL `af cost` reports, so the
ceiling is enforced against the same ledger the report shows (receipts are
persisted, so the cap survives restarts and counts across runs).

- `0` = **unlimited**, the legacy behaviour — exactly as before.
- When the measured spend is **at or above** the cap, `af run` stops
  dispatching **new** attempts; attempts already in flight are allowed to
  finish (a running attempt is never killed). It prints one diagnostic
  naming the spend, the cap, and how many in-scope tasks were not started,
  then exits `3`.
- **Completion wins**: a campaign whose in-scope tasks are all already
  `done` exits `0` even when the measured spend exceeds the cap.
- `af run --dry-run` prints the same budget line when the spend is already
  over the cap, still creating nothing and still exiting `0`.

`af cost` aggregates the receipts every attempt appends to `state/receipts/`.
Plain `af cost` totals every attempt; `--last` narrows to the most recent
receipt per task (the greatest `ts`, ties broken by the greater attempt
number) so retries are not double-counted; `--since DATE|UNIX_TS` keeps only
receipts with `ts >=` the instant — `DATE` is `YYYY-MM-DD` (UTC midnight) or
a bare unix timestamp, applied before `--last` so `--last --since D` is
"the latest attempt per task since D". The window flags compose with each
other and with `--task ID` (which narrows the table rows); the TOTAL line
and the per-worker trust block are always computed over the selected
receipts only.

The table also carries a `TOKENS` column: it sums the `tokens` recorded on
the selected receipts and shows `-` when they carry none, so a report over
legacy receipts reads exactly as it did before the column existed. In JSON
output mode the receipt is the cost ledger, so a FAILED attempt records the
tokens it spent too — a scope violation, a gate failure or a merge conflict
happens after the agent has already run, and those are the most expensive
failures of all.

Both tables also carry a `COST` column — an estimate of expense derived
from the basis each worker DECLARES (see `params_b` / `price_per_mtok_usd`
in the worker schema below), so the report answers "who is burning my
budget?" even when no provider reports a price. A worker with a declared
price shows real dollars (`$` + four decimals, computed from its recorded
tokens — summed over a task row's attempts); a worker with only `params_b`
shows its relative RATE (`1.25x` — a task row spanning several workers
shows the token-weighted mean); and `-` appears when the worker declares
NEITHER basis, or when the receipt names a worker that is no longer in
`workers.json`. A declared basis is never blanked by missing token data:
the default output mode records no tokens, so a declaring worker shows its
rate (`1.25x`) or its declared price (`$0.6000/Mtok`, unit-suffixed so a
rate is never misread as a spend) rather than `-`. One `cost basis:` line
above the tables states the basis in force — declared prices, the
`params_b` proxy (relative; cheapest declared worker = `1.00x`), or none —
so a proxy can never be misread as money. Receipts naming workers absent
from the config are listed in one `note:` footnote after the tables
(sorted, deduplicated
names with the attempt count): historical receipts routinely outlive
config edits, so this is a note — never an error.

When the agent CLI reports what an attempt actually COST, that measurement is
recorded on the receipt in integer micro-USD (`cost_micros`) and OUTRANKS any
declared basis for the attempt it belongs to. Only a strictly positive report
counts as a measurement: every provider tested here reports `0`, which is
indistinguishable from "not tracked", so a reported zero is recorded as
unknown — never as a measured free run. On a report where any selected receipt
carries a measurement the basis line names the measured source (`cost basis:
provider-reported (USD)`, appending `; N of M attempt(s) estimated from a
declared price`), and a row mixing a measured attempt with an estimated one is
marked `~` (`~$0.0123` — summed dollars with at least one term an assumption),
so an assumption is never presented as a measurement. The row invariants still
hold: a measured+sized row stays `-`, because a dollar amount and a parameter
ratio are incommensurable. A report none of whose receipts carries a
measurement keeps the declared-basis output byte-for-byte, so nothing about a
no-telemetry provider changes.

The report also ends with a waste section: `WASTED: <seconds>s on <failed>
of <total> attempt(s) (<pct>%)`, followed by a `WASTED BY REASON` breakdown
that groups failed attempts by the CAUSE — the text before the first `:` in
the `error` field, trimmed (and `unknown` when the receipt has no error).
Grouping by cause keeps two failures with the same cause but different file
lists in one row, and never truncates the key mid-word; the distinct full
reasons remain visible as indented sub-counts. Like the TOTAL line and the
trust block, the waste figures are computed over the same window-selected
receipts, so `--last` / `--since` narrow the waste alongside the rest of the
report; a window whose receipts contain no failures reports `0.0s` and omits
the reason breakdown.

A failed attempt's committed work is never destroyed. A GATE failure keeps its
branch and the retry re-runs only the gate; a SCOPE VIOLATION or a MERGE
CONFLICT archives the branch as `<branch>-rejected-<unix-ts>` before cleanup
and names it in the receipt's error, so work an agent was already paid for
stays recoverable — while the original branch is still deleted, so the retry
starts clean off the current base. An attempt lost to a killed orchestrator is
recorded as `interrupted`, and because the worker and the start time are
persisted at dispatch the receipt names that worker (and its model) and
reports `wall_clock_s` as an upper bound — time since dispatch, since the exit
time is unknown. `interrupted` is not a verdict, so it never lowers trust.

`af recover --task ID` puts that archived work back to use: it re-checks the
archived branch against the task's CURRENT scope and re-runs its acceptance
gate, then merges it on success — without re-invoking the agent. This is what
makes an operator's over-narrow scope cost one re-validation instead of a
re-run. It selects the newest archived branch by parsing the timestamp (never
by git's ordering), exits 2 when there is nothing to recover (or no such task),
and exits 1 when the work still fails its re-check, keeping the branch. The
failed attempt's receipt is left untouched: recovery does not rewrite history,
it reuses it. `--dry-run` reports what it would do and changes nothing.

A receipt file that cannot be parsed (a torn write from an interrupted
campaign) is reported by name with one `warning:` line — `af cost` and
`af status` load receipts through the checked loader — and is never allowed
to block the command: the readable history is still accounted for.

## Task schema (`config/tasks.json`)

| Field | Description |
|-------|-------------|
| `id` | Unique task id (branch names, status keys) |
| `title` | Human-readable description (injected into the prompt) |
| `deps` | Task ids that must reach `done` first |
| `scope` | File globs the task may modify (contention + advisory) |
| `touch` | Optional declaration of the files you believe the task MUST edit; `af validate` rejects the config when one is covered by no `scope` entry (that task cannot pass) |
| `accept` | Shell command run in the task's worktree; exit 0 = pass |
| `acceptance_prose` | Natural-language success criteria (injected into the prompt) |
| `manual` | Skip the acceptance gate (manual sign-off) |
| `priority` | Tie-breaker when multiple tasks are ready |
| `gate_replay` | Whether the gate is replay-safe (default `true`); `false` = side effects |

Replay contract: the `accept` command MUST be idempotent; declare
`gate_replay: false` for a gate with side effects — an interrupted such gate
is settled as failed instead of re-run. The gate process receives the
decision in `TF_GATE_REPLAY` (`1`/`0`).

## Worker schema (`config/workers.json`)

`defaults` (`accept_timeout_s`, `max_attempts`, `retry_delay_s`, `agent_timeout_s`)
plus a `workers` list: `name`, `provider`, `model`, `api_base`, `api_key_env`,
`enabled`, `cli` (agent CLI binary; default `pi`), and the optional
cost-basis declarations `params_b` / `price_per_mtok_usd` (below). Each
worker runs at most one task at a time.

`retry_delay_s` (default `0`, strictly opt-in) is the backoff between
attempts of the SAME task: after a failed attempt that will be retried,
the task waits this many seconds before its next attempt starts. The wait
rides the retry path only — first attempts, tasks that merge on the first
try, and every other worker's dispatches are never delayed (each paced
retry is announced in the run log). It mainly helps against provider rate
limits; since the agent run dominates a campaign's cost, an unexplained
pause per retry is otherwise pure added wall-clock, so `0` (no delay) is
the default and a campaign that wants backoff sets it explicitly.

Tasks accept an optional `repo` field: `""` (default) or `"main"` target the
main repo (`TF_REPO_DIR`); other names must appear in `repos.json` (next to
tasks.json or via `--repos`/`TF_REPOS_JSON`) — e.g.
`{"repos": {"docs": "../docs-site"}}`. Unknown names warn and fall back.

Two optional fields let a campaign DECLARE how expensive each worker is —
they are the operator's declaration, used only when the provider reports no
price (agentflow never guesses a model's size from its name):

- `params_b` — model size in billions of parameters, a proxy for expense.
- `price_per_mtok_usd` — real price in USD per million tokens, when the
  operator knows it; a declared price beats the `params_b` proxy.

Both absent = neutral (no opinion), so an existing workers.json keeps
working unchanged. A declared value must be finite and positive — otherwise
loading fails with an error naming the worker and the field — and an
enabled worker declaring neither field gets a one-line warning at load time
(cost estimates will be neutral for it); a disabled worker never warns.

## Sandboxing

The agent CLI executes LLM-directed tool calls, so `af` treats it as untrusted
(ADR-10):

- **Env allowlist** — the agent child sees only system basics, the dispatched
  worker's `api_key_env`, and `TF_AGENT_ENV_PASSTHROUGH` (comma-separated
  extras). Other workers' API keys and your shell secrets are withheld.
- **Git hygiene** — `GIT_TERMINAL_PROMPT=0`, credential helpers disabled:
  no credential popups, no hangs.
- **Wrapper seam** — `TF_SANDBOX_CMD="firejail --net=none"` (or bwrap /
  sandbox-exec / a container) is prepended to the agent argv for real
  filesystem/network containment. Env hygiene alone is not a sandbox —
  see `docs/arc42/08-concepts.md` §8.8 for threat model and recipes.

## Agent CLI compatibility

`af` invokes your agent CLI as:

```
<cli> --provider <name> --model <model> -p @<prompt-file>
```

Any OpenAI-compatible CLI supporting that shape works. `src/bin/example_agent.rs`
is a stub agent (writes a file + commits) used by the test suite and CI.

## Dogfooding / parallel workers

agentflow is built to be run *by* agents, and the fastest way to exercise it
is to dispatch several tasks at once — often across more than one repository —
to two or more workers. Here is a full two-repo / two-worker campaign.

### `config/repos.json`

```json
{
  "repos": {
    "main": "/abs/path/to/agentflow",
    "docs":  "/abs/path/to/docs-site"
  }
}
```

### `config/tasks.json`

```json
{
  "_meta": { "project": "dogfood" },
  "tasks": [
    {
      "id": "fix-readme",
      "title": "Document the --repos flag in the README",
      "repo": "main",
      "deps": [],
      "scope": ["README.md"],
      "accept": "grep -q -- '--repos' README.md",
      "acceptance_prose": "README documents the --repos flag.",
      "manual": false
    },
    {
      "id": "fix-docs-index",
      "title": "Refresh the docs site index page",
      "repo": "docs",
      "deps": [],
      "scope": ["index.md"],
      "accept": "git diff --stat --exit-code",
      "acceptance_prose": "Docs index is updated.",
      "manual": false
    }
  ]
}
```

### `config/workers.json`

```json
{
  "defaults": {
    "accept_timeout_s": 600,
    "max_attempts": 3,
    "retry_delay_s": 0,
    "agent_timeout_s": 3600
  },
  "workers": [
    { "name": "opus",  "provider": "anthropic", "model": "claude-opus-4", "api_base": "https://api.anthropic.com/v1", "api_key_env": "ANTHROPIC_API_KEY", "enabled": true, "cli": "pi" },
    { "name": "gpt4o", "provider": "openai",    "model": "gpt-4o",        "api_base": "https://api.openai.com/v1",      "api_key_env": "OPENAI_API_KEY",    "enabled": true, "cli": "pi" }
  ]
}
```

### Invocation

```sh
export TF_REPO_DIR=/abs/path/to/agentflow
export TF_STATE_DIR=state

./target/release/af run --dry-run \
    --tasks config/tasks.json --workers config/workers.json --repos config/repos.json
./target/release/af run \
    --tasks config/tasks.json --workers config/workers.json --repos config/repos.json
./target/release/af status
./target/release/af cost
```

`fix-readme` targets repo `main` and `fix-docs-index` targets repo `docs`; they
have no `deps` between them, so the scheduler treats both as ready at the same
time. With two idle workers the two tasks dispatch **in parallel** — one in an
isolated git worktree on `main`, the other on `docs` — each later merging into
its own repo. Swap in `--task`/`--worker` to pin a single task or worker.

### How trust routing picks workers

Every attempt leaves a **receipt** (worker, model, wall-clock time, outcome).
On dispatch, the router replays receipts into per-worker `(wins, attempts)`
stats and, among the currently free workers, picks by **UCB1**:

```
score(worker) =  mean(worker)  +  sqrt( 2 * ln(N + 1) / (n + 1) )
                └─ exploitation ┘   └─ exploration bonus ┘
```

- `mean` = `wins ÷ attempts` — the worker's measured trust rate.
- `N` = total attempts across all workers; `n` = this worker's attempts.

A tie between equally scoring workers is broken in a fixed order: the cheaper
DECLARED basis (`params_b` / `price_per_mtok_usd`) first, then the strictly
faster measured mean wall-clock (`MEAN_S` in `af cost`, computed over verdict
attempts only — an `interrupted` duration is a placeholder, not a
measurement), then **config order**. A strictly higher score never loses a
tie-break, so measured trust always outranks both. Duration is deliberately
only a tie-break: wall-clock is confounded by task difficulty (the hard tasks
go to the trusted worker), so making it part of the score would penalise a
worker for being given the hard work and starve it.

With no receipts yet, every worker scores `0` (a tie), broken in **config
order** — the first configured worker (`opus`) takes the first ripe task and
`gpt4o` takes the second, so both run concurrently. As receipts accumulate, a
worker that keeps failing lowers its `mean`, while the exploration term gives
an under-tried (or untried) worker the chance to be routed past it. `af cost`
shows each worker's live trust rate — and the `MEAN_S` the tie-break reads —
so you can watch routing adapt between campaigns.

## Testing

```sh
cargo test        # 313 tests: unit (scheduler DAG, contention, deadlock, state,
                  # receipts, config validation, sandbox policy, multi-repo,
                  # UCB1 router, retry context, subprocess contracts) + E2E
                  # (fake agent + scratch git repos — no network)
```

See `docs/arc42/` for the architecture documentation (12 chapters), and
`openspec/changes/` for the spec-driven change history (spec → contract → test).

## Coverage ratchet

CI enforces a **line-coverage floor** so coverage cannot silently rot. The
floor lives in a single place — `.coverage-min` (currently `94%`, just below
the measured 94.19% baseline) — and is checked by `scripts/coverage-gate.sh`, which runs
`cargo llvm-cov --workspace --fail-under-lines "$MIN"`. A drop below the floor
fails the build.

Run it locally (requires `cargo-llvm-cov` + `llvm-tools-preview`):

```sh
./scripts/coverage-gate.sh   # enforces the floor
```

The threshold is a **ratchet: it may only be RAISED**, never lowered to make a
red build green. When new untested code legitimately drops coverage, add tests
for it; only as an explicit, reviewed last resort is the floor itself changed.
See [`docs/coverage.md`](docs/coverage.md) for the full policy.

## License

MIT
