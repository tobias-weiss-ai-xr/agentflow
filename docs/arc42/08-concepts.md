# 8. Cross-cutting Concepts

## 8.1 Effect Sandwich (pi-durable)

Every attempt is modeled as an effect sandwich with durable checkpoints so a
crashed orchestrator resumes the attempt without re-running the expensive,
non-replayable agent step:

```
commit intent: AttemptPhase::Spawned        → resume = RerunAgent
    ↓
   AGENT (subprocess, non-replayable)
    ↓
commit outcome: AttemptPhase::AgentDone     → resume = RerunGate
    ↓
   GATE (idempotent)
    ↓  
commit outcome: AttemptPhase::GatePassed    → resume = MergeOnly
    ↓
   MERGE (idempotent)
```

- **Spawned**: worktree created, agent about to spawn. Crash resume →
  `RerunAgent` (always safe: no durable agent outcome yet).
- **AgentDone**: agent exited 0, change committed on attempt branch. Crash
  resume → `RerunGate` (skip agent, the work is already durable).
- **GatePassed**: gate passed. Crash resume → `MergeOnly` (idempotent merge
  step only).

The phase is persisted in `TaskStatus.phase` BEFORE each boundary so a killed
orchestrator's stale `Running` state can be healed by finishing only the
uncommitted effects.

## 8.2 Trust Model (ADR-12, ADR-13)

### 8.2.1 Measured routing

Worker selection uses **UCB1** (Upper Confidence Bound 1):

```
score = mean_reward + exploration_term
  where mean_reward = wins / total
        exploration_term = √(2 * ln(N_total + 1) / (n_worker + 1))
```

`N_total` = sum of all attempts recorded. The exploration term guarantees every
worker gets an attempt eventually; a strictly higher mean always displaces the
incumbent unconditionally (measured reliability outranks every assumed
heuristic).

### 8.2.2 Tie-breaking rules

On a score tie within `SCORE_TIE_EPSILON` (1e-9):
1. **Declared cost** — cheaper basis wins (via `cost::compare`; only when an
   ordering exists; incomparable bases fall through)
2. **Mean duration** — strictly lower mean duration wins (missing duration
   never swaps; treats `None` as "no history", not zero)
3. **Config order** — the worker that appears first in `workers.json` wins

Ordering principle: **measured trust > declared cost > measured duration**.
Duration is a tie-break only, never a score term, because wall-clock is
confounded by task difficulty and would penalize trusted workers given hard
tasks.

### 8.2.3 Verdict vs non-verdict outcomes

Only `"merged"` and `"failed"` outcomes count as **verdicts** on worker
reliability. Excluded from all trust calculations:
- `"interrupted"` — attempt lost when orchestrator died; no agent result
- `"recovered"` — pre-dispatch reuse merged work that was already paid for;
  no agent ran

`Receipt::counts_as_verdict()` encodes this; `af cost` excludes both from its
WINS/TOTAL denominator and trust percentage.


## 8.3 Scope Enforcement

### 8.3.1 glob matching

Scope entries are file globs. `scope_overlap(a, b)` (pure, no regex) determines
if any two patterns overlap:
- Exact match → overlap
- Prefix match on the literal part before any `*`/`?` → overlap
- Example: `src/*.rs` overlaps `src/lib.rs`, `src/main.rs`; does NOT overlap
  `tests/*.rs`

### 8.3.2 Checkpoints

At gate time, `changed_paths` = `git diff --name-only <base>...<branch>` 
(committed changes only). `scope_violations` checks every changed path against
the task's `scope` globs. Non-empty violations → `Failed` with the file list.

An agent that edits but never commits is now **caught**: T1 of round 13
commitsthe dirty worktree BEFORE judging, so scope check, gate, and merge all
judge exactly the same committed tree.

### 8.3.3 The `touch` contract (ADR-12)

Optional `Task.touch: Vec<String>` declares which files the task intends to
edit. `config::validate` rejects a task whose `touch` entry is not covered by
any `scope` entry at **config time** — a safety net for the common mistake
of widening the prompt but forgetting to widen the scope.

`touch` is advisory; the gate is the legal contract. A task with `touch`
missing a file that the agent changes still fails gate if the file is out of
scope.

## 8.4 Honest Success (round 13)

A `Merged` outcome **must mean the work is in the base branch**. The harness:

1. Commits a dirty worktree before judging
2. Fails an attempt that produces **0 commits ahead** of base
   (`"agent produced no change"`)
3. Verifies `branch_merged_into_head` (tip is ancestor of HEAD) before
   reporting `Merged`

These three checks close:
- The **false green** (dirty file judges green, merge carries nothing)
- The **scope-enforcement bypass** (uncommitted out-of-scope edit invisible
  to scope check)
- The **zero-change false success** (empty task needs `manual: true`)

## 8.5 Work Preservation

On **every failure path** the attempt's committed work is preserved in a branch
named `<branch>-rejected-[<attempt>-]<unix-ts>[-<n>]`:

- **11 arms** in `execute::execute_attempt` call `preserve_work` / `preserve_and_note`
- `worktree::heal` at startup archives before removing stale worktrees
- Archive name encodes: task id (prefix), attempt number, unix timestamp,
  optional collision suffix
- Legacy 3-field format (`<prefix>/<id>-rejected-<unix-ts>`) is backward
  compatible via parser disambiguation (field ≥ 1_000_000_000 = timestamp)

`af clean [--dry-run]` removes archived branches matching the `<prefix>`;
`af recover --task ID [--attempt N] [--dry-run]` selectively re-validates
and merges them.

## 8.6 Recovery Ledger

When `af recover` or `af run`'s auto-reuse merges an archived branch:

