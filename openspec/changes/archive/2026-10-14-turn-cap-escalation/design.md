# Design: turn-cap escalation + gate/scope mismatch warning

## Context

The builtin harness resolves the turn cap inside `harness::run` via
`effective_max_turns` (task override → worker → `TF_AGENT_MAX_TURNS` → 32).
The orchestrator persists a receipt for every attempt with the exact failure
string (`harness: turn cap N reached`), and `execute_attempt` already loads
those receipts for episodic retry memory (ADR-13 `failure_context`). The
escalation reuses exactly that evidence — no new state, no new config knob.

## Goals / Non-Goals

- Goals: a turn-cap death can never repeat at the same cap; authors get
  pointed at unsatisfiable gate/scope combinations before dispatching.
- Non-Goals: no configurable ceiling knob (128 is a hard constant — a config
  knob for "allow the agent even more turns" is speculative); no escalation
  for CLI workers (their turn caps are enforced by the external CLI, not by
  receipts we can read); no auto-retry loop extension (max_attempts stays the
  bound).

## Decisions

### D1: Escalation computed at the harness call site from receipts

`execute.rs` computes the effective turn cap as:

```
prior turn-cap caps N_i from receipts (task == id, attempt < current, failed)
if any: cap = clamp(2 × max(N_i), ≤ 128), overriding any base
else:   pass task.max_turns through unchanged
```

This lives in a small pure function `escalate_turns(receipts, id, attempt,
base) -> Option<u32>` next to `failure_context` (same evidence source, same
filter, unit-testable without an LLM). The harness call site replaces
`task.max_turns` with the computed value. Rationale: receipts survive crash,
resume, and state-dir reuse; parsing one canonical error string we own
(harness emits it) is the cheapest possible signal — no new fields, no
migration.

Edge: `2 × N` uses `saturating_mul` then `.min(128)`, so a 128-cap death
retries at 128 (bounded thrash, never 256). A mismatched line (dep changed
the error format) degrades to no-escalation — the parse is total.

### D2: Warning as a validate-time heuristic, one marker

`config.rs::validate` adds: if `accept` contains `--workspace` AND `scope`
is non-empty AND no scope entry starts with `**`, push a non-fatal warning.
Rationale: `--workspace` is the one unambiguous "gate is broader than any
narrow scope" marker in this ecosystem (cargo); heuristics for `pytest` /
`pnpm test` are ambiguous (often fine with rootdir args). One honest rule
beats five fuzzy ones — extend the marker list only when a real campaign
loses attempts to a second pattern.

### D3: Ceiling constant, not config

`const TURN_CAP_CEILING: u32 = 128;` with a `ponytail:` comment naming the
upgrade path (env knob) if long-horizon tasks ever legitimately need more.

## Risks / Trade-offs

- Doubling on a turn-cap retry can let a thrashing agent burn up to 2× the
  original budget per retry — accepted: the alternative (same cap) is a
  guaranteed identical failure, i.e., 100% waste.
- The warning is string-matching on user config — a false warning costs one
  log line, a missed one costs nothing new (status quo).

## Migration Plan

Additive. No config format change, no state migration. Behavior change is
scoped to retry attempts after turn-cap deaths.

## Open Questions

- None.
