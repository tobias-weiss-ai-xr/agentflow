# Design: First-class verify tasks + per-contract acceptance

**Date:** 2026-10-18
**Status:** Approved (Sections 1-3) — pending user review of this document
**Scope:** `verify: true` task flag + `accept_report.json` per-contract acceptance for agentflow (`af`).

## Problem

Two gaps from `docs/af-usage-feedback.md`:

- **§4 First-class verify tasks.** A spec/contract slice ("write probe, must pass, archive receipt") has no useful code diff. The current model assumes a code commit before the acceptance gate, so such a slice can only be expressed awkwardly (e.g. `manual`, or a pointless change).
- **§5 Per-contract acceptance granularity.** `accept` is a single exit-0 command → binary pass/fail. It cannot tell which contract (C-M1…C-Px) broke, so a failed slice is retried blind.

This design adds a `verify: true` task flag and an optional `accept_report.json` emitted by the acceptance gate, persisting per-contract verdicts through the receipt ledger and `status --json`, and seeding them into retry prompts.

---

## Section 1 — Config & behaviour

### `verify: bool` on Task (`src/config.rs`)

New independent boolean on `Task`, `#[serde(default)]` (default `false`), alongside the existing `readonly`/`manual`/`accept` fields.

```toml
[[tasks]]
id = "verify-license-header"
verify = true
scope = ["src/**"]
accept = "bash ./scripts/check-license.sh"   # exits 0 on pass
```

### Semantics

A `verify` task is a **no-merge acceptance task**: its deliverable is a passing gate, not a code diff merged to base.

- **Success** = agent runs + acceptance gate exits 0.
- **Never merge** (locked): the attempt branch is archived (existing `preserve_and_note`), never merged to base.
- **No-diff is allowed to pass** (locked): a `verify` task whose agent produced **0 commits ahead of base** still runs the gate and can pass — the gate is the sole deliverable.
- **Gate failure** behaves exactly like a normal task's gate failure (`execute.rs:631-661`): the attempt is `Failed`, the committed work is preserved for a gate-only retry.
- **New receipt outcome** `verified`, which **counts as a verdict** on the worker's reliability (added to `counts_as_verdict()`).

### Validation (`src/config.rs` `validate`, mirroring the `readonly`+`accept` check at `config.rs:364-369`)

`af validate` rejects:

- `verify: true` **and** `manual: true` — a verify task must run its gate; skipping it is contradictory.
- `verify: true` **and** `readonly: true` — `readonly` forbids the gate (`config.rs:364-369` already rejects `readonly`+`accept`); a verify task's gate is its whole point, so `readonly` + `verify` is contradictory.
- `verify: true` **without an `accept` gate** — the gate is a verify task's sole deliverable; a verify task with no gate would "pass" with nothing verified. This is a hard validation error (unlike the `no gate and not manual` warning at `config.rs:371-376`).

---

## Section 2 — Data model

### `Outcome` enum (`src/execute.rs:15-18`)

Add a third variant:

```rust
pub enum Outcome {
    Merged,
    Failed(String),
    Verified,
}
```

Extend the receipt mapping at `execute.rs:109-116`:

- `Outcome::Verified => "verified"` (outcome name).
- `Outcome::Verified => None` (error — a verified attempt has no failure reason).

### `ContractVerdict` + `Receipt.contracts` (`src/state.rs`)

