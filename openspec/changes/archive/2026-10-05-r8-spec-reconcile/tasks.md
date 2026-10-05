# Tasks

- [x] 1.1 Author the scheduling delta: REMOVE `Retry with fresh branch`, ADD
      `Retry reuses verified agent work` with the four scenarios
      (`gate failure retries only the gate`, `gate failure keeps the
      verified branch`, `a second gate failure falls back to a fresh
      attempt`, `attempts are bounded by max_attempts`) matching the
      round-7 implementation and its green tests.
- [x] 1.2 Author the cli delta: MODIFY `Cost report` as a full replacement
      that reproduces the trust scenario verbatim, adds the waste prose and
      the `wasted spend surfaces failed attempts` scenario.
- [x] 1.3 Update the stale markers in `tests/e2e.rs` (lines ~279, ~1017,
      ~1018, ~1075) to the new slugs; add scenario markers onto
      `gate_failure_keeps_the_agents_committed_branch` and the waste marker
      onto `tests/cli.rs::cost_report_surfaces_wasted_spend`. Keep the
      `UNMAPPED` / `UNMAPPED_SCENARIOS` allowlists empty.
- [x] 1.4 Validate (`openspec validate`), archive
      (`openspec archive r8-spec-reconcile --yes`), and re-validate the
      spec library (`openspec validate --specs`, `openspec list --specs`).
- [x] 1.5 Run the full acceptance gate: traceability suite, coverage gate,
      `cargo test`, `cargo fmt --check`, `cargo clippy --all-targets --
      -D warnings`.
