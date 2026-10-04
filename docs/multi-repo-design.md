# Multi-Repo Task Support — Design Document

## Overview

Enable taskfleet tasks to target multiple git repositories, allowing cross-repo
refactors and coordinated changes across a monorepo split or micro-service suite.

## Motivation

*Only agent-dispatch (⭐30, Python) offers clean cross-repo orchestration among
the ~15 competitors we analyzed.* This is taskfleet’s final differentiator gap.

## Architecture

### Repo Configuration (`config/repos.json`)

```json
{
  "repos": {
    "main": "/path/to/main-repo",
    "docs": "/path/to/docs-site",
    "infra": "/path/to/infra-repo"
  }
}
```

Or (short names resolved relative to `$TF_DIR`):
```json
{
  "repos": {
    "main": "..",
    "docs": "../docs",
    "infra": "../infra"
  }
}
```

If no `repos.json` exists, single-repo mode is assumed with all tasks targeting
`$TF_REPO_DIR`.

### Task Schema Extension

```json
{
  "tasks": [
    {
      "id": "update-readme",
      "engine": "markdown",
      "title": "Update README",
      "repo": "main",           // <-- NEW: defaults to "main" or "" (tf_compatible)
      "section": "docs",
      "deps": [],
      "scope": ["README.md"],
      "accept": "git diff --stat",
      "manual": false
    },
    {
      "id": "update-docs-readme",
      "engine": "markdown",
      "title": "Update docs site README",
      "repo": "docs",          // <-- targets docs repo
      "section": "docs",
      "deps": ["update-readme"],
      "scope": ["index.md"],
      "accept": "git diff --stat"
    }
  ]
}
```

### Worktree Organization

The `$TF_WORKTREE_ROOT` is shared across all repos. Worktree paths remain
`$TF_WORKTREE_ROOT/<task_id>/`. A task’s branch lives in its designated repo:
`$TF_BRANCH_PREFIX/<task_id>` in the repo specified by the task’s `repo` field.

### Cross-Repo Dependencies

Dependencies (`deps`) are task IDs — they work across repos seamlessly:
- The directed graph is global (not partitioned per-repo)
- A task in repo "docs" can depend on a task in repo "main"
- The scheduler sees the full DAG regardless of repo boundaries
- **Partial hw6**: when task B (repo=docs) depends on task A (repo=main),
  task B's worktree is created once A is done. No special cross-repo
  worktree linking is needed — the dependency is purely at the task level.

## Implementation Summary (Current State)

### Implemented in agentflow (Rust, ADR-11) — current truth

- `config/repos.json` (optional, next to tasks.json; `TF_REPOS_JSON` or
  `--repos FILE` override): `{"repos": {"<name>": "<path>"}}`; relative
  paths resolve against the repos.json file's directory. Missing file =
  single-repo mode.
- `Config::repo_dir_for(task, default)` — `""` → default; `"main"` →
  `repos["main"]` or default; named → `repos[name]`. Unknown names **warn
  and fall back** to the default repo (ADR-4: the taskfleet corpus
  composes across files — never hard-fail planning).
- `execute_task` resolves the task's repo; worktree dir stays
  `$TF_WORKTREE_ROOT/<task_id>/`, branch `prefix/<task_id>` lives in the
  task's repo; merge lands in the task's repo.
- Merge serialization: one global in-process lock for all repos (correct
  for any repo count; per-repo keys if throughput ever matters).
- Self-heal scans all configured repos + the default (best-effort removal
  of orphan worktrees and stale branches).
- Deps stay a global task DAG — cross-repo deps need no special handling.
- Verified by e2e `multi_repo_campaign_merges_into_each_repo` (A on main,
  B on auxrepo, dep A→B, each merges into its own repo) and
  `unknown_repo_warns_and_falls_back`.

### Historical: bash fork (taskfleet) plan

- `REPOS_JSON` config path in `lib/common.sh`
- `tf_task_repo <task_id>` — returns repo name from task's `repo` field ("" for default/main)
- `tf_repo_dir <repo_name>` — resolves repo name to absolute path via `repos.json`
- `lib/worktree.sh` — `tf_worktree_create`/`tf_worktree_merge` use task-specific repos
- `lib/dispatch.sh` — pass task's repo info through worktree calls

## Backward Compatibility

- No `repos.json` (or no `"main"` key) → single-repo mode, behavior
  unchanged; `repo: "main"` falls back to the default repo silently, so
  taskfleet corpora that annotate `repo: "main"` keep planning identically.
- Unknown repo names warn and fall back to the default repo (same
  treatment as dangling deps).
- Cross-repo deps are just DAG deps — already ordered by the scheduler.