1. A `recovered` receipt is appended (outcome `"recovered"`,
   `wall_clock_s: 0.0`, records the (task, attempt) of the failed receipt
   it reclaims)
2. `af cost` pairs each `recovered` with the corresponding `failed` receipt
   by `(task, attempt)`
3. The `RECOVERED:` line shows the count and reclaimed seconds (sum of
   paired failed receipts' wall-clock)
4. Those failed receipts are **excluded from WASTED** — they were paid for
   once, merged, and are not lost

This is bookkeeping: recovery is NOT a new attempt (no agent ran, no worker
slots).

## 8.7 Cost Model

### 8.7.1 `Basis` enum

A worker declares its expense metric explicitly; `f64` is incommensurable across
models:

```rust
pub enum Basis {
    Priced(f64),    // $ per million tokens
    Sized(f64),     // parameter count in billions (size proxy)
}
```

**No conversion.** `compare(a, b)` returns `None` when variants differ; never
invents a conversion rate between dollars and parameters.

### 8.7.2 Truthfulness ladder

For each receipt, expense is folded most-truthful-first:

1. **Measured** — `cost_micros: Some(u64)`. Provider's own report in
   micro-USD. Outranks every declared basis for its attempt.
2. **Estimated dollars** — declared price × recorded tokens. Minimum
   computable assumption.
3. **Ratio** — declared `params_b` relative to cheapest `Sized` basis.
   Proportional proxy, never dollars, incommensurable with measured.
4. **Unknown** — no declared basis, no provider report. Rendered as `-`; not
   zero, not invented.

The report rows show truthfulness: measured receipts in a task row yield
`$X.XXXX` cells; estimated rows show `~$X.XXXX`; purely relative rows show
`Y.XXx`.

### 8.7.3 Provider limitation

Every real provider tested reports `usage.cost.total: 0` (confirmed rounds
9–14). The measured path is therefore exercised only via the stub agent's
`FAKE_AGENT_JSON_COST` knob. Real campaigns show `-` for cost until providers
ship non-zero amounts.

## 8.8 Sandbox (ADR-10)

The agent child is **untrusted**. Three layers:

1. **Env allowlist**: empty environment + system basics + `api_key_env`
   for the dispatched worker only + whitelisted vars from
   `TF_AGENT_ENV_PASSTHROUGH`. Other workers' keys and orchestrator secrets
   are withheld.
2. **Git hygiene**: `GIT_TERMINAL_PROMPT=0`; `credential.helper=""` to
   prevent auth hangs/theft.
3. **Wrapper seam**: `TF_SANDBOX_CMD` prepends a user-provided wrapper
   command (e.g. `firejail`, `bubblewrap`) to the agent argv. Real
   filesystem/network containment is delegated to this layer.

`af` itself does NOT provide OS-level sandboxing and remains dependency-free.

## 8.9 Multi-repo (ADR-11)

`repos.json` maps task ids or patterns to repository roots. Per-task resolution:
- If `task.repo` is set, use that repo
- Else fall back to `TF_REPO_DIR` (or `repo_dir` in settings)

Deps stay a **global DAG** across all repos. Worktree/branch/merge target the
resolved repo. Merges are globally serialized (one `MergeLocks` map per
process) — correct for any repo count.

Multi-repo transactional merges are a v2 candidate (see
`docs/multi-repo-design.md`).

## 8.10 Atomic persistence (ADR-3)

- **Single writer process**: `state/state.lock` records pid; dead pid →
  reclaim lock
- **Atomic writes**: temp file (`<name>.tmp-<pid>-<seq>`) → fsync → rename
  over final path; temp removed on failure
- **Append-only receipts**: one receipt per attempt; unique temp filename →
  fsync → rename ensures no two receipts overwrite
- A torn state file is reported at load time; `af run` refuses to start
  until it is fixed; a torn receipt is reported as a warning, never blocks
  the cost report

## 8.11 Receipts as Episodic Memory (ADR-13)

Every attempt records:
- `task`, `attempt`, `worker`, `model`, `wall_clock_s`, `tokens` (optional),
  `ts` (unix), `outcome` (string), `error` (first line, optional),
  `cost_micros` (optional)
- Filename: `state/receipts/{task}-{attempt}-{ts}-{nanos}x.json`

Retry prompts (attempt ≥ 2) render the task's earlier failures so the agent
avoids repeating them. Cross-task episode recall is intentionally deferred
until tasks have types.

## 8.12 Polling with wake-on-completion

`af run` polls every `TF_POLL` seconds (default 15). After each attempt
finishes, the orchestrator **wakes the poller** immediately via in-process
state, so the loop never waits the full interval when actual work is queued.
This reduces average dispatch latency from O(poll) to O(1) for most of a
campaign (round 8: reduced 60s → 4.5s on the E2E corpus).

## 8.13 Stall watchdog and wall-clock budget

- **Stall watchdog** (`TF_AGENT_STALL_S`): kills an agent that produces no
  output for the configured window; distinct from total timeout
- **Wall-clock budget** (`TF_MAX_WALL_CLOCK_S`): stops the entire campaign
  early when the wall-clock exceeds the budget; exit code 3
- Both are independent. A zero value disables each feature.

## 8.14 Auto-reuse (round 13)

Before dispatching an agent for a task, `af run` looks for the newest archived
branch of that task whose changed paths satisfy the task's CURRENT scope.
When one qualifies, it re-runs the gate and merges it without invoking an
agent. Default ON; `TF_NO_REUSE=1` disables it. Use case: operator widens the
scope after a scope-failure; the archived work passes the widened scope and
can be merged for zero agent spend.
