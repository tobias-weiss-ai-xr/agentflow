# Proposal: multi-repo

## Why

taskfleet's final differentiator gap: cross-repo orchestration (monorepo
splits, coordinated service+docs+infra changes). Only one competitor
(agent-dispatch) offers it. `docs/multi-repo-design.md` already specifies the
design for the bash fork; this change implements it in the Rust orchestrator.

`Task.repo` is already parsed and ignored (v1); `Config.repos` makes it real.

## What Changes

1. **`repos.json`** (optional, next to tasks.json; `TF_REPOS_JSON` /
   `--repos` override): `{"repos": {"main": "/path", "docs": "../docs"}}`.
   Relative paths resolve against the repos.json file's directory.
   Missing file → single-repo mode (all tasks → `TF_REPO_DIR`), unchanged.
2. **Per-task repo resolution.** `repo: ""` → `TF_REPO_DIR`; `"main"` →
   `repos["main"]` if present, else `TF_REPO_DIR`; any other name →
   `repos[name]`. **Unknown names warn and fall back** to `TF_REPO_DIR`
   (ADR-4: the taskfleet corpus composes across files; a task referencing a
   sibling repo must not hard-fail single-repo planning — same treatment as
   dangling deps).
3. **Worktree/branch/merge target the task's repo.** Worktree dir stays
   `$TF_WORKTREE_ROOT/<task_id>/`; branch `prefix/<task_id>` and the merge
   happen in the resolved repo. Merge stays globally serialized (small
   diff, correct; per-repo locks if throughput ever matters).
4. **Self-heal spans repos.** Startup orphan-worktree cleanup tries every
   configured repo + the default repo.

Deps remain a global task DAG (already true) — cross-repo deps need no
special handling.

## Capabilities

### Modified
- `config` — repos.json loading, resolution, unknown-repo warning.
- `worktree` — per-task repo for create/remove/branch-delete/merge; heal
  across repos.

### Added
- `cli` — `--repos FILE` flag and `TF_REPOS_JSON` env.

## Impact

- `src/config.rs`: `Config.repos`, `load_repos`, `repo_dir_for`, validation.
- `src/run.rs`: heal across repos; unknown-repo warnings printed at startup.
- `src/execute.rs`: per-task repo for worktree create/merge/remove.
- `src/worktree.rs`: `heal` signature takes a repo list.
- `src/main.rs`: `--repos` flag.
- Docs: `docs/multi-repo-design.md` implementation summary, ADR-11.
- Backwards compatible: no repos.json → behavior identical (all 41 tests and
  the 64-config taskfleet corpus must stay green).
