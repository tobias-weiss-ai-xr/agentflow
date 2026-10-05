# Change: r8-spec-reconcile

## Why

Round 7 deliberately changed gate-failure retry behavior (`r7-gate-retry`):
a gate failure no longer deletes the attempt branch, and a retry whose
branch carries committed agent work beyond the base branch attaches to that
branch and re-runs ONLY the gate — the agent is never re-invoked for work
it already committed. The tests encoding this are green
(`gate_flake_retry_does_not_rerun_the_agent`,
`gate_failure_keeps_the_agents_committed_branch`,
`retry_after_gate_failure_runs_fresh_attempts_until_it_passes`), but the
scheduling spec still says the opposite — every attempt "SHALL use a newly
created branch" and a gate failure is "re-queued for a fresh attempt". The
implementation and its spec contradict each other, and the traceability
checker cannot catch it because it only verifies that a `// spec:` marker
exists — not that the referenced test agrees with the scenario's prose.

Separately, round 7 also added a user-visible waste section to `af cost`
(`r7-cost-waste`: `WASTED: …s on N of M attempt(s) (P%)` plus a
`WASTED BY REASON` breakdown, computed over the same receipt selection as
the rest of the report) that is absent from the cli spec's `Cost report`
requirement.

## What Changes

- **scheduling**: replace the now-false `Retry with fresh branch`
  requirement (which mandated a fresh branch per attempt) with
  `Retry reuses verified agent work`, stating the actual rule: up to
  `max_attempts` attempts; a gate failure keeps the attempt branch and the
  retry re-runs only the gate; scope violations, agent failures, and merge
  failures still discard the branch; a second gate failure on the reuse
  path drops the branch so the next attempt is a fresh agent run. Four
  scenarios, each backed by a green test.
- **cli**: extend the `Cost report` requirement to also specify the waste
  section — seconds and attempt count of non-`merged` attempts, the wasted
  percentage, and a breakdown grouped by failure reason, computed over the
  same receipt selection as the rest of the report (`--last` / `--since` /
  `--task` narrow it too). The existing trust scenario is preserved
  verbatim; one new scenario is added.
- **tests**: update the four stale markers referencing
  `scheduling/retry-with-fresh-branch` / `#retry-after-gate-failure` to the
  new requirement/scenario slugs, add scenario markers onto the round-7
  tests that prove the new scenarios, and add the waste marker onto
  `cost_report_surfaces_wasted_spend`. Comment-only edits; no assertion
  changes.
