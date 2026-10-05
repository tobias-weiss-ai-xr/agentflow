# Proposal: durability-contracts

## Why

Rounds 1–2 of agentflow's durability work (pi-durable: single-writer lock,
effect-sandwich phase journal, resumable attempts) and round 4's contract work
(scope enforcement, prompt hardening, storage conformance, gate replay) all
shipped with tests — but with no spec. The traceability checker
(`tests/spec_traceability.rs`) only sees requirements under `openspec/specs/`,
so six shipped behaviors are invisible to the contract. Spec them so the
library describes what `af` actually does.

## What Changes

Add six requirements to the living spec library. All are `## ADDED`; no
existing behavior changes.

- `state` — Single-writer state lock, Attempt phase journal, Storage backend
  conformance suite.
- `lifecycle` — Gate replay contract, Prompt placeholder guarantee.
- `scheduling` — Scope enforcement on agent edits.

## Capabilities

### Modified
- `state` — three new robustness/persistence requirements.
- `lifecycle` — two new execution-contract requirements.
- `scheduling` — one new enforcement requirement.

## Impact

- `openspec/specs/{state,lifecycle,scheduling}/spec.md` gain the requirements
  at archive time.
- No source changes. The existing tests in
  `tests/{lock,journal,gate_replay,conformance,scope_enforcement,contract_prompt}.rs`
  gain `// spec:` traceability markers.