New small struct (a Vec-of-pairs, not a HashMap, so per-contract order is preserved):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractVerdict {
    pub contract: String,
    pub verdict: String,
}
```

`Receipt` (state.rs:101-123) gains:

```rust
/// Optional per-contract verdicts from an accept_report.json (verify tasks).
#[serde(default)]
pub contracts: Option<Vec<ContractVerdict>>,
```

### `counts_as_verdict()` (`src/state.rs:145-147`)

Add `verified`:

```rust
self.outcome == "merged" || self.outcome == "failed" || self.outcome == "verified"
```

### `status --json` (`src/run.rs:1629-1645`)

Per task, add (derived from the latest receipt):

- `"verify"` — the task's `verify` flag.
- `"verdict"` — the latest outcome: `"merged" | "failed" | "verified"`.
- `"contracts"` — the latest per-contract verdicts (from `receipt.contracts`), when present.

The existing `state`/`attempts`/`last_error` fields are unchanged.

---

## Section 3 — Verify task flow, report parsing, retry seeding

All in `src/execute.rs`.

### 3a. Where `verify` branches in `execute_attempt`

- **No-change is not success — but is for verify.** The `ahead == 0` failure arm at `execute.rs:548-560` is wrapped in `if !task.verify { ... }`. For a verify task, zero commits ahead does **not** fail the attempt; the flow continues to the gate.
- **Gate is the sole deliverable.** The gate block (`execute.rs:617-668`) runs unchanged for verify tasks — same `accept`, same `run_accept`, same exit-0 = pass. `record_phase(..., GatePassed)` at line 666 still fires.
- **Never merge — archive only.** After `GatePassed`, the merge block (lines 670-701) becomes:

```text
if task.verify {
    parse accept_report.json from wt_path            // 3b
    let _ = preserve_and_note(&repo, &wt, id, attempt, &mut append);  // archive branch
    cleanup(&repo, &wt);
    return (Outcome::Verified, spend);               // carries contracts
} else {
    ... existing merge block (670-701) ...
}
```

`contracts` are threaded through to the receipt append at `execute.rs:117-128` (new `contracts` field on the `Receipt` built there).

### 3b. `accept_report.json` parsing

- After the gate passes, if `wt_path/accept_report.json` exists and parses as a JSON object `{"C-M1": "pass", "C-P2": "fail", ...}`, read it into the per-contract verdicts (sorted by key for deterministic ordering).
- **Supplement only** (locked): a missing or unparseable report does **not** fail the task. Gate exit 0 is still a pass; the report only adds per-contract detail. Parse errors are best-effort ignored (a note is appended to the attempt journal, not fatal).
- Verdicts flow into `Receipt.contracts` and surface in `status --json`.

### 3c. Retry-prompt seeding (`failure_context`, `src/execute.rs:135-153`)

Extend the existing "Previous attempts" block. For prior **failed** receipts of this task (the existing `r.outcome == "failed"` filter at line 141), when the receipt carries `contracts`, additionally append the failing contract names:

```text
- attempt N: <error> (failed contracts: C-M2, C-P1)
```

This reuses the same `receipts` input and the loop that builds the block at lines 148-151 — a contained change.

### 3d. Error handling summary

- Gate fails → existing path (`execute.rs:631-661`): `Failed`, branch preserved for gate-only retry — unchanged for verify tasks.
- Report missing/unparseable but gate passes → still `Verified`, no contracts.
- Archive fails → best-effort (existing `preserve_and_note` returns `None`; the receipt is still written).

---

## Notes & non-goals

- **No integration with openprose/prose** (evaluated 2026-10-18): the `acceptance_prose` field (`config.rs:93`) is a prompt-facing natural-language acceptance description (`{{ACCEPTANCE}}` placeholder), unrelated to the openprose language. No coupling is introduced.
- **Non-goal:** standing-goal / re-render semantics. agentflow remains a one-shot task pipeline (`task → agent → gate → merge-or-archive`); `verify` only changes the terminal outcome (archive instead of merge) and adds per-contract granularity.
- **Non-goal:** changing gate semantics. The gate remains a deterministic shell command (exit-0 = pass); `accept_report.json` only supplements it.

## Testing

- Unit: `Outcome::Verified` receipt mapping; `counts_as_verdict` includes `verified`; validate rejects `verify`+`manual`, `verify`+`readonly`, and `verify` without an `accept` gate.
- Contract/CLI: a `verify` task whose agent produces 0 commits and a passing gate yields `verified` and does **not** merge (branch archived, base unchanged); a failing gate yields `failed`; `accept_report.json` parsed → per-contract verdicts surface in `status --json`.
- Retry seeding: a failed verify task with `contracts` seeds failing contract names into the next attempt prompt.
