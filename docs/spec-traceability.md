# Spec↔test traceability

`openspec/specs/<id>/spec.md` is the living contract for agentflow (the
archived OpenSpec change deltas were reconciled into it). Prose that nothing
checks is not a contract — so `tests/spec_traceability.rs` enforces that
**every requirement in the spec library is referenced by at least one test**.
Run it with the rest of the suite: `cargo test`.

## The marker convention

A test that verifies a requirement carries a comment line

```rust
// spec: <spec-id>/<requirement-slug>
```

immediately above its `#[test]` fn (after any doc comments). The slug is the
requirement title lowercased, with every run of non-alphanumeric characters
replaced by a single `-` and trimmed:

| Spec heading | Marker |
|---|---|
| `### Requirement: Cost receipts` (in `openspec/specs/state/spec.md`) | `// spec: state/cost-receipts` |
| `### Requirement: Scope contention avoidance` (in `openspec/specs/scheduling/spec.md`) | `// spec: scheduling/scope-contention-avoidance` |
| `### Requirement: Worktrees target the task's repository` | `// spec: worktree/worktrees-target-the-task-s-repository` |

A test may carry several stacked markers when it genuinely verifies several
requirements (e.g. a happy-path E2E covers the execute pipeline, the
dependency DAG, and the worktree lifecycle at once).

Example:

```rust
/// Gate exit-0 after agent writes DONE.txt → merged to main, all done.
// spec: lifecycle/execute-pipeline
// spec: worktree/worktree-lifecycle
#[test]
fn happy_path_dependency_and_merge() { ... }
```

## What the checker does

`tests/spec_traceability.rs`:

1. **Parses the contract** — every `openspec/specs/*/spec.md`, extracting
   each `### Requirement:` title into a `<spec-id>/<slug>` id.
2. **Scans the tests** — every `tests/*.rs` and `src/**/*.rs` file — for
   `// spec:` marker lines (only whole-line comments count; mentions in
   prose or string literals never register).
3. **`every_requirement_has_at_least_one_referencing_test`** — FAILS listing
   every requirement with no marker, each with the requirement title, its
   spec file, and the exact marker line to add.
4. **`traceability_allowlist_stays_small`** — the `UNMAPPED` const holds
   `(requirement, reason)` pairs for requirements that genuinely cannot be
   tested yet. It is capped at 3 entries so the gap can only shrink, and
   every entry must name a real requirement and carry a reason. Do NOT use
   `UNMAPPED` to paper over a requirement that has an obvious test — write
   the test instead.
5. **Self-check** — `a_bogus_requirement_fails_the_matcher` feeds the
   matcher a synthetic spec with an unreferenced requirement and asserts it
   is reported, so the checker itself is under test.

## Workflows

- **Adding a requirement to a spec**: `cargo test --test spec_traceability`
  goes red naming the new requirement and the marker to add. Either mark the
  test that verifies it, or write that test.
- **Deleting a test**: if it was the only marker for a requirement, the
  checker tells you which requirement just lost its coverage.
- **Finding a requirement's tests**: `grep -rn "spec: <id>/<slug>" tests/ src/`.

## Marker placement rules

- One marker line per requirement, stacked directly above `#[test]`
  (doc comments may sit above the markers).
- Markers may live in integration tests (`tests/*.rs`) or unit test
  modules (`src/**/*.rs`) — the checker scans both.
- Never invent a marker for an id that is not in the spec library; the
  checker's allowlist test also flags stale `UNMAPPED` entries, and markers
  that match no requirement are dead weight (review them away).
