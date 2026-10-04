## Why

Fair comparison of implementation languages for af's core loop. Rust works; a stdlib-only Go port on a branch makes tradeoffs concrete (build times, LOC, error handling, single-binary story).

## What Changes

- New `go/` module implementing the af core subset: tasks/workers JSON config, per-task git worktree, subprocess agent dispatch (prompt file), acceptance gate, retry with attempts, atomic run-state JSON, ff-merge on success, logs; CLI `run`/`status`/`attach`
- E2E Go test mirroring `tests/e2e.rs` (fake agent, git fixture)
- No third-party Go deps (parity with af's std-only policy)

## Impact

- New `go/` tree only; Rust implementation untouched
