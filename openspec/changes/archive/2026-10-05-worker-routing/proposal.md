# Proposal: worker-routing

## Why

Worker selection is currently "first free enabled worker" — a worker that
fails 80% of its tasks is chosen exactly as often as a reliable one. The
MoE-sovereign shortlist calls for measured routing: trust score + UCB1
affinity. In Rust the bash I/O constraints are moot; what survives is the
idea: **route work by track record, record every decision.**

The substrate already exists: per-attempt receipts (`task, worker, attempt,
model, wall_clock_s, ts`). What's missing is the outcome.

## What Changes

1. **Receipts record outcomes.** Every attempt end (merged *and* failed)
   appends a receipt; new `outcome` field (`"merged"`/`"failed"`, default
   `"merged"` for pre-existing receipts — they were only ever written on
   success). `af cost` still works: failed attempts now honestly count
   toward wall-clock cost.
2. **Router (UCB1).** At startup the run loop replays receipts into per-
   worker (wins, total). Free-worker selection scores
   `mean + sqrt(2·ln(N+1)/(n+1))`; ties break in config order (so a fresh
   install behaves exactly like today's first-free rule). Updated in-memory
   on every attempt end.
3. **Trust surfaced.** `af cost` gains a per-worker trust section
   (`wins/total`, rate).

No new files, no new dependencies, no knobs. Cumulative history (no decay)
is deliberate for v2 — ponytail: add windowing if stale workers haunt us.

## Capabilities

### Modified
- `state` — receipts carry `outcome`; failed attempts are recorded.
- `scheduling` — UCB1 worker selection among free workers.
- `cli` — `af cost` shows per-worker trust.

## Impact

- `src/state.rs`: `Receipt.outcome` (+ serde default), test.
- `src/execute.rs`: receipt append moves to a wrapper around the attempt.
- `src/router.rs` (new): Router (replay, record, pick, trust) + unit tests.
- `src/run.rs`: router wiring into `pick_worker`/`Msg::Done`; cost section.
- Compat: single worker / fresh state → selection identical to today; all
  existing tests must stay green.
