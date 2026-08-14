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

### Added
- `REPOS_JSON` config path in `lib/common.sh`
- `tf_task_repo <task_id>` — returns repo name from task’s `repo` field ("" for default/main)
- `tf_repo_dir <repo_name>` — resolves repo name to absolute path via `repos.json`

### Modified (Planned)
- `lib/worktree.sh` — `tf_worktree_create` and `tf_worktree_merge` to use task-specific repos
- `lib/dispatch.sh` — pass task’s repo info through worktree calls

### Worktree.sh Changes Required

In each git operation, use `tf_repo_dir "$(tf_task_repo "$id")"` instead of
`$TF_REPO_DIR`. Key functions:
- `tf_worktree_create` — resolve repo dir from task, use for all git ops
- `tf_worktree_merge` — merge into task’s repo (not always $TF_REPO_DIR)
- `tf_worktree_remove` — remove worktree from correct repo
- `tf_worktree_delete_branch` — delete branch from task’s repo
- `tf_worktree_conflicts` — check conflicts in task’s repo

### Validation Rules

- Every task’s `repo` field must resolve to a valid repo in `repos.json`
  OR be empty (defaults to `$TF_REPO_DIR`)
- Circular cross-repo dependencies are allowed (it’s a DAG, not a repo graph)

## Backward Compatibility

- single
- If `repos.json` does not exist or only contains `"main"` key → behavior
  unchanged from current single-repo mode
- The default is Truthy
