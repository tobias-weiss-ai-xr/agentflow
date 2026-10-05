# Spec↔test traceability

`openspec/specs/<id>/spec.md` is the living contract for agentflow (the
archived OpenSpec change deltas were reconciled into it). Prose that nothing
checks is not a contract — so `tests/spec_traceability.rs` enforces that
**every requirement in the spec library is referenced by at least one test**,
and — one level finer — that **every `#### Scenario:` under a requirement is
referenced by its own test**. Run it with the rest of the suite: `cargo test`.

## The marker convention

A test that verifies a requirement carries a comment line

```rust
// spec: <spec-id>/<requirement-slug>
```

immediately above its `#[test]` fn (after any doc comments), and a test that
verifies one specific `#### Scenario:` under that requirement carries the
finer-grained form

```rust
// spec: <spec-id>/<requirement-slug>#<scenario-slug>
```

The slug is the same rule at both levels: the heading title lowercased, with
every run of non-alphanumeric characters replaced by a single `-` and trimmed.
For a requirement it is applied to the `### Requirement:` title; for a
scenario to the `#### Scenario:` title, appended to the requirement's id
after a `#`.

| Spec heading | Marker |
|---|---|
| `### Requirement: Cost receipts` (in `openspec/specs/state/spec.md`) | `// spec: state/cost-receipts` |
| `### Requirement: Scope contention avoidance` (in `openspec/specs/scheduling/spec.md`) | `// spec: scheduling/scope-contention-avoidance` |
| `#### Scenario: Overlapping scope deferred` under that requirement | `// spec: scheduling/scope-contention-avoidance#overlapping-scope-deferred` |
| `### Requirement: Worktrees target the task's repository` | `// spec: worktree/worktrees-target-the-task-s-repository` |

The two granularities compose in one direction only:

- A **scenario marker satisfies its requirement** — a test carrying
  `scheduling/scope-contention-avoidance#overlapping-scope-deferred`
  also counts as coverage of `scheduling/scope-contention-avoidance`, so
  upgrading a requirement marker to a scenario marker never breaks the
  requirement-level checker.
- A **requirement marker does NOT satisfy the scenarios under it** — that is
  the whole point: a requirement can hold five scenarios and one test. Every
  scenario needs its own referencing test (or an explicit allowlist entry
  below).

A test may carry several stacked markers when it genuinely verifies several
requirements/scenarios (e.g. a happy-path E2E covers the execute pipeline,
the dependency DAG, and the worktree lifecycle at once).

Example:

```rust
/// Gate exit-0 after agent writes DONE.txt → merged to main, all done.
// spec: lifecycle/execute-pipeline#happy-path
// spec: worktree/worktree-lifecycle#create-and-remove
#[test]
fn happy_path_dependency_and_merge() { ... }
```
## What the checker does

`tests/spec_traceability.rs`:

1. **Parses the contract** — every `openspec/specs/*/spec.md`, extracting
   each `### Requirement:` title into a `<spec-id>/<slug>` id and each
   `#### Scenario:` title (attached to its nearest preceding requirement)
   into a `<spec-id>/<slug>#<scenario-slug>` id.
2. **Scans the tests** — every `tests/*.rs` and `src/**/*.rs` file — for
   `// spec:` marker lines (only whole-line comments count; mentions in
   prose or string literals never register).
3. **`every_requirement_has_at_least_one_referencing_test`** — FAILS listing
   every requirement with no marker (requirement-level OR any of its
   scenarios' markers), each with the requirement title, its spec file, and
   the exact marker line to add.
4. **`every_scenario_has_a_referencing_test`** — FAILS listing every
   scenario with no exact `<req>#<scenario>` marker, each with the scenario
   title, its spec file, and the exact marker line to add. A
   requirement-level marker never satisfies a scenario.
5. **The allowlists** — `UNMAPPED` (requirements) and `UNMAPPED_SCENARIOS`
   hold `(id, reason)` pairs for entries that genuinely cannot be tested
   yet. Each list is capped at 3 entries so the gap can only shrink, and
   every entry must name a real spec entry and carry a non-empty reason
   (checked by `traceability_allowlist_stays_small`). Do NOT use an
   allowlist to paper over a scenario that has an obvious test — write the
   test instead; the small, shrinking cap is what makes the allowlist
   trustworthy.
6. **Self-check** — `a_bogus_requirement_fails_the_matcher` feeds the
   matcher a synthetic spec with an unreferenced requirement AND an
   unreferenced scenario, asserts both are reported (with actionable
   messages), that a requirement-level marker does NOT satisfy a scenario,
   and that a scenario marker satisfies both levels — so the checker itself
   is under test.

## Workflows

- **Adding a requirement to a spec**: `cargo test --test spec_traceability`
  goes red naming the new requirement and the marker to add. Either mark the
  test that verifies it, or write that test.
- **Adding a scenario (safely)**: add the `#### Scenario:` block under its
  requirement, run `cargo test --test spec_traceability`, and read the
  failure — it names the spec file and prints the exact marker line to add.
  Then either stack that marker above the existing test that already
  verifies the behavior, or (preferred when nothing covers it yet) write a
  small focused test for exactly that scenario and put the marker above it.
  Only if the scenario genuinely cannot be tested yet, add a
  `UNMAPPED_SCENARIOS` entry with a reason — the ≤3 cap means someone must
  ship a test (or remove an older entry) before the list can grow.
- **Deleting a test**: if it was the only marker for a requirement or
  scenario, the checker tells you which spec entry just lost its coverage.
- **Finding a scenario's tests**:
  `grep -rn "spec: <id>/<req-slug>#<scenario-slug>" tests/ src/`.

## Marker placement rules

- One marker line per requirement or scenario, stacked directly above
  `#[test]` (doc comments may sit above the markers).
- Markers may live in integration tests (`tests/*.rs`) or unit test
  modules (`src/**/*.rs`) — the checker scans both.
- Never invent a marker for an id that is not in the spec library; the
  checker's allowlist test also flags stale `UNMAPPED`/`UNMAPPED_SCENARIOS`
  entries, and markers that match no requirement or scenario are dead
  weight (review them away).
- Marker edits are comment-only: adding markers must never change what the
  test asserts.
