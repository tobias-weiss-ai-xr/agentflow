# Proposal: turn-cap escalation on retry + gate/scope mismatch validation warning

## Why

The World-Office campaign (2026-10-14) lost five consecutive attempts to
`harness: turn cap 12 reached` even though the r19 calibration ("12 is enough")
was followed — the calibration holds only for simple write+commit+gate tasks.
Two systemic gaps surfaced:

1. **A turn-cap failure is a BUDGET signal, not a quality signal.** When an
   attempt dies at the cap, the retry is dispatched with the SAME cap, so a
   task that needs more turns than configured can never succeed — every retry
   burns full cost and dies the same death. The orchestrator already has the
   exact evidence (the receipt error `harness: turn cap N reached`) and already
   has episodic retry memory (ADR-13) fed from those same receipts.
2. **A gate that is broader than the task scope makes the task unsatisfiable.**
   A task scoped to two crates whose `accept` runs a whole-workspace gate fails
   forever on out-of-scope warnings, no matter how good the agent is. Nothing
   at validate time points this out.

## What Changes

- **Turn-cap escalation on retry (builtin harness)**: when a previous attempt
  of the same task failed with `harness: turn cap N reached`, the next attempt
  runs with a cap of `2 × N`, floored at the configured base and capped at a
  hard ceiling of **128 turns**. Evidence comes from stored receipts, so it
  survives `--resume` and state-dir reuse. First attempts and attempts whose
  predecessors failed for other reasons are unchanged.
- **Gate/scope mismatch warning at validate time**: if a task's `accept`
  contains a workspace-wide marker (`--workspace`) while `scope` is non-empty
  and narrow, `af validate` prints a non-fatal warning naming the unsatisfiable
  combination. No behavior change at run time.
- **Docs**: README task-schema rows for the escalation behavior and the
  mismatch warning.

## Capabilities

- **Modified `lifecycle`**: builtin-harness attempts escalate the turn cap on
  retry based on prior turn-cap failures recorded in receipts.
- **Modified `config`**: `af validate` warns on gate/scope mismatch (non-fatal).

## Impact

- `src/execute.rs` (turn computation at the harness call site), `src/config.rs`
  (validate warning), README task-schema section, spec deltas for `lifecycle`
  and `config`.
- Escalation is automatic and bounded (ceiling 128); existing configs behave
  identically for first attempts and non-turn-cap retries. The validate warning
  is non-fatal and purely advisory.
