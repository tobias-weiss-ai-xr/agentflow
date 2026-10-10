# Proposal: harness tuning (write-time scope guard, per-task turns, readonly tasks, router prior)

## Why

Round-17 dogfooding showed three systemic knobs leave money/time on the table:
1. **70% wasted wall-clock**, most of it on attempts that die an hour in because a model-edited file was out of scope — the scope violation is only detected when the attempt ends. The harness owns the `write`/`edit` tools and can reject an out-of-scope path at tool-call time, killing the attempt in seconds instead of an hour.
2. **One global turn cap** over-caps small tasks (quickstart needed ~48) and under-caps audits (bug-hunt needed 128). Per-task budget is the natural knob.
3. **Trust routing on tiny samples**: with 1–2 attempts per worker, `wins/attempts` swings (0/2 → 0.00 (looks hopeless), 1/1 → 1.00) and routing lurches. A pessimistic prior de-noises small-N decisions without touching the multi-decision-metric architecture.

A `readonly` task mode is the fourth item: investigation-only tasks should not need to risk a narrow write scope, and should complete when the agent reports done without a merge or gate.

## What Changes

- **Write-time scope guard in the builtin harness**: `write`/`edit` tool calls against a path not covered by the task `scope` fail fast with a message naming the allowed scope (instead of at attempt end). The end-of-attempt check remains as the backstop for `bash`-tool writes and CLI workers.
- **Per-task `max_turns`** (`max_turns` field on a Task, overriding the worker default / `TF_AGENT_MAX_TURNS` / 32 for builtin workers).
- **`readonly` task mode**: a task flagged `readonly: true` completes when the agent stops normally — no gate, no merge — and the harness rejects all `write`/`edit` tool calls. Validation requires such tasks to declare no `accept` gate (a gate on a read-only task is contradictory).
- **Router Laplace prior**: the UCB1 scoring mean becomes `(wins + 1)/(attempts + 2)` instead of `wins/attempts`, so 0/1 scores 0.33 and 1/1 scores 0.67, de-noising small-N routing while converging to the empirical rate. The `af cost` TRUST column keeps showing the raw rate; the smoothed mean is documented as the routing mean.
- **Docs**: README task schema rows for `max_turns` / `readonly`, trust-routing paragraph for the prior; arc42 and the relevant spec deltas updated.

## Capabilities

- **Modified `config`**: `max_turns` and `readonly` join the Task schema with validation rules.
- **Modified `lifecycle`**: builtin-harness attempts fail fast on out-of-scope `write`/`edit`; `readonly` attempts complete without gate/merge.
- **Modified `scheduling`**: UCB1 uses a Laplace-smoothed trust mean.

## Impact

- `src/config.rs` (Task fields, validation), `src/harness.rs` (tool guard, effective turns, readonly), `src/execute.rs` (harness call site, readonly completion path), `src/router.rs` (prior), README + arc42, spec deltas.
- Behavior is additive and opt-in: existing configurations without the new fields behave exactly as before. Routing decisions change for existing fleets (prior), which is the point.
